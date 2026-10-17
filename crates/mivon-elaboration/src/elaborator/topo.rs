//! Instance graph + auto-top scoring + checksum/topo-order — extract
//! dari mod.rs untuk Arsitektur-06 (elaborator monolithic → submodule).
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

/// Kumpulkan nama module yang di-instansiasi dari daftar ModuleItem,
/// MENURUNI blok generate — pindah dari mod.rs (Arsitektur-06).
/// (Beda bentuk AST dari `collect_inst_names`: `ModuleItem::Generate`
/// membawa `gen.items` langsung.) Dipakai reachability analysis dari top.
pub(crate) fn collect_instance_names(items: &[ModuleItem], out: &mut Vec<Symbol>) {
    for item in items {
        match item {
            ModuleItem::Instance(inst) => out.push(inst.module_name),
            ModuleItem::Generate(gen) => {
                for gi in &gen.items {
                    match gi {
                        GenerateItem::If {
                            true_items,
                            false_items,
                            ..
                        } => {
                            collect_instance_names(true_items, out);
                            collect_instance_names(false_items, out);
                        }
                        GenerateItem::For { body_items, .. } => {
                            collect_instance_names(body_items, out);
                        }
                        GenerateItem::Case { items, default, .. } => {
                            for ci in items {
                                collect_instance_names(&ci.body, out);
                            }
                            if let Some(d) = default {
                                collect_instance_names(d, out);
                            }
                        }
                        GenerateItem::Items(items) => {
                            collect_instance_names(items, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
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

/// Checksum struktural module (CRIT-016): hash field struktural
/// (bukan Debug seluruh AST) — nama + port/param + items. Dipakai cache
/// elaborasi incremental + dependency-aware signature.
pub(crate) fn compute_module_checksum(module: &Module) -> u64 {
    use mivon_core::checksum::{combine_checksum, compute_checksum, compute_str_checksum};

    // Hash structural fields instead of Debug-formatting the entire AST
    let mut h = compute_str_checksum(module.name.as_str());
    h = combine_checksum(
        h,
        compute_checksum(&(module.ports.len() as u64).to_le_bytes()),
    );
    for port in &module.ports {
        h = combine_checksum(h, compute_str_checksum(port.name.as_str()));
        h = combine_checksum(h, compute_checksum(&[(port.direction as u8)]));
    }
    for param in &module.params {
        h = combine_checksum(h, compute_str_checksum(param.name.as_str()));
    }
    for item in &module.items {
        match item {
            ModuleItem::Instance(inst) => {
                h = combine_checksum(h, compute_str_checksum(inst.module_name.as_str()));
                h = combine_checksum(
                    h,
                    compute_checksum(&(inst.port_conns.len() as u64).to_le_bytes()),
                );
            }
            ModuleItem::Param(p) => {
                h = combine_checksum(h, compute_str_checksum(p.name.as_str()));
            }
            ModuleItem::Decl(d) => {
                for v in &d.names {
                    h = combine_checksum(h, compute_str_checksum(v.name.as_str()));
                }
            }
            ModuleItem::Func(f) => {
                h = combine_checksum(h, compute_str_checksum(f.name.as_str()));
            }
            ModuleItem::Generate(g) => {
                h = combine_checksum(h, compute_checksum(&(g.items.len() as u64).to_le_bytes()));
            }
            ModuleItem::Import { package, item } => {
                h = combine_checksum(h, compute_str_checksum(package.as_str()));
                h = combine_checksum(h, compute_str_checksum(item.as_str()));
            }
            _ => {}
        }
    }
    h
}

/// Compute topological order of modules so children (instantiated) come before parents.
/// Uses Kahn's algorithm.
pub(crate) fn compute_topo_order(modules: &[Module], module_names: &[Symbol]) -> Vec<Symbol> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let name_set: HashSet<Symbol> = module_names.iter().copied().collect();
    let mut in_degree: HashMap<Symbol, usize> = HashMap::new();
    let mut dependents: HashMap<Symbol, Vec<Symbol>> = HashMap::new(); // child → [parents]

    for module in modules {
        in_degree.entry(module.name).or_insert(0);
        for item in &module.items {
            if let ModuleItem::Instance(inst) = item {
                if name_set.contains(&inst.module_name) {
                    // parent depends on child (child must be elaborated first)
                    dependents
                        .entry(inst.module_name)
                        .or_default()
                        .push(module.name);
                    *in_degree.entry(module.name).or_insert(0) += 1;
                }
            }
        }
    }

    // Kahn: start with modules that have no dependencies (in_degree == 0)
    let mut queue: VecDeque<Symbol> = VecDeque::new();
    for (&name, &deg) in &in_degree {
        if deg == 0 {
            queue.push_back(name);
        }
    }

    let mut order = Vec::new();
    let mut order_set: HashSet<Symbol> = HashSet::new();
    while let Some(name) = queue.pop_front() {
        order.push(name);
        order_set.insert(name);
        if let Some(children) = dependents.get(&name) {
            for &parent in children {
                if let Some(deg) = in_degree.get_mut(&parent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(parent);
                    }
                }
            }
        }
    }

    // Handle cycles: add remaining modules not in order
    for module in modules {
        if !order_set.contains(&module.name) {
            order.push(module.name);
            order_set.insert(module.name);
        }
    }

    order
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

    fn test_module(name: &str, insts: &[&str]) -> Module {
        use std::collections::HashMap;
        Module {
            name: sym(name),
            ports: vec![],
            params: vec![],
            decls: vec![],
            items: insts
                .iter()
                .map(|m| {
                    ModuleItem::Instance(ModuleInstance {
                        module_name: sym(m),
                        instance_name: sym("u"),
                        range: None,
                        param_assigns: HashMap::new(),
                        type_param_assigns: HashMap::new(),
                        port_conns: vec![],
                        line: 0,
                        col: 0,
                        attrs: vec![],
                    })
                })
                .collect(),
            timescale: None,
        }
    }

    #[test]
    fn test_topo_order_children_first_and_cycle() {
        // Parent P instantiate child C → C dulu.
        let modules = vec![test_module("p", &["c"]), test_module("c", &[])];
        let names = vec![sym("p"), sym("c")];
        let order = compute_topo_order(&modules, &names);
        let (pc, pp) = (
            order.iter().position(|n| *n == sym("c")).unwrap(),
            order.iter().position(|n| *n == sym("p")).unwrap(),
        );
        assert!(pc < pp, "{order:?}");
        // Siklus A↔B: keduanya tetap ada (fallback), tanpa hang.
        let cyc = vec![test_module("a", &["b"]), test_module("b", &["a"])];
        let cnames = vec![sym("a"), sym("b")];
        let co = compute_topo_order(&cyc, &cnames);
        assert_eq!(co.len(), 2);
    }

    #[test]
    fn test_checksum_stable_and_name_sensitive() {
        let m1 = test_module("m", &["c"]);
        let m2 = test_module("m", &["c"]);
        let m3 = test_module("m2", &["c"]);
        assert_eq!(compute_module_checksum(&m1), compute_module_checksum(&m2));
        assert_ne!(compute_module_checksum(&m1), compute_module_checksum(&m3));
    }
}
