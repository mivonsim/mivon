//! Gate-level optimization helpers for `mlint`.

use std::collections::HashMap as StdHashMap;

use mivon_ast::stmt::Stmt;
use mivon_ast::types::ModuleItem;
use mivon_core::intern::Symbol;

use super::Finding;

pub(crate) fn find_gate_opt_patterns(scope: &str, items: &[ModuleItem], out: &mut Vec<Finding>) {
    for item in items {
        if let ModuleItem::Always(ab) = item {
            find_redundant_assigns(scope, &ab.stmts, out);
        }
    }
}

pub(crate) fn find_redundant_assigns(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    let mut writes: StdHashMap<Symbol, usize> = StdHashMap::new();
    for stmt in stmts {
        match stmt {
            Stmt::BlockingAssign {
                lhs: mivon_ast::expr::Expr::Ident { name, .. },
                ..
            } => {
                let count = writes.entry(*name).or_insert(0);
                *count += 1;
                if *count == 2 {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "gate-opt-redundant",
                        severity: "W",
                        message: format!(
                            "signal '{}' overwritten in same block (redundant assignment)",
                            name.as_str()
                        ),
                    });
                }
            }
            Stmt::NonBlockingAssign {
                lhs: mivon_ast::expr::Expr::Ident { name, .. },
                ..
            } => {
                let count = writes.entry(*name).or_insert(0);
                *count += 1;
                if *count == 2 {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "gate-opt-redundant",
                        severity: "W",
                        message: format!(
                            "signal '{}' overwritten in same block (redundant NBA)",
                            name.as_str()
                        ),
                    });
                }
            }
            Stmt::Block { stmts: inner } => {
                find_redundant_assigns(scope, inner, out);
            }
            _ => {}
        }
    }
}