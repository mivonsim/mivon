//! Connectivity Checking — FORMAL-13 (tahap 1 + 2a).
//!
//! Analisis statis jalur koneksi combinational antar sinyal:
//! bangun graf dependensi (rhs → lhs) dari semua assignment proses
//! Combinational/CombReactive + Initial, lalu BFS dari sumber ke tujuan.
//!
//! Tahap 2a: jalur sequential — `check_connectivity_including_seq`
//! juga memodelkan NonBlockingAssign proses Sequential sebagai edge
//! 1-hop lintas 1 cycle (dokumentasi eksplisit, bukan klaim setara
//! kombinational). API lama `check_connectivity` tetap murni
//! kombinational (tanpa ubah perilaku).

use mivon_ir::*;
use std::collections::{HashMap, HashSet};

/// Hasil pemeriksaan satu pasangan (src, dst).
#[derive(Debug, Clone)]
pub struct ConnectivityResult {
    pub src: String,
    pub dst: String,
    /// true bila ada jalur kombinational dari src ke dst.
    pub connected: bool,
    /// Jumlah hop assignment pada jalur terpendek (None bila tak terhubung).
    pub path_len: Option<usize>,
    /// Nama sinyal sepanjang jalur: [src, ..., dst].
    pub path: Vec<String>,
    /// Pesan kesalahan (mis. sinyal tidak dikenal).
    pub error: Option<String>,
}

/// Kumpulkan semua pasangan (lhs_id, rhs_expr) dari proses yang relevan.
/// `include_seq`: ikutkan NonBlockingAssign proses Sequential (edge
/// lintas 1 cycle, tahap 2a). BlockingAssign Sequential TIDAK diikutkan
/// (pola jarang, hindari false positive).
fn collect_assignments(processes: &[Process], include_seq: bool) -> Vec<(usize, IrExpr)> {
    fn walk(stmts: &[IrStmt], include_seq: bool, is_seq: bool, out: &mut Vec<(usize, IrExpr)>) {
        for stmt in stmts {
            match stmt {
                IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(id, _),
                    rhs,
                    ..
                } => {
                    if !is_seq {
                        out.push((*id, rhs.clone()));
                    }
                }
                IrStmt::NonBlockingAssign {
                    lhs: IrLValue::Signal(id, _),
                    rhs,
                    ..
                } => {
                    if include_seq && is_seq {
                        out.push((*id, rhs.clone()));
                    }
                }
                IrStmt::Block { stmts: inner } => walk(inner, include_seq, is_seq, out),
                IrStmt::NamedBlock { stmts: inner, .. } => walk(inner, include_seq, is_seq, out),
                IrStmt::If {
                    true_branch,
                    false_branch,
                    ..
                } => {
                    walk(true_branch, include_seq, is_seq, out);
                    walk(false_branch, include_seq, is_seq, out);
                }
                IrStmt::Case { items, default, .. } => {
                    for item in items {
                        walk(&item.body, include_seq, is_seq, out);
                    }
                    walk(default, include_seq, is_seq, out);
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    for process in processes {
        match process {
            Process::Combinational { body, .. }
            | Process::CombReactive { body, .. }
            | Process::Initial { name: _, body } => walk(body, include_seq, false, &mut out),
            Process::Sequential { body, .. } if include_seq => {
                walk(body, include_seq, true, &mut out)
            }
            _ => {}
        }
    }
    out
}

/// Kumpulkan id sinyal yang dirujuk dalam ekspresi.
fn collect_expr_signals(expr: &IrExpr, out: &mut Vec<usize>) {
    match expr {
        IrExpr::Signal(id, _) => out.push(*id),
        IrExpr::BinaryOp(_, l, r) => {
            collect_expr_signals(l, out);
            collect_expr_signals(r, out);
        }
        IrExpr::UnaryOp(_, v) => collect_expr_signals(v, out),
        IrExpr::Cond(c, t, f) => {
            collect_expr_signals(c, out);
            collect_expr_signals(t, out);
            collect_expr_signals(f, out);
        }
        IrExpr::Signed(v) => collect_expr_signals(v, out),
        // Const/FillLit/RangeSelect atas const dll: tanpa referensi sinyal
        // tambahan untuk MVP (RangeSelect dengan basis Signal ditangani
        // lewat arm lain bila bentuknya berbeda di skema IR saat ini).
        _ => {}
    }
}

/// Resolve nama sinyal → index di design.top.signals.
fn find_signal_index(design: &IrDesign, name: &str) -> Option<usize> {
    design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == name)
}

/// Periksa konektivitas kombinational untuk setiap pasangan (src, dst).
pub fn check_connectivity(
    design: &IrDesign,
    pairs: &[(String, String)],
) -> Vec<ConnectivityResult> {
    check_connectivity_inner(design, pairs, false)
}

/// Tahap 2a: sama tapi ikutkan edge NBA Sequential (lintas 1 cycle).
/// Jalur yang lewat register tetap 1 hop per assignment — panggil
/// hanya bila analisis sequential memang diinginkan.
pub fn check_connectivity_including_seq(
    design: &IrDesign,
    pairs: &[(String, String)],
) -> Vec<ConnectivityResult> {
    check_connectivity_inner(design, pairs, true)
}

fn check_connectivity_inner(
    design: &IrDesign,
    pairs: &[(String, String)],
    include_seq: bool,
) -> Vec<ConnectivityResult> {
    let n = design.top.signals.len();
    let name_of = |id: usize| -> String {
        design
            .top
            .signals
            .get(id)
            .map(|s| s.name.to_string())
            .unwrap_or_else(|| format!("sig_{}", id))
    };

    // Adjacency: lhs <- daftar sinyal sumber rhs. Simpan juga hop untuk
    // rekonstruksi jalur (BFS dari src maju mengikuti edge rhs→lhs).
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n]; // (dst_id, assign_idx)
    let assigns = collect_assignments(&design.top.processes, include_seq);
    for (ai, (lhs, rhs)) in assigns.iter().enumerate() {
        if *lhs >= n {
            continue;
        }
        let mut sources = Vec::new();
        collect_expr_signals(rhs, &mut sources);
        sources.sort_unstable();
        sources.dedup();
        for s in sources {
            if s < n && s != *lhs {
                adj[s].push((*lhs, ai));
            }
        }
    }

    let mut results = Vec::new();
    for (src_name, dst_name) in pairs {
        let mut res = ConnectivityResult {
            src: src_name.clone(),
            dst: dst_name.clone(),
            connected: false,
            path_len: None,
            path: Vec::new(),
            error: None,
        };
        let Some(src) = find_signal_index(design, src_name) else {
            res.error = Some(format!("sinyal '{}' tidak ditemukan", src_name));
            results.push(res);
            continue;
        };
        let Some(dst) = find_signal_index(design, dst_name) else {
            res.error = Some(format!("sinyal '{}' tidak ditemukan", dst_name));
            results.push(res);
            continue;
        };

        // BFS dari src.
        let mut prev: Vec<Option<(usize, usize)>> = vec![None; n]; // (node_sebelumnya, assign_idx)
        let mut visited = vec![false; n];
        visited[src] = true;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(src);
        while let Some(cur) = queue.pop_front() {
            if cur == dst && cur != src {
                break;
            }
            for &(next, ai) in &adj[cur] {
                if !visited[next] {
                    visited[next] = true;
                    prev[next] = Some((cur, ai));
                    queue.push_back(next);
                }
            }
        }

        if !visited[dst] || (src == dst) {
            if src == dst {
                // Trivially connected ke dirinya sendiri.
                res.connected = true;
                res.path_len = Some(0);
                res.path = vec![src_name.clone()];
            }
            results.push(res);
            continue;
        }

        // Rekonstruksi jalur dst ← ... ← src.
        let mut path_ids = vec![dst];
        let mut cur = dst;
        while let Some((p, _)) = prev[cur] {
            path_ids.push(p);
            cur = p;
            if cur == src {
                break;
            }
        }
        path_ids.reverse();
        res.connected = true;
        res.path_len = Some(path_ids.len() - 1);
        res.path = path_ids.iter().map(|&id| name_of(id)).collect();
        results.push(res);
    }
    results
}

// ═══ FORMAL-13 tahap 2b: konektivitas lintas instance/hierarki ═══
// Mendukung nama hierarki "a.b.c.sig" (multi-level: top → child bertingkat
// melalui sub_instances[].port_map) dan BFS lintas instance hingga seluruh
// hierarki desain. Batasi kedalaman rekursi untuk mencegah infinite loop
// pada instansi bersirkul (jarang terjadi pada IR yang valid).

const MAX_HIER_DEPTH: usize = 32;

/// Node dalam graf konektivitas hierarki.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HierNode {
    Top(usize),
    Child { instances: Vec<usize>, signal: usize },
}

/// Bangun HierNode dari path instansi + id sinyal.
/// Path kosong -> sinyal top; path non-kosong -> sinyal di child bertingkat.
fn node_from_path_signal(inst_path: &[usize], signal: usize) -> HierNode {
    if inst_path.is_empty() {
        HierNode::Top(signal)
    } else {
        HierNode::Child {
            instances: inst_path.to_vec(),
            signal,
        }
    }
}

/// Resolve nama hierarki "sig" (top) atau "a.b.c.sig" (child bertingkat) ke
/// HierNode. Mengwalk instance hierarchy secara rekursif hingga satu level
/// sebelum sinyal akhir.
fn resolve_hier_name(design: &IrDesign, name: &str) -> Option<HierNode> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.is_empty() {
        return None;
    }
    if parts.len() == 1 {
        // Sinyal top-level.
        let signal = design
            .top
            .signals
            .iter()
            .position(|s| s.name.as_str() == parts[0])?;
        Some(HierNode::Top(signal))
    } else {
        // Multi-level: parts[0..n-2] adalah instansi, parts[n-1] adalah sinyal.
        let signal_name = *parts.last()?;
        let inst_parts = &parts[..parts.len() - 1];

        let mut inst_path: Vec<usize> = Vec::new();
        let mut current_module: &IrModule = &design.top;

        for (i, inst_name) in inst_parts.iter().enumerate() {
            let inst_idx = current_module
                .sub_instances
                .iter()
                .position(|inst| inst.instance_name.as_str() == *inst_name)?;
            inst_path.push(inst_idx);

            let module_name = current_module.sub_instances[inst_idx].module_name.clone();
            current_module = design.modules.get(&module_name)?;

            // Pada instansi terakhir, sinyar berada di current_module.
            if i == inst_parts.len() - 1 {
                let signal = current_module
                    .signals
                    .iter()
                    .position(|s| s.name.as_str() == signal_name)?;
                return Some(HierNode::Child { instances: inst_path, signal });
            }
        }
        None
    }
}

/// Tambah edge bidirectional antara dua node (port menyambungkan sebagai satu
/// jaringan, jadi konektivitas bersifat simetris).
fn add_hier_edge(
    adj: &mut HashMap<HierNode, Vec<HierNode>>,
    a: &HierNode,
    b: &HierNode,
) {
    if a == b {
        return;
    }
    adj.entry(a.clone()).or_default().push(b.clone());
    adj.entry(b.clone()).or_default().push(a.clone());
}

/// Bangun adjacency hierarki: edge internal modul + edge lintas port antar
/// top dan child langsung. Hanya kombinational (deterministik untuk MVP).
fn build_hier_adj(design: &IrDesign) -> HashMap<HierNode, Vec<HierNode>> {
    let mut adj: HashMap<HierNode, Vec<HierNode>> = HashMap::new();
    build_hier_adj_recursive(&mut adj, design, &design.top, &[], 0);
    adj
}

fn build_hier_adj_recursive(
    adj: &mut HashMap<HierNode, Vec<HierNode>>,
    design: &IrDesign,
    module: &IrModule,
    inst_path: &[usize],
    depth: usize,
) {
    if depth > MAX_HIER_DEPTH {
        return;
    }

    // Edge internal ke modul ini (lhs <- rhs sources dari assignment kombinational).
    let assigns = collect_assignments(&module.processes, false);
    for (_ai, (lhs, rhs)) in assigns.iter().enumerate() {
        let mut sources = Vec::new();
        collect_expr_signals(rhs, &mut sources);
        sources.sort_unstable();
        sources.dedup();
        for s in sources {
            if s != *lhs {
                let lhs_node = node_from_path_signal(inst_path, *lhs);
                let rhs_node = node_from_path_signal(inst_path, s);
                // Edge internal bersifat arah (rhs → lhs), konsisten dengan
                // check_connectivity (stage 1). Port edge (di luar loop ini)
                // tetap bidirectional via add_hier_edge.
                adj.entry(rhs_node).or_default().push(lhs_node);
            }
        }
    }

    // Edge lintas port + rekursi ke setiap child instance.
    for (inst_idx, inst) in module.sub_instances.iter().enumerate() {
        let child_module = match design.modules.get(&inst.module_name) {
            Some(m) => m,
            None => continue,
        };
        let mut child_path = inst_path.to_vec();
        child_path.push(inst_idx);

        // Port edge: sinyal parent <-> sinyal port child (bidirectional, satu jaringan).
        for (port_name, parent_sig_id) in inst.port_map.iter() {
            if *parent_sig_id >= module.signals.len() {
                continue;
            }
            let child_signal = child_module
                .signals
                .iter()
                .position(|s| s.name.as_str() == port_name.as_str());
            let Some(child_signal) = child_signal else {
                continue;
            };
            let parent_node = node_from_path_signal(inst_path, *parent_sig_id);
            let child_node = HierNode::Child {
                instances: child_path.clone(),
                signal: child_signal,
            };
            add_hier_edge(adj, &parent_node, &child_node);
        }

        // Rekursi ke modul child.
        build_hier_adj_recursive(adj, design, child_module, &child_path, depth + 1);
    }
}

/// Cek konektivitas hierarki untuk pasangan nama ("sig" atau "inst.sig").
pub fn check_connectivity_hierarchical(
    design: &IrDesign,
    pairs: &[(String, String)],
) -> Vec<ConnectivityResult> {
    let adj = build_hier_adj(design);
    let mut results = Vec::new();
    for (src_name, dst_name) in pairs {
        let mut res = ConnectivityResult {
            src: src_name.clone(),
            dst: dst_name.clone(),
            connected: false,
            path_len: None,
            path: Vec::new(),
            error: None,
        };
        let Some(src) = resolve_hier_name(design, src_name) else {
            res.error = Some(format!("sinyal '{}' tidak ditemukan", src_name));
            results.push(res);
            continue;
        };
        let Some(dst) = resolve_hier_name(design, dst_name) else {
            res.error = Some(format!("sinyal '{}' tidak ditemukan", dst_name));
            results.push(res);
            continue;
        };
        if src == dst {
            res.connected = true;
            res.path_len = Some(0);
            res.path = vec![src_name.clone()];
            results.push(res);
            continue;
        }

        // BFS.
        let mut visited = HashSet::new();
        let mut prev: HashMap<HierNode, HierNode> = HashMap::new();
        let mut queue = std::collections::VecDeque::new();
        visited.insert(src.clone());
        queue.push_back(src.clone());
        let mut found = false;
        while let Some(cur) = queue.pop_front() {
            if cur == dst {
                found = true;
                break;
            }
            for next in adj.get(&cur).into_iter().flat_map(|v| v.iter()).cloned() {
                if !visited.contains(&next) {
                    visited.insert(next.clone());
                    prev.insert(next.clone(), cur.clone());
                    queue.push_back(next);
                }
            }
        }

        if found {
            let mut path_nodes = Vec::new();
            let mut cur = dst.clone();
            loop {
                path_nodes.push(cur.clone());
                if cur == src {
                    break;
                }
                cur = prev.get(&cur).expect("path harus ada").clone();
            }
            path_nodes.reverse();
            res.connected = true;
            res.path_len = Some(path_nodes.len() - 1);
            res.path = path_nodes.iter().map(|n| node_name(design, n)).collect();
        }
        results.push(res);
    }
    results
}

/// Nama string dari HierNode untuk laporan jalur (child diformat "a.b.c.sig").
fn node_name(design: &IrDesign, node: &HierNode) -> String {
    match node {
        HierNode::Top(id) => design
            .top
            .signals
            .get(*id)
            .map(|s| s.name.to_string())
            .unwrap_or_else(|| format!("sig_{}", id)),
        HierNode::Child { instances, signal } => {
            let mut names: Vec<String> = Vec::new();
            let mut current_module: &IrModule = &design.top;
            for &inst_idx in instances {
                let inst = &current_module.sub_instances[inst_idx];
                names.push(inst.instance_name.to_string());
                let module_name = inst.module_name.clone();
                if let Some(child) = design.modules.get(&module_name) {
                    current_module = child;
                }
            }
            let sig_name = current_module
                .signals
                .get(*signal)
                .map(|s| s.name.to_string())
                .unwrap_or_default();
            format!("{}.{}", names.join("."), sig_name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::intern::Symbol;
    use mivon_ir::{IrDesign, IrExpr, IrLValue, IrStmt, Process, SignalKind};
    use std::collections::HashMap;

    fn test_design() -> IrDesign {
        IrDesign {
            top: IrModule {
                name: Symbol::from("top"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("a"),
                        width: 1,
                        kind: SignalKind::Input,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("b"),
                        width: 1,
                        kind: SignalKind::Wire,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("c"),
                        width: 1,
                        kind: SignalKind::Wire,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("d"),
                        width: 1,
                        kind: SignalKind::Output,
                        ..Default::default()
                    },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![
                    Process::Combinational {
                        name: Symbol::from("comb1"),
                        sensitivity: vec![],
                        body: vec![IrStmt::BlockingAssign {
                            lhs: IrLValue::Signal(1, 1),
                            rhs: IrExpr::Signal(0, 0),
                            delay: None,
                        }],
                    },
                    Process::Combinational {
                        name: Symbol::from("comb2"),
                        sensitivity: vec![],
                        body: vec![IrStmt::BlockingAssign {
                            lhs: IrLValue::Signal(2, 1),
                            rhs: IrExpr::BinaryOp(
                                BinaryIrOp::BitOr,
                                Box::new(IrExpr::Signal(1, 0)),
                                Box::new(IrExpr::Signal(0, 0)),
                            ),
                            delay: None,
                        }],
                    },
                    Process::Combinational {
                        name: Symbol::from("comb3"),
                        sensitivity: vec![],
                        body: vec![IrStmt::BlockingAssign {
                            lhs: IrLValue::Signal(3, 1),
                            rhs: IrExpr::Signal(2, 0),
                            delay: None,
                        }],
                    },
                ],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        }
    }

    #[test]
    fn test_connectivity_chain() {
        let design = test_design();
        let results = check_connectivity(&design, &[
            ("a".to_string(), "b".to_string()),
            ("a".to_string(), "c".to_string()),
            ("a".to_string(), "d".to_string()),
            ("b".to_string(), "a".to_string()),
        ]);
        assert_eq!(results.len(), 4);

        // a -> b: connected, 1 hop
        assert!(results[0].connected);
        assert_eq!(results[0].path_len, Some(1));
        assert_eq!(results[0].path, vec!["a", "b"]);

        // a -> c: connected directly via a -> c edge (c = b | a depends on both b and a)
        assert!(results[1].connected);
        assert_eq!(results[1].path_len, Some(1));
        assert_eq!(results[1].path, vec!["a", "c"]);

        // a -> d: connected, 2 hops via a -> c -> d (shortest path)
        assert!(results[2].connected);
        assert_eq!(results[2].path_len, Some(2));
        assert_eq!(results[2].path, vec!["a", "c", "d"]);

        // b -> a: not connected (no reverse path)
        assert!(!results[3].connected);
        assert_eq!(results[3].path_len, None);
    }

    #[test]
    fn test_connectivity_unknown_signal() {
        let design = test_design();
        let results = check_connectivity(&design, &[
            ("nonexistent".to_string(), "a".to_string()),
            ("a".to_string(), "nonexistent".to_string()),
        ]);
        assert_eq!(results.len(), 2);

        assert!(!results[0].connected);
        assert!(results[0].error.is_some());
        assert!(results[0].error.as_ref().unwrap().contains("sinyal 'nonexistent' tidak ditemukan"));

        assert!(!results[1].connected);
        assert!(results[1].error.is_some());
        assert!(results[1].error.as_ref().unwrap().contains("sinyal 'nonexistent' tidak ditemukan"));
    }

    #[test]
    fn test_connectivity_including_seq() {
        let ir = IrDesign {
            top: IrModule {
                name: Symbol::from("t"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("d"),
                        width: 1,
                        kind: SignalKind::Input,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("q"),
                        width: 1,
                        kind: SignalKind::Output,
                        ..Default::default()
                    },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Sequential {
                    name: Symbol::from("ff"),
                    clock: ClockEdge::PosEdge(0),
                    reset: None,
                    body: vec![IrStmt::NonBlockingAssign {
                        lhs: IrLValue::Signal(1, 1),
                        rhs: IrExpr::Signal(0, 0),
                        delay: None,
                    }],
                    iff: None,
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        // Kombinational (tanpa include_seq): NBA write tidak terdeteksi sebagai edge
        let no_seq = check_connectivity(&ir, &[("d".to_string(), "q".to_string())]);
        assert_eq!(no_seq.len(), 1);
        assert!(!no_seq[0].connected);

        // Include_seq: NBA write d -> q detected as edge
        let with_seq = check_connectivity_including_seq(&ir, &[("d".to_string(), "q".to_string())]);
        assert_eq!(with_seq.len(), 1);
        assert!(with_seq[0].connected);
        assert_eq!(with_seq[0].path_len, Some(1));
        assert_eq!(with_seq[0].path, vec!["d", "q"]);
    }

    #[test]
    fn test_connectivity_hierarchical_cross_instance() {
        use std::sync::Arc;
        use mivon_ir::IrInstance;

        // Top: sinyal "a" (id 0) dan "x" (id 1). Instance "u0" dari modul "sub".
        // port_map: {"b": 0} → top sinyal "a" mengemudi child port "b".
        // Child "sub": sinyal "b" (id 0, input), "c" (id 1, wire).
        // Proses child: c = b. Jadi hierarki: a -> b(port) -> c.
        let child_signals = vec![
            SignalInfo {
                name: Symbol::from("b"),
                width: 1,
                kind: SignalKind::Input,
                ..Default::default()
            },
            SignalInfo {
                name: Symbol::from("c"),
                width: 1,
                kind: SignalKind::Wire,
                ..Default::default()
            },
        ];
        let child_module = IrModule {
            name: Symbol::from("sub"),
            signals: child_signals,
            inputs: vec![],
            outputs: vec![],
            inouts: vec![],
            processes: vec![Process::Combinational {
                name: Symbol::from("comb"),
                sensitivity: vec![],
                body: vec![IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(1, 1),
                    rhs: IrExpr::Signal(0, 0),
                    delay: None,
                }],
            }],
            sub_instances: vec![],
        };

        let top_signals = vec![
            SignalInfo {
                name: Symbol::from("a"),
                width: 1,
                kind: SignalKind::Wire,
                ..Default::default()
            },
            SignalInfo {
                name: Symbol::from("x"),
                width: 1,
                kind: SignalKind::Wire,
                ..Default::default()
            },
        ];

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("top"),
                signals: top_signals,
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![],
                sub_instances: vec![IrInstance {
                    module_name: Symbol::from("sub"),
                    instance_name: Symbol::from("u0"),
                    port_map: Arc::new(HashMap::from([(Symbol::from("b"), 0)])),
                    param_map: Arc::new(HashMap::new()),
                    type_param_map: Arc::new(HashMap::new()),
                    line: 1,
                    col: 1,
                    attrs: Vec::new(),
                    param_width_map: Arc::new(HashMap::new()),
                }],
            },
            modules: HashMap::from([(Symbol::from("sub"), child_module)]),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        // a (top id 0) -> u0.b (child id 0) -> u0.c (child id 1): terhubung.
        let r = check_connectivity_hierarchical(&design, &[("a".to_string(), "u0.c".to_string())]);
        assert_eq!(r.len(), 1);
        assert!(r[0].connected, "a harus terhubung ke u0.c lewat port b");
        assert_eq!(r[0].path_len, Some(2));
        assert_eq!(r[0].path, vec!["a", "u0.b", "u0.c"]);

        // x (top, tidak terhubung) -> u0.c: tidak terhubung.
        let r2 = check_connectivity_hierarchical(&design, &[("x".to_string(), "u0.c".to_string())]);
        assert_eq!(r2.len(), 1);
        assert!(!r2[0].connected);

        // Sinyal child tak dikenal: error.
        let r3 = check_connectivity_hierarchical(&design, &[("a".to_string(), "u0.zzz".to_string())]);
        assert_eq!(r3.len(), 1);
        assert!(!r3[0].connected);
        assert!(r3[0].error.is_some());

        // Sinyal top tak dikenal: error.
        let r4 = check_connectivity_hierarchical(&design, &[("zzz".to_string(), "u0.c".to_string())]);
        assert_eq!(r4.len(), 1);
        assert!(!r4[0].connected);
        assert!(r4[0].error.is_some());
    }

    #[test]
    fn test_connectivity_hierarchical_multi_level() {
        use std::sync::Arc;
        use mivon_ir::IrInstance;

        // Hierarki 3 level: top → u0 (mod "mid") → u00 (mod "leaf").
        // top: sinyal "a" (id 0).
        // mid: port "b" (id 0, dihubungkan ke top.a), sinyal internal "c" (id 1).
        //   proses mid: c = b.
        //   mid punya instance u00 dari modul "leaf".
        // leaf: port "d" (id 0, dihubungkan ke mid.c), sinyal internal "e" (id 1).
        //   proses leaf: e = d.
        // Jalur terduga: a -> u0.b -> u0.c -> u0.u00.d -> u0.u00.e (4 hop).

        let leaf_signals = vec![
            SignalInfo {
                name: Symbol::from("d"),
                width: 1,
                kind: SignalKind::Input,
                ..Default::default()
            },
            SignalInfo {
                name: Symbol::from("e"),
                width: 1,
                kind: SignalKind::Wire,
                ..Default::default()
            },
        ];
        let leaf_module = IrModule {
            name: Symbol::from("leaf"),
            signals: leaf_signals,
            inputs: vec![],
            outputs: vec![],
            inouts: vec![],
            processes: vec![Process::Combinational {
                name: Symbol::from("comb"),
                sensitivity: vec![],
                body: vec![IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(1, 1),
                    rhs: IrExpr::Signal(0, 0),
                    delay: None,
                }],
            }],
            sub_instances: vec![],
        };

        let mid_signals = vec![
            SignalInfo {
                name: Symbol::from("b"),
                width: 1,
                kind: SignalKind::Input,
                ..Default::default()
            },
            SignalInfo {
                name: Symbol::from("c"),
                width: 1,
                kind: SignalKind::Wire,
                ..Default::default()
            },
        ];
        let mid_module = IrModule {
            name: Symbol::from("mid"),
            signals: mid_signals,
            inputs: vec![],
            outputs: vec![],
            inouts: vec![],
            processes: vec![Process::Combinational {
                name: Symbol::from("comb"),
                sensitivity: vec![],
                body: vec![IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(1, 1),
                    rhs: IrExpr::Signal(0, 0),
                    delay: None,
                }],
            }],
            sub_instances: vec![IrInstance {
                module_name: Symbol::from("leaf"),
                instance_name: Symbol::from("u00"),
                port_map: Arc::new(HashMap::from([(Symbol::from("d"), 1)])),
                param_map: Arc::new(HashMap::new()),
                type_param_map: Arc::new(HashMap::new()),
                line: 1,
                col: 1,
                attrs: Vec::new(),
                param_width_map: Arc::new(HashMap::new()),
            }],
        };

        let top_signals = vec![SignalInfo {
            name: Symbol::from("a"),
            width: 1,
            kind: SignalKind::Wire,
            ..Default::default()
        }];

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("top"),
                signals: top_signals,
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![],
                sub_instances: vec![IrInstance {
                    module_name: Symbol::from("mid"),
                    instance_name: Symbol::from("u0"),
                    port_map: Arc::new(HashMap::from([(Symbol::from("b"), 0)])),
                    param_map: Arc::new(HashMap::new()),
                    type_param_map: Arc::new(HashMap::new()),
                    line: 1,
                    col: 1,
                    attrs: Vec::new(),
                    param_width_map: Arc::new(HashMap::new()),
                }],
            },
            modules: HashMap::from([
                (Symbol::from("mid"), mid_module.clone()),
                (Symbol::from("leaf"), leaf_module.clone()),
            ]),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        // a (top) → u0.b (mid port) → u0.c (mid internal) → u0.u00.d (leaf port) → u0.u00.e
        let r = check_connectivity_hierarchical(&design, &[
            ("a".to_string(), "u0.u00.e".to_string()),
        ]);
        assert_eq!(r.len(), 1);
        assert!(r[0].connected, "a harus terhubung ke u0.u00.e lewat 3 level hierarki");
        assert_eq!(r[0].path_len, Some(4));
        assert_eq!(
            r[0].path,
            vec!["a", "u0.b", "u0.c", "u0.u00.d", "u0.u00.e"]
        );

        // Tidak ada jalur dari a ke sinyal yang tidak ada di leaf
        let r2 = check_connectivity_hierarchical(&design, &[
            ("a".to_string(), "u0.u00.zzz".to_string()),
        ]);
        assert_eq!(r2.len(), 1);
        assert!(!r2[0].connected);
        assert!(r2[0].error.is_some());

        // Sinyal top yang tidak terhubung ke u0.u00.e
        let top_x_signals = vec![
            SignalInfo {
                name: Symbol::from("a"),
                width: 1,
                kind: SignalKind::Wire,
                ..Default::default()
            },
            SignalInfo {
                name: Symbol::from("x"),
                width: 1,
                kind: SignalKind::Wire,
                ..Default::default()
            },
        ];
        let design2 = IrDesign {
            top: IrModule {
                name: Symbol::from("top"),
                signals: top_x_signals,
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![],
                sub_instances: vec![IrInstance {
                    module_name: Symbol::from("mid"),
                    instance_name: Symbol::from("u0"),
                    port_map: Arc::new(HashMap::from([(Symbol::from("b"), 0)])),
                    param_map: Arc::new(HashMap::new()),
                    type_param_map: Arc::new(HashMap::new()),
                    line: 1,
                    col: 1,
                    attrs: Vec::new(),
                    param_width_map: Arc::new(HashMap::new()),
                }],
            },
            modules: HashMap::from([
                (Symbol::from("mid"), mid_module.clone()),
                (Symbol::from("leaf"), leaf_module.clone()),
            ]),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };
        let r3 = check_connectivity_hierarchical(&design2, &[
            ("x".to_string(), "u0.u00.e".to_string()),
        ]);
        assert_eq!(r3.len(), 1);
        assert!(!r3[0].connected);
    }
}
