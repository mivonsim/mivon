//! Memory inference helpers for `mlint`.

use mivon_ast::types::ModuleItem;

use super::Finding;

pub(crate) fn find_memory_inference(
    scope: &str,
    decls: &[mivon_ast::types::Decl],
    _items: &[ModuleItem],
    out: &mut Vec<Finding>,
) {
    for d in decls {
        for v in &d.names {
            if v.range.is_some() && v.array_range.is_some() {
                out.push(Finding {
                    module: scope.to_string(),
                    check: "memory",
                    severity: "I",
                    message: format!("reg '{}' array — kandidat inferred RAM/ROM", v.name.as_str()),
                });
            }
        }
    }
}