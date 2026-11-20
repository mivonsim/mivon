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
}
