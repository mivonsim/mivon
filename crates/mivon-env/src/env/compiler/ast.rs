use mivon_ast::Design;

/// Gabungkan beberapa Design (per-file) menjadi satu dengan MOVING elemen
/// (O(1) per field, tanpa clone). Design pertama menjadi target.
pub fn merge_designs(mut target: Design, other: &mut Design) -> Design {
    target.modules.append(&mut other.modules);
    target.packages.append(&mut other.packages);
    target.interfaces.append(&mut other.interfaces);
    target.classes.append(&mut other.classes);
    target.binds.append(&mut other.binds);
    target.clocking_blocks.append(&mut other.clocking_blocks);
    target.configs.append(&mut other.configs);
    target.udp_defs.append(&mut other.udp_defs);
    target.unit_imports.append(&mut other.unit_imports);
    target.unit_funcs.append(&mut other.unit_funcs);
    target.unit_tasks.append(&mut other.unit_tasks);
    target.unit_typedefs.append(&mut other.unit_typedefs);
    target.unit_params.append(&mut other.unit_params);
    target.unit_decls.append(&mut other.unit_decls);
    target.unit_items.append(&mut other.unit_items);
    target
}

/// Gabungkan batch Design menjadi satu (empty = None).
pub fn merge_all(designs: &mut [Design]) -> Option<Design> {
    if designs.is_empty() {
        return None;
    }
    let mut merged = std::mem::take(&mut designs[0]);
    for d in &mut designs[1..] {
        merged = merge_designs(merged, d);
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::compiler::{lex, parse_strict};

    #[test]
    fn test_merge_all() {
        let d1 = parse_strict(lex("module a; endmodule"), "a.sv").unwrap();
        let d2 = parse_strict(lex("module b; endmodule"), "b.sv").unwrap();
        let mut v = vec![d1, d2];
        let merged = merge_all(&mut v).unwrap();
        assert_eq!(merged.modules.len(), 2);
    }

    #[test]
    fn test_merge_all_empty() {
        let mut v = Vec::new();
        assert!(merge_all(&mut v).is_none());
    }
}
