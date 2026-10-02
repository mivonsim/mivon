//! Codegen — definisi bersama & class: emit_typedef (alias/struct/union/enum),
//! emit_package, emit_interface (+modport), emit_class (+constraint),
//! emit_constraint_items. 1 file = 1 tanggung jawab.

use super::expr::{emit_expr, emit_type};
use super::module::emit_args;
use super::{line, pad_type};
use crate::ast::*;

/// Emit interface SV: `interface axi_lite; ... endinterface`.
/// Port (`in/out`) & `sig` sama-sama jadi deklarasi signal; modport
/// menamai subset signal dengan arah (MIVON-HDL.md §6.10).
pub(crate) fn emit_interface(out: &mut String, indent: usize, ifc: &Interface) {
    line(out, indent, &format!("interface {};", ifc.name));
    for p in &ifc.ports {
        for n in &p.names {
            line(
                out,
                indent + 1,
                &format!("{};", super::emit_signal_decl(&p.ty, n)),
            );
        }
    }
    for (names, ty, ..) in &ifc.sigs {
        // Pakai `emit_signal_decl_multi` (bukan `emit_type`): dimensi
        // unpacked SV harus SETELAH nama sinyal (`logic [7:0] m [0:3]`),
        // `emit_type` menaruhnya sebelum nama (`logic [7:0] [3] m`) yang
        // bukan grammar SV — LRM 1800 §7.3.
        line(
            out,
            indent + 1,
            &format!("{};", super::emit_signal_decl_multi(ty, names)),
        );
    }
    for mp in &ifc.modports {
        let parts: Vec<String> = mp
            .dirs
            .iter()
            .map(|(dir, names)| {
                let d = match dir {
                    Dir::In => "input",
                    Dir::Out => "output",
                    Dir::Inout => "inout",
                };
                format!("{d} {}", names.join(", "))
            })
            .collect();
        line(
            out,
            indent + 1,
            &format!("modport {} ({});", mp.name, parts.join(", ")),
        );
    }
    line(out, indent, "endinterface");
}

pub(crate) fn emit_package(out: &mut String, indent: usize, pkg: &Package) {
    line(out, indent, &format!("package {};", pkg.name));
    // Konstanta package DILETAKKAN lebih dulu, baru typedef. Dalam SV,
    // deklarasi harus tampil SEBELUM dipakai (LRM 1800 §5.6/§8.2): lebar
    // enum/typedef sering merujuk konstanta package
    // (`const W = 4` + `enum(W) Dyn`), dan urutan tulis `.mv` tidak
    // menjamin const lebih dulu. Emitter lama menulis typedef dulu →
    // forward reference → iverilog/verilator error.
    for (name, ty, value) in &pkg.consts {
        let ty_s = ty
            .as_ref()
            .map(|t| format!("{} ", emit_type(t)))
            .unwrap_or_default();
        line(
            out,
            indent + 1,
            &format!("localparam {ty_s}{name} = {};", emit_expr(value)),
        );
    }
    for td in &pkg.typedefs {
        emit_typedef(out, indent + 1, td);
    }
    line(out, indent, "endpackage");
}

pub(crate) fn emit_typedef(out: &mut String, indent: usize, td: &Typedef) {
    match td {
        Typedef::Alias { name, ty, .. } => {
            // For typedef, array dims go AFTER name (SV unpacked array syntax):
            // `type A = logic[8][4]` → `typedef logic [7:0] A [0:3];`
            // Use emit_signal_decl_multi logic with single name.
            let mut dims: Vec<&Expr> = Vec::new();
            let mut elem = ty;
            while let MvType::Array(inner, ds) = elem {
                dims.extend(ds.iter());
                elem = inner;
            }
            // dims collected from outermost to innermost; reverse for SV order
            // (first array dim in source = first dim after name in SV).
            dims.reverse();
            if dims.is_empty() {
                line(out, indent, &format!("typedef {} {name};", emit_type(ty)));
            } else {
                let ty_s = super::expr::emit_type(elem);
                let mut dim_s = String::new();
                for d in dims {
                    let n = match d {
                        Expr::Int(v) => format!("{}", v.saturating_sub(1)),
                        other => format!("{} - 1", super::expr::emit_expr(other)),
                    };
                    dim_s.push_str(&format!(" [0:{n}]"));
                }
                line(out, indent, &format!("typedef {ty_s} {name}{dim_s};"));
            }
        }
        Typedef::Struct {
            name,
            packed,
            fields,
            ..
        } => {
            let pk = if *packed { " packed" } else { "" };
            line(out, indent, &format!("typedef struct{pk} {{"));
            for f in fields {
                let names = f.names.join(", ");
                let ty_s = pad_type(emit_type(&f.ty));
                line(out, indent + 1, &format!("{ty_s}{names};"));
            }
            line(out, indent, &format!("}} {name};"));
        }
        Typedef::Union {
            name,
            packed,
            fields,
            ..
        } => {
            let pk = if *packed { " packed" } else { "" };
            line(out, indent, &format!("typedef union{pk} {{"));
            for f in fields {
                let names = f.names.join(", ");
                let ty_s = pad_type(emit_type(&f.ty));
                line(out, indent + 1, &format!("{ty_s}{names};"));
            }
            line(out, indent, &format!("}} {name};"));
        }
        Typedef::Enum {
            name,
            width,
            members,
            ..
        } => {
            let w = match width {
                Some(Expr::Int(n)) => format!("{}", n - 1),
                Some(w) => format!("{} - 1", emit_expr(w)),
                None => format!("{}", crate::enum_bits(members.len())),
            };
            let members_s: Vec<String> = members
                .iter()
                .map(|m| match &m.value {
                    Some(v) => format!("{} = {}", m.name, emit_expr(v)),
                    None => m.name.clone(),
                })
                .collect();
            let joined = members_s.join(", ");
            line(
                out,
                indent,
                &format!("typedef enum logic [{w}:0] {{ {joined} }} {name};"),
            );
        }
    }
}

/// Item constraint SV (F12): ekspresi `e;`, `if (c) { ... }`,
/// `solve x before a, b;` — emisi 1:1 dengan sintaks .mv.
pub(crate) fn emit_constraint_items(out: &mut String, indent: usize, items: &[ConstraintItem]) {
    for item in items {
        match item {
            ConstraintItem::Expr(e) => {
                line(out, indent, &format!("{};", emit_expr(e)));
            }
            // Constraint `if` SV memakai `{ }`, BUKAN begin/end (sintaks
            // constraint block, bukan procedural block).
            ConstraintItem::If { cond, then, els } => {
                line(out, indent, &format!("if ({}) {{", emit_expr(cond)));
                emit_constraint_items(out, indent + 1, then);
                if els.is_empty() {
                    line(out, indent, "}");
                } else {
                    line(out, indent, "} else {");
                    emit_constraint_items(out, indent + 1, els);
                    line(out, indent, "}");
                }
            }
            ConstraintItem::Solve { var, before, .. } => {
                line(
                    out,
                    indent,
                    &format!("solve {var} before {};", before.join(", ")),
                );
            }
        }
    }
}

/// Emit class SV: `class Name extends Base; ... endclass`.
pub(crate) fn emit_class(out: &mut String, c: &MClass) {
    let ext = c
        .extends
        .as_ref()
        .map(|b| format!(" extends {b}"))
        .unwrap_or_default();
    line(out, 0, &format!("class {}{};", c.name, ext));

    for (name, ty, rand) in &c.fields {
        let r = if *rand { "rand " } else { "" };
        // `emit_signal_decl` (bukan `emit_type`): dimensi unpacked SV
        // harus setelah nama field (`logic [7:0] fa [0:3]`), LRM 1800 §7.3.
        line(
            out,
            1,
            &format!("{r}{};", super::emit_signal_decl(ty, name)),
        );
    }

    for (cname, items) in &c.constraints {
        line(out, 0, "");
        line(out, 1, &format!("constraint {cname} {{"));
        emit_constraint_items(out, 2, items);
        line(out, 1, "}");
    }

    for f in &c.funcs {
        line(out, 0, "");
        let ret = if f.name == "new" {
            String::new()
        } else {
            format!(
                "{} ",
                f.ret
                    .as_ref()
                    .map(emit_type)
                    .unwrap_or_else(|| "void".into())
            )
        };
        let args = emit_args(&f.args, false);
        line(
            out,
            1,
            &format!("function {ret}{}({});", f.name, args.join(", ")),
        );
        for s in &f.body {
            super::stmt::emit_stmt(out, 2, s);
        }
        line(out, 1, "endfunction");
    }
    for t in &c.tasks {
        line(out, 0, "");
        let args = emit_args(&t.args, true);
        line(out, 1, &format!("task {}({});", t.name, args.join(", ")));
        for s in &t.body {
            super::stmt::emit_stmt(out, 2, s);
        }
        line(out, 1, "endtask");
    }
    line(out, 0, "endclass");
}
