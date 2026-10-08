//! Class elaboration — extract dari mod.rs untuk Arsitektur-06
//! (elaborator monolithic → submodule).
//!
//! Fungsi-fungsi ini menghasilkan `IrClassDef` dari AST `ClassDecl`,
//! termasuk parent class method/field merging (recursively).

use std::collections::HashMap;

use mivon_ast::types::{ClassMember, TypeParam};
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::{IrClassDef, IrClassField, IrClassMethod, IrTypeParam};

use super::Elaborator;

/// Merge `rand` fields warisan: parent-first (rantai `extends`), lalu rand
/// milik sendiri; field yang di-redeclare NON-rand di child mengeluarkan
/// nama dari daftar. Tanpa ini field warisan tak pernah di-sample solver →
/// `randomize() with` di subclass selalu gagal (temuan tahap 7).
/// Sejajar merge fields/methods (order-dependent sama: parent harus sudah
/// ada di map; guard siklus + depth 16).
fn merged_rand_fields(
    classes: &HashMap<Symbol, IrClassDef>,
    cd: &mivon_ast::types::ClassDecl,
    own_rand: &[Symbol],
) -> Vec<Symbol> {
    use std::collections::HashSet;
    let mut out: Vec<Symbol> = Vec::new();
    let mut seen: HashSet<Symbol> = HashSet::new();
    // Rantai ancestor, child-first → dibalik jadi parent-first.
    let mut chain: Vec<Symbol> = Vec::new();
    let mut visited: HashSet<Symbol> = HashSet::new();
    let mut cur = cd.extends;
    let mut guard = 0;
    while let Some(pn) = cur {
        if guard >= 16 {
            break;
        }
        guard += 1;
        let key = pn.split("::").last().unwrap_or_else(|| pn.as_str());
        let Some(pd) = classes.get(&Symbol::intern(key)) else {
            break;
        };
        if !visited.insert(pd.name) {
            break;
        }
        chain.push(pd.name);
        cur = pd.extends;
    }
    for cn in chain.iter().rev() {
        if let Some(pd) = classes.get(cn) {
            for r in &pd.rand_fields {
                if seen.insert(*r) {
                    out.push(*r);
                }
            }
        }
    }
    // Override: redeclare non-rand di child mengeluarkan nama warisan.
    let own_nonrand: HashSet<Symbol> = cd
        .members
        .iter()
        .filter_map(|m| match m {
            ClassMember::Decl(d) => Some(
                d.names
                    .iter()
                    .filter(|v| !v.is_rand)
                    .map(|v| v.name)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    out.retain(|n| !own_nonrand.contains(n));
    for r in own_rand {
        if seen.insert(*r) {
            out.push(*r);
        }
    }
    out
}

/// Standalone helper — resolve width tipe class field, termasuk generic type param.
pub(super) fn resolve_class_field_width_standalone(
    dtype: &mivon_ast::types::DataType,
    type_params: &[TypeParam],
) -> usize {
    if let mivon_ast::types::DataType::UserDefined(name) = dtype {
        if let Some(tp) = type_params.iter().find(|tp| tp.name == *name) {
            if let Some(ref default_dt) = tp.default_type {
                return default_dt.width();
            }
        }
    }
    dtype.width()
}

impl Elaborator {
    /// Resolve width class field — delegasi ke standalone helper.
    pub(super) fn resolve_class_field_width(
        &self,
        dtype: &mivon_ast::types::DataType,
        type_params: &[TypeParam],
    ) -> usize {
        resolve_class_field_width_standalone(dtype, type_params)
    }

    /// Elaborate semua class declaration → HashMap<Symbol, IrClassDef>.
    ///
    /// Field & method parent di-merge recursively (parent duluan).
    /// Method override: child mengganti parent jika nama sama.
    pub(super) fn elaborate_classes(&self) -> Result<HashMap<Symbol, IrClassDef>, SimError> {
        let mut classes = HashMap::new();
        for cd in &self.design.classes {
            let mut fields = Vec::new();
            for member in &cd.members {
                if let ClassMember::Decl(decl) = member {
                    for dv in &decl.names {
                        let decl_width =
                            self.resolve_class_field_width(&decl.dtype, &cd.type_params);
                        let var_width = dv.resolved_width(&HashMap::new()).unwrap_or(1);
                        let elem_width = decl_width.max(var_width).max(1);
                        let (array_depth, actual_elem_width) = if let Some(ar) = &dv.array_range {
                            let depth = if ar.msb >= ar.lsb {
                                ar.msb - ar.lsb + 1
                            } else {
                                ar.lsb - ar.msb + 1
                            };
                            (depth, elem_width)
                        } else {
                            (1, elem_width)
                        };
                        let total_width = array_depth * actual_elem_width;
                        fields.push(IrClassField {
                            name: dv.name,
                            width: total_width,
                            array_depth,
                            elem_width: actual_elem_width,
                            dtype: Some(decl.dtype.clone()),
                        });
                    }
                }
            }
            let mut methods: Vec<IrClassMethod> = cd
                .members
                .iter()
                .filter_map(|m| match m {
                    ClassMember::Function(fd) => Some(IrClassMethod {
                        name: fd.name,
                        is_task: false,
                        virtual_flag: fd.virtual_flag,
                        is_static: fd.is_static,
                        ports: fd.ports.clone(),
                        decls: fd.decls.clone(),
                        stmts: fd.stmts.clone(),
                    }),
                    ClassMember::Task(td) => Some(IrClassMethod {
                        name: td.name,
                        is_task: true,
                        virtual_flag: td.virtual_flag,
                        is_static: td.is_static,
                        ports: td.ports.clone(),
                        decls: td.decls.clone(),
                        stmts: td.stmts.clone(),
                    }),
                    _ => None,
                })
                .collect();
            // Merge parent class methods (recursively) — parent methods come before child methods
            if let Some(ref parent_name) = cd.extends {
                let parent_key = parent_name
                    .split("::")
                    .last()
                    .unwrap_or_else(|| parent_name.as_str());
                let mut merged_methods = Vec::new();
                let mut seen_methods: std::collections::HashSet<Symbol> =
                    std::collections::HashSet::new();
                if let Some(parent_cd) = classes.get(&Symbol::intern(parent_key)) {
                    let mut ancestors: Vec<&IrClassDef> = vec![parent_cd];
                    loop {
                        let current = ancestors.last().unwrap();
                        if let Some(ref gp) = current.extends {
                            let gp_key = gp.split("::").last().unwrap_or_else(|| gp.as_str());
                            if let Some(gp_cd) = classes.get(&Symbol::intern(gp_key)) {
                                ancestors.push(gp_cd);
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    for anc in ancestors.iter().rev() {
                        for m in &anc.methods {
                            if seen_methods.insert(m.name) {
                                merged_methods.push(m.clone());
                            }
                        }
                    }
                }
                for m in &methods {
                    let method_name: Symbol = m.name;
                    if seen_methods.insert(method_name) {
                        merged_methods.push(m.clone());
                    } else if let Some(pos) = merged_methods.iter().position(|pm| pm.name == m.name)
                    {
                        merged_methods[pos] = m.clone();
                    }
                }
                methods = merged_methods;
            }
            let constraints: Vec<(Symbol, bool, Vec<mivon_ast::types::ConstraintItem>)> = cd
                .members
                .iter()
                .filter_map(|m| {
                    if let ClassMember::Constraint {
                        name,
                        body,
                        is_static,
                    } = m
                    {
                        Some((*name, *is_static, body.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            let rand_fields: Vec<Symbol> = cd
                .members
                .iter()
                .flat_map(|m| {
                    if let ClassMember::Decl(decl) = m {
                        decl.names
                            .iter()
                            .filter(|dv| dv.is_rand)
                            .map(|dv| dv.name)
                            .collect::<Vec<_>>()
                    } else {
                        vec![]
                    }
                })
                .collect();
            let lets: Vec<mivon_ast::types::LetDecl> = cd
                .members
                .iter()
                .filter_map(|m| match m {
                    ClassMember::Let(ld) => Some(ld.clone()),
                    _ => None,
                })
                .collect();
            // Merge parent class fields (recursively) — parent fields come before child fields
            let all_fields = if let Some(ref parent_name) = cd.extends {
                let parent_key = parent_name
                    .split("::")
                    .last()
                    .unwrap_or_else(|| parent_name.as_str());
                let mut merged = Vec::new();
                let mut seen = std::collections::HashSet::new();
                if let Some(parent_cd) = classes.get(&Symbol::intern(parent_key)) {
                    let mut ancestors: Vec<&IrClassDef> = vec![parent_cd];
                    loop {
                        let current = ancestors.last().unwrap();
                        if let Some(ref gp) = current.extends {
                            let gp_key = gp.split("::").last().unwrap_or_else(|| gp.as_str());
                            if let Some(gp_cd) = classes.get(&Symbol::intern(gp_key)) {
                                ancestors.push(gp_cd);
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    for anc in ancestors.iter().rev() {
                        for f in &anc.fields {
                            if seen.insert(f.name) {
                                merged.push(f.clone());
                            }
                        }
                    }
                }
                for f in &fields {
                    if seen.insert(f.name) {
                        merged.push(f.clone());
                    } else if let Some(pos) = merged.iter().position(|pf| pf.name == f.name) {
                        merged[pos] = f.clone();
                    }
                }
                merged
            } else {
                fields
            };

            classes.insert(
                cd.name,
                IrClassDef {
                    name: cd.name,
                    extends: cd.extends,
                    type_params: cd
                        .type_params
                        .iter()
                        .map(|tp| IrTypeParam {
                            name: tp.name,
                            default_type: tp.default_type.clone(),
                        })
                        .collect(),
                    fields: all_fields,
                    methods,
                    constraints,
                    rand_fields: merged_rand_fields(&classes, cd, &rand_fields),
                    lets,
                },
            );
        }
        Ok(classes)
    }

    /// Ekstrak fields struct package untuk ekspresi override param instance.
    pub(super) fn struct_override_fields(
        &self,
        expr: &mivon_ast::expr::Expr,
        effective_params: &HashMap<Symbol, i64>,
    ) -> Option<Vec<super::SField>> {
        let base: Option<Symbol> = match expr {
            mivon_ast::expr::Expr::Ident { name, .. } => Some(*name),
            mivon_ast::expr::Expr::ScopedIdent { item, .. } => Some(*item),
            mivon_ast::expr::Expr::BitSelect { expr: inner, index } => {
                if let mivon_ast::expr::Expr::Ident { name, .. } = inner.as_ref() {
                    match super::const_eval_with_params(index, effective_params) {
                        Ok(idx) => Some(Symbol::intern(&format!("{}[{}]", name.as_str(), idx))),
                        Err(_) => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let base = base?;
        self.pkg_struct_ref_index.get(&base).cloned()
    }
}
