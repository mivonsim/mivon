//! Check — statement: assignment rules (E2002/03/04), kontrol flow, loop
//! depth, fork, assert, delay/event. 1 file = 1 tanggung jawab.

use super::{err_at, expr::check_expr, BlockKind, Ctx, Scope};
use crate::ast::*;
use crate::MvError;

pub(crate) fn check_stmt<'a>(
    stmt: &'a Stmt,
    ctx: &'a Ctx<'a>,
    scope: &mut Scope<'a>,
    kind: BlockKind,
) -> Result<(), MvError> {
    match stmt {
        Stmt::Block(stmts) => {
            for s in stmts {
                check_stmt(s, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::Assign {
            lhs,
            rhs,
            nba,
            line,
            col,
        } => {
            // E2004: operator assignment di konteks salah
            if kind == BlockKind::Seq && !*nba {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "blocking assign '=' tidak boleh di dalam seq (pakai '<=') — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if kind != BlockKind::Seq && *nba {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "non-blocking assign '<=' hanya boleh di dalam seq — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            // E2003: drive port input hanya diizinkan di body initial/final.
            if kind != BlockKind::Tb {
                if let Some(base) = base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            check_expr(rhs, ctx, scope, 0)?;
            // E2002: RHS lebih lebar dari LHS → truncation
            let wl = super::expr::expr_width(lhs, ctx, scope, 0);
            let wr = super::expr::expr_width(rhs, ctx, scope, 0);
            if let (Some(l), Some(r)) = (wl, wr) {
                if r > l {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2002",
                        format!(
                            "lebar {r} bit ke sinyal {l}-bit '{}' — di '{}'",
                            describe_lhs(lhs),
                            scope.env.mname
                        ),
                    ));
                }
            }
            Ok(())
        }
        // F36: `lhs += rhs` — compound assignment (blocking, seperti `=`).
        Stmt::CompoundAssign {
            lhs,
            op,
            rhs,
            line,
            col,
        } => {
            if kind == BlockKind::Seq {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "compound assign '{op}' tidak boleh di dalam seq (pakai '<=') — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if kind != BlockKind::Tb {
                if let Some(base) = base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            check_expr(rhs, ctx, scope, 0)?;
            let wl = super::expr::expr_width(lhs, ctx, scope, 0);
            let wr = super::expr::expr_width(rhs, ctx, scope, 0);
            if let (Some(l), Some(r)) = (wl, wr) {
                if r > l {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2002",
                        format!(
                            "lebar {r} bit ke sinyal {l}-bit '{}' — di '{}'",
                            describe_lhs(lhs),
                            scope.env.mname
                        ),
                    ));
                }
            }
            Ok(())
        }
        // F36: `lhs++` / `lhs--` — increment (blocking, seperti `=`).
        Stmt::IncDec { lhs, line, col, .. } => {
            if kind == BlockKind::Seq {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "increment/decrement tidak boleh di dalam seq (pakai '<=') — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if kind != BlockKind::Tb {
                if let Some(base) = base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            Ok(())
        }
        Stmt::If { cond, then, els } => {
            check_expr(cond, ctx, scope, 0)?;
            check_stmt(then, ctx, scope, kind)?;
            if let Some(e) = els {
                check_stmt(e, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::Case {
            expr,
            items,
            default,
            qual: _,
            kind: _,
        } => {
            check_expr(expr, ctx, scope, 0)?;
            for (vals, body) in items {
                for v in vals {
                    check_expr(v, ctx, scope, 0)?;
                }
                check_stmt(body, ctx, scope, kind)?;
            }
            if let Some(d) = default {
                check_stmt(d, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::For {
            var,
            from,
            to,
            step,
            body,
        } => {
            check_expr(from, ctx, scope, 0)?;
            check_expr(to, ctx, scope, 0)?;
            if let Some(s) = step {
                check_expr(s, ctx, scope, 0)?;
            }
            let mut inner = scope.clone();
            inner.sigs.insert(var.as_str());
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::While { cond, body } => {
            check_expr(cond, ctx, scope, 0)?;
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        // F38: `do { body } while (cond)` — post-test.
        Stmt::DoWhile { cond, body } => {
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)?;
            check_expr(cond, ctx, scope, 0)
        }
        // F38: event trigger `->ev` — target harus signal/event yang dikenal.
        Stmt::EventTrigger(ev) => check_expr(ev, ctx, scope, 0),
        // F39: fork/join — tiap branch divalidasi di scope yang sama.
        Stmt::Fork { branches, .. } => {
            for b in branches {
                check_stmt(b, ctx, scope, kind)?;
            }
            Ok(())
        }
        // `foreach (arr[i]) { ... }` — arr harus sinyal dikenal; index var
        // otomatis lokal di dalam body.
        Stmt::Foreach { arr, inds, body } => {
            if !scope.known(arr) {
                return Err(err_at(
                    0,
                    0,
                    "E2001",
                    format!(
                        "undefined signal '{arr}' (foreach) — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            let mut inner = scope.clone();
            for iv in inds {
                inner.sigs.insert(iv.as_str());
            }
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::Repeat { count, body } => {
            check_expr(count, ctx, scope, 0)?;
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::Forever(body) => {
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::Wait { cond, body } => {
            check_expr(cond, ctx, scope, 0)?;
            check_stmt(body, ctx, scope, kind)
        }
        // F45: `wait fork;` — tanpa operand, selalu valid.
        Stmt::WaitFork => Ok(()),
        // F45: `disable fork;` / `disable <label>;` — konservatif: label
        // blok tidak dilacak checker (seperti assert property), selalu lolos.
        Stmt::Disable { .. } => Ok(()),
        Stmt::Event { expr, body } => {
            check_expr(expr, ctx, scope, 0)?;
            if let Some(b) = body {
                check_stmt(b, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::Delay { amt, body } => {
            check_expr(amt, ctx, scope, 0)?;
            check_stmt(body, ctx, scope, kind)
        }
        Stmt::ExprStmt(e) => check_expr(e, ctx, scope, 0),
        Stmt::VarDecl { names, ty, init } => {
            super::expr::check_type_scope(ty, ctx, Some(scope), 0)?;
            if let Some(i) = init {
                check_expr(i, ctx, scope, 0)?;
            }
            for n in names {
                if scope.sigs.contains(n.as_str()) {
                    return Err(MvError::new(
                        0,
                        0,
                        format!("E2007: variabel '{}' sudah dideklarasikan", n),
                    ));
                }
                scope.sigs.insert(n.as_str());
                scope.types.insert(n.as_str(), ty);
            }
            Ok(())
        }
        Stmt::Return(v) => {
            if let Some(v) = v {
                if scope.in_task {
                    return Err(MvError::new(
                        0,
                        0,
                        "E2008: task tidak boleh mengembalikan nilai (return expr)".to_string(),
                    ));
                }
                check_expr(v, ctx, scope, 0)?;
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => {
            if scope.loop_depth == 0 {
                return Err(MvError::new(
                    0,
                    0,
                    "E2009: break/continue hanya boleh di dalam loop".to_string(),
                ));
            }
            Ok(())
        }
        Stmt::Assert { cond, pass, fail } => {
            check_expr(cond, ctx, scope, 0)?;
            if let Some(p) = pass {
                check_stmt(p, ctx, scope, kind)?;
            }
            if let Some(f) = fail {
                check_stmt(f, ctx, scope, kind)?;
            }
            Ok(())
        }
        // `assert property (...)` — body RAW, konservatif: isi tidak dianalisis.
        Stmt::AssertProperty(_) => Ok(()),
        // Escape hatch `@sv { ... }` — teks SV mentah, ditangani lexer/codegen;
        // check tidak menganalisis isi (konservatif, seperti assert property).
        Stmt::RawSvh(_) => Ok(()),
    }
}

pub(crate) fn base_ident(e: &Expr) -> Option<&str> {
    match e {
        Expr::Ident(s, ..) => Some(s),
        Expr::Member(o, ..) => base_ident(o),
        Expr::Index(o, _) => base_ident(o),
        Expr::Range(o, _, _) => base_ident(o),
        _ => None,
    }
}

pub(crate) fn describe_lhs(e: &Expr) -> String {
    match e {
        Expr::Ident(s, ..) => s.clone(),
        Expr::Member(o, f, ..) => format!("{}.{}", describe_lhs(o), f),
        Expr::Index(o, i) => format!("{}[{}]", describe_lhs(o), describe_lhs(i)),
        Expr::Range(o, a, b) => format!(
            "{}[{}:{}]",
            describe_lhs(o),
            describe_lhs(a),
            describe_lhs(b)
        ),
        other => format!("{other:?}"),
    }
}
