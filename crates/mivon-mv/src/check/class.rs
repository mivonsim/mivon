//! Check — function/task/class/constraint. 1 file = 1 tanggung jawab.

use super::{err_at, expr::check_expr, new_scope, BlockKind, Ctx, Scope};
use crate::ast::*;
use crate::MvError;
use std::collections::HashSet;

pub(crate) fn check_func<'a>(
    f: &'a MFunc,
    ctx: &'a Ctx<'a>,
    base: &mut Scope<'a>,
) -> Result<(), MvError> {
    let mut scope = base.clone();
    for (n, t, _, default) in &f.args {
        super::expr::check_type_scope(t, ctx, Some(&scope), 0)?;
        if let Some(d) = default {
            check_expr(d, ctx, &scope, 0)?;
        }
        scope.sigs.insert(n.as_str());
        scope.types.insert(n.as_str(), t);
    }
    if let Some(ret) = &f.ret {
        super::expr::check_type_scope(ret, ctx, Some(&scope), 0)?;
    }
    for s in &f.body {
        check_expr_stmt_ctx(s, ctx, &mut scope)?;
    }
    Ok(())
}

pub(crate) fn check_task<'a>(
    t: &'a MTask,
    ctx: &'a Ctx<'a>,
    base: &mut Scope<'a>,
) -> Result<(), MvError> {
    let mut scope = base.clone();
    scope.in_task = true;
    for (n, ty, _, default) in &t.args {
        super::expr::check_type_scope(ty, ctx, Some(&scope), 0)?;
        if let Some(d) = default {
            check_expr(d, ctx, &scope, 0)?;
        }
        scope.sigs.insert(n.as_str());
        scope.types.insert(n.as_str(), ty);
    }
    for s in &t.body {
        check_expr_stmt_ctx(s, ctx, &mut scope)?;
    }
    Ok(())
}

fn check_expr_stmt_ctx<'a>(
    s: &'a Stmt,
    ctx: &'a Ctx<'a>,
    scope: &mut Scope<'a>,
) -> Result<(), MvError> {
    super::stmt::check_stmt(s, ctx, scope, BlockKind::Always)
}

/// Class (MIVON-HDL.md §8): fields terlihat di scope method/constraint;
/// `this`/`super`/`rand` adalah kata kunci konteks — bukan sinyal.
pub(crate) fn check_class<'a>(c: &'a MClass, ctx: &'a Ctx<'a>) -> Result<(), MvError> {
    let mut scope = new_scope(ctx, &c.name);
    scope.in_class = true;

    // ── field: duplikat (E2007) + tipe (E2005) ──
    let mut fseen = HashSet::new();
    for (n, ty, _) in &c.fields {
        if !fseen.insert(n.as_str()) {
            return Err(err_at(
                c.line,
                c.col,
                "E2007",
                format!("field '{n}' dideklarasikan dua kali di class '{}'", c.name),
            ));
        }
        super::expr::check_type(ty, ctx, 0)?;
        scope.sigs.insert(n.as_str());
        scope.types.insert(n.as_str(), ty);
    }

    // ── method: duplikat nama (E2007) ──
    let mut mseen = HashSet::new();
    for f in &c.funcs {
        if !mseen.insert(f.name.as_str()) {
            return Err(err_at(
                f.line,
                f.col,
                "E2007",
                format!(
                    "method '{}' dideklarasikan dua kali di class '{}'",
                    f.name, c.name
                ),
            ));
        }
    }
    for t in &c.tasks {
        if !mseen.insert(t.name.as_str()) {
            return Err(err_at(
                t.line,
                t.col,
                "E2007",
                format!(
                    "method '{}' dideklarasikan dua kali di class '{}'",
                    t.name, c.name
                ),
            ));
        }
    }

    // ── constraint: duplikat (E2007) + item divalidasi (F12: if/solve/expr) ──
    let mut cseen = HashSet::new();
    for (cname, items) in &c.constraints {
        if !cseen.insert(cname.as_str()) {
            return Err(err_at(
                c.line,
                c.col,
                "E2007",
                format!(
                    "constraint '{cname}' dideklarasikan dua kali di class '{}'",
                    c.name
                ),
            ));
        }
        check_constraint_items(items, ctx, &scope)?;
    }

    for f in &c.funcs {
        check_func(f, ctx, &mut scope)?;
    }
    for t in &c.tasks {
        check_task(t, ctx, &mut scope)?;
    }
    Ok(())
}

// ── Constraint items (F12) ──

/// Validasi item constraint: ekspresi (termasuk inside/dist), if/else
/// (rekursif ke cabang), dan `solve var before a, b` (var harus dikenal).
pub(crate) fn check_constraint_items<'a>(
    items: &'a [ConstraintItem],
    ctx: &'a Ctx<'a>,
    scope: &Scope<'a>,
) -> Result<(), MvError> {
    for item in items {
        match item {
            ConstraintItem::Expr(e) => check_expr(e, ctx, scope, 0)?,
            ConstraintItem::If { cond, then, els } => {
                check_expr(cond, ctx, scope, 0)?;
                check_constraint_items(then, ctx, scope)?;
                check_constraint_items(els, ctx, scope)?;
            }
            ConstraintItem::Solve {
                var,
                before,
                line,
                col,
            } => {
                if !scope.known(var) {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2001",
                        format!(
                            "undefined signal '{var}' (solve) — di '{}'",
                            scope.env.mname
                        ),
                    ));
                }
                for b in before {
                    if !scope.known(b) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2001",
                            format!(
                                "undefined signal '{b}' (solve before) — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
