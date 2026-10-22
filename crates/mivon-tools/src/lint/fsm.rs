//! FSM detection helpers for `mlint`.

use mivon_ast::expr::Expr;
use mivon_ast::stmt::Stmt;

use super::Finding;

pub(crate) fn find_fsm(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::Case { expr, .. }
            | Stmt::CaseX { expr, .. }
            | Stmt::CaseZ { expr, .. }
            | Stmt::UniqueCase { expr, .. }
            | Stmt::PriorityCase { expr, .. }
            | Stmt::Unique0Case { expr, .. }
            | Stmt::CaseInside { expr, .. } => {
                if let Expr::Ident { name, .. } = expr {
                    if name.as_str().contains("state") || name.as_str().contains("fsm") {
                        out.push(Finding {
                            module: scope.to_string(),
                            check: "fsm",
                            severity: "I",
                            message: format!(
                                "deteksi FSM: register state '{}' ({} case item)",
                                name.as_str(),
                                case_item_count(stmt)
                            ),
                        });
                    }
                }
            }
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_fsm(scope, stmts, out);
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                find_fsm(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_fsm(scope, std::slice::from_ref(fb), out);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn case_item_count(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::Case { items, .. }
        | Stmt::CaseX { items, .. }
        | Stmt::CaseZ { items, .. }
        | Stmt::UniqueCase { items, .. }
        | Stmt::PriorityCase { items, .. }
        | Stmt::Unique0Case { items, .. }
        | Stmt::CaseInside { items, .. } => items.len(),
        _ => 0,
    }
}