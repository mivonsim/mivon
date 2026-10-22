//! Latch detection helpers for `mlint`.

use mivon_ast::stmt::Stmt;

use super::Finding;

pub(crate) fn find_incomplete_if(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::IfElse { false_branch, .. } if false_branch.is_none() => {
                out.push(Finding {
                    module: scope.to_string(),
                    check: "latch",
                    severity: "W",
                    message: "if tanpa else di blok kombinasional → potensi latch".to_string(),
                });
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                find_incomplete_if(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_incomplete_if(scope, std::slice::from_ref(fb), out);
                }
            }
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_incomplete_if(scope, stmts, out);
            }
            _ => {}
        }
    }
}