//! Generate-walk helpers for `mlint`.

use std::collections::{HashMap, HashSet};

use mivon_ast::types::ModuleItem;
use mivon_core::intern::Symbol;

use super::scan;
use super::width;

pub(crate) fn walk_generate<'a>(
    gi: &'a mivon_ast::types::GenerateItem,
    reads: &mut HashSet<Symbol>,
    writes: &mut HashSet<Symbol>,
    always_blocks: &mut Vec<&'a mivon_ast::stmt::AlwaysBlock>,
    do_width: bool,
    declared: &HashMap<Symbol, usize>,
    out: &mut Vec<super::Finding>,
) {
    use mivon_ast::types::GenerateItem;
    match gi {
        GenerateItem::Items(items) => {
            walk_items(items, reads, writes, always_blocks, do_width, declared, out)
        }
        GenerateItem::If {
            true_items,
            false_items,
            ..
        } => {
            walk_items(
                true_items,
                reads,
                writes,
                always_blocks,
                do_width,
                declared,
                out,
            );
            walk_items(
                false_items,
                reads,
                writes,
                always_blocks,
                do_width,
                declared,
                out,
            );
        }
        GenerateItem::For { body_items, .. } => {
            walk_items(
                body_items,
                reads,
                writes,
                always_blocks,
                do_width,
                declared,
                out,
            );
        }
        GenerateItem::Case { items, default, .. } => {
            for ci in items {
                walk_items(
                    &ci.body,
                    reads,
                    writes,
                    always_blocks,
                    do_width,
                    declared,
                    out,
                );
            }
            if let Some(d) = default {
                walk_items(d, reads, writes, always_blocks, do_width, declared, out);
            }
        }
    }
}

pub(crate) fn walk_items<'a>(
    items: &'a [ModuleItem],
    reads: &mut HashSet<Symbol>,
    writes: &mut HashSet<Symbol>,
    always_blocks: &mut Vec<&'a mivon_ast::stmt::AlwaysBlock>,
    do_width: bool,
    declared: &HashMap<Symbol, usize>,
    out: &mut Vec<super::Finding>,
) {
    for item in items {
        match item {
            ModuleItem::Always(b) => always_blocks.push(b),
            ModuleItem::Assign(ca) => {
                scan::scan_expr_reads(&ca.lhs, reads, writes);
                scan::scan_expr_reads(&ca.rhs, reads, writes);
                if let Some(root) = scan::lvalue_root(&ca.lhs) {
                    writes.insert(root);
                }
                if do_width {
                    width::check_width("generate", &ca.lhs, &ca.rhs, declared, out);
                }
            }
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    walk_generate(gi, reads, writes, always_blocks, do_width, declared, out);
                }
            }
            _ => {}
        }
    }
}
