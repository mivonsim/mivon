//! Case analysis helpers for `mlint`.

use mivon_ast::stmt::Stmt;

use super::Finding;

pub(crate) fn find_case_analysis(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::Case { items, default, .. }
            | Stmt::CaseX { items, default, .. }
            | Stmt::CaseZ { items, default, .. } => {
                if default.is_none() && !items.is_empty() {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "case",
                        severity: "W",
                        message: format!(
                            "case tanpa 'default' ({} item) → potensi latch atau nilai tak terdefinisi",
                            items.len()
                        ),
                    });
                }
            }
            Stmt::UniqueCase { items, default, .. } => {
                if default.is_none() && !items.is_empty() {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "case_unique",
                        severity: "W",
                        message: format!(
                            "unique case tanpa 'default' ({} item) → bila tidak ada match, nilai tak terdefinisi",
                            items.len()
                        ),
                    });
                }
            }
            Stmt::PriorityCase { items, default, .. } => {
                if default.is_none() && !items.is_empty() {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "case_priority",
                        severity: "W",
                        message: format!(
                            "priority case tanpa 'default' ({} item) → bila tidak ada match, nilai tak terdefinisi",
                            items.len()
                        ),
                    });
                }
            }
            Stmt::Unique0Case { .. } => {}
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_case_analysis(scope, stmts, out);
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                find_case_analysis(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_case_analysis(scope, std::slice::from_ref(fb), out);
                }
            }
            _ => {}
        }
    }
}