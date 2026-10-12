//! Instance graph + auto-top scoring — extract dari mod.rs untuk
//! Arsitektur-06 (elaborator monolithic → submodule).
//!
//! Graf instansiasi module-level dan heuristik pemilihan top otomatis
//! (cone transitif + sinyal nama). Murni (tanpa Elaborator); dipakai
//! `elaborate` saat top tidak dispesifikasikan.

use std::collections::{HashMap, HashSet};

use mivon_ast::*;
use mivon_core::intern::Symbol;

/// Kumpulkan nama module yang di-instansiasi sebuah module item, termasuk
/// instance di dalam blok generate (recursive: If/For/Case/Items).
fn collect_inst_names(item: &ModuleItem, out: &mut Vec<Symbol>) {
    match item {
        ModuleItem::Instance(inst) => out.push(inst.module_name),
        ModuleItem::Generate(GenerateBlock { items }) => {
            for gi in items {
                match gi {
                    GenerateItem::If {
                        true_items,
                        false_items,
                        ..
                    } => {
                        for it in true_items.iter().chain(false_items.iter()) {
                            collect_inst_names(it, out);
                        }
                    }
                    GenerateItem::For { body_items, .. } => {
                        for it in body_items {
                            collect_inst_names(it, out);
                        }
                    }
                    GenerateItem::Case { items, default, .. } => {
                        for ci in items {
                            for m in &ci.body {
                                collect_inst_names(m, out);
                            }
                        }
                        if let Some(mis) = default {
                            for m in mis {
                                collect_inst_names(m, out);
                            }
                        }
                    }
                    GenerateItem::Items(mis) => {
                        for m in mis {
                            collect_inst_names(m, out);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Graf instansiasi module-level: nama module → nama module yang di-instansiasi
/// (direct + generate). Dipakai auto top resolution (cone size) dan penentuan
/// candidate top (module yang TIDAK di-instansiasi siapa pun).
pub(crate) fn build_inst_graph(design: &Design) -> HashMap<Symbol, Vec<Symbol>> {
    let mut graph: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
    for m in &design.modules {
        let mut deps: Vec<Symbol> = Vec::new();
        for item in &m.items {
            collect_inst_names(item, &mut deps);
        }
        graph.insert(m.name, deps);
    }
    graph
}

/// Ukuran cone (BFS) sebuah module candidate: banyaknya modul yang di-reach
/// secara transitif. SoC chip top (menginstansiasi ratusan modul) jauh lebih
/// besar daripada testbench/bind/assertion kecil — diskriminator utama auto
/// top resolution.
pub(crate) fn module_cone_size(start: Symbol, graph: &HashMap<Symbol, Vec<Symbol>>) -> usize {
    let mut seen: HashSet<Symbol> = HashSet::new();
    let mut stack: Vec<Symbol> = vec![start];
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        if let Some(deps) = graph.get(&n) {
            stack.extend(deps.iter().copied());
        }
    }
    seen.len()
}

/// Skor kandidat top untuk auto resolution: ukuran cone (transitive module
/// count) sebagai bobot DOMINAN (×20) + sinyal nama sebagai tie-breaker.
/// Preferensi simulasi (`verilator`/`_sim`/`tb`) dan `chip`; penalti wrapper
/// teknologi khusus (`_asic`/`_cw`/`fpga`) dan assertion bind
/// (`_bind`/`_fpv`/`_sva`/`_assert`) yang bukan top fungsional. Bobot cone
/// sengaja jauh di atas bonus nama — SoC chip (cone 500+) selalu menang atas
/// testbench kecil walau tb diberi bonus nama.
pub(crate) fn score_auto_top(name: Symbol, cone: usize) -> i64 {
    let lower = name.as_str().to_ascii_lowercase();
    let mut s = (cone as i64) * 20;
    if lower.contains("verilator")
        || lower.contains("_sim")
        || lower.starts_with("tb")
        || lower.contains("_tb")
    {
        s += 100;
    }
    if lower.contains("chip") {
        s += 100;
    }
    if lower.contains("_asic") || lower.contains("_cw") || lower.contains("fpga") {
        s -= 300;
    }
    if lower.contains("_bind")
        || lower.contains("_fpv")
        || lower.contains("_sva")
        || lower.contains("_assert")
        || lower.contains("_sca_wrapper")
    {
        s -= 500;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(s: &str) -> Symbol {
        Symbol::intern(s)
    }

    #[test]
    fn test_cone_size_transitive_and_cycle() {
        let mut g: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
        g.insert(sym("a"), vec![sym("b")]);
        g.insert(sym("b"), vec![sym("c")]);
        g.insert(sym("c"), vec![]);
        assert_eq!(module_cone_size(sym("a"), &g), 3);
        assert_eq!(module_cone_size(sym("c"), &g), 1);
        // Siklus tidak hang, tiap node sekali.
        let mut cyc: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
        cyc.insert(sym("a"), vec![sym("b")]);
        cyc.insert(sym("b"), vec![sym("a")]);
        assert_eq!(module_cone_size(sym("a"), &cyc), 2);
    }

    #[test]
    fn test_score_cone_dominates_and_penalties() {
        // Cone dominan atas bonus nama tb.
        assert!(score_auto_top(sym("chip"), 500) > score_auto_top(sym("tb"), 10));
        // Penalti bind di bawah polos pada cone sama.
        assert!(score_auto_top(sym("x_bind"), 100) < score_auto_top(sym("plain"), 100));
    }
}
