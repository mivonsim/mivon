//! Clock gating helpers for `mlint`.

use mivon_ast::stmt::Stmt;

use super::Finding;

pub(crate) fn find_clock_gating(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                if false_branch.is_none() {
                    if has_nonblocking(std::slice::from_ref(true_branch)) {
                        out.push(Finding {
                            module: scope.to_string(),
                            check: "clock_gating",
                            severity: "W",
                            message:
                                "always_ff dengan if tanpa else: potensi clock gating implisit"
                                    .to_string(),
                        });
                    }
                }
                find_clock_gating(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_clock_gating(scope, std::slice::from_ref(fb), out);
                }
            }
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_clock_gating(scope, stmts, out);
            }
            _ => {}
        }
    }
}

pub(crate) fn has_nonblocking(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::NonBlockingAssign { .. } => return true,
            Stmt::Block { stmts } | Stmt::NamedBlock { stmts, .. } => {
                if has_nonblocking(stmts) {
                    return true;
                }
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                if has_nonblocking(std::slice::from_ref(true_branch)) {
                    return true;
                }
                if let Some(fb) = false_branch {
                    if has_nonblocking(std::slice::from_ref(fb)) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}