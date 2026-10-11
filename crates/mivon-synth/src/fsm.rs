//! FSM Extraction — COMP-11 (tahap 1 dari rencana bertahap).
//!
//! Mendeteksi dan mengekstrak finite state machine dari proses
//! Sequential RTL:
//!   1. State register = sinyal yang menjadi SELECTOR sebuah `case`
//!      DAN ditulis via NonBlockingAssign di proses yang sama.
//!   2. Transisi = untuk tiap item case dengan label konstanta (state
//!      saat ini), cari NBA-write konstanta ke state register di body
//!      item (state berikutnya).
//!
//! Tahapan lanjutan (belum): deteksi output Mealy/Moore,
//! lintas-always FSM.
//!
//! Tahap 2a (selesai): minimisasi ringan — buang state terisolasi
//! (tanpa transisi masuk/keluar) + encoding padat 0..N-1 untuk report.
//!
//! Catatan penamaan: fungsi/struct di modul ini memakai awalan `Fsm`
//! agar tidak tertukar dengan tipe netlist lain.

use mivon_ir::*;

/// Satu transisi FSM: dari nilai state `from` (label case) ke `to`
/// (konstanta NBA-write). `from = None` berarti berasal dari default/
/// kondisi tak dikenal.
#[derive(Debug, Clone, PartialEq)]
pub struct FsmTransition {
    pub from: Option<u64>,
    pub to: u64,
}

/// Info satu FSM terdeteksi.
#[derive(Debug, Clone)]
pub struct FsmInfo {
    /// Nama proses Sequential asal.
    pub process_name: String,
    /// Sinyal state register.
    pub state_signal: String,
    pub state_width: usize,
    /// Nilai state unik yang diketahui (sorted).
    pub states: Vec<u64>,
    /// Transisi antar state.
    pub transitions: Vec<FsmTransition>,
}

/// Evaluasi sederhana ekspresi konstanta → u64 (MVP):
/// Const, Signed(Const), FillLit, dan BinaryOp aritmetika/logis bit
/// atas operand konstanta. Selain itu → None.
pub(crate) fn try_const_u64(expr: &IrExpr) -> Option<u64> {
    match expr {
        IrExpr::Const(lv) => Some(lv.to_u64()),
        IrExpr::FillLit(v) => Some(if *v == LogicVal::Zero { 0 } else { 1 }),
        IrExpr::Signed(inner) => try_const_u64(inner),
        IrExpr::BinaryOp(op, l, r) => {
            let a = try_const_u64(l)?;
            let b = try_const_u64(r)?;
            Some(match op {
                BinaryIrOp::Add => a.wrapping_add(b),
                BinaryIrOp::Sub => a.wrapping_sub(b),
                BinaryIrOp::Mul => a.wrapping_mul(b),
                BinaryIrOp::BitAnd => a & b,
                BinaryIrOp::BitOr => a | b,
                BinaryIrOp::BitXor => a ^ b,
                _ => return None,
            })
        }
        _ => None,
    }
}

/// Kumpulkan nilai konstanta NBA-write ke `sig_id` dalam sekumpulan
/// statement (rekursif Block/If/Case).
fn collect_nba_consts(stmts: &[IrStmt], sig_id: usize, out: &mut Vec<u64>) {
    for stmt in stmts {
        match stmt {
            IrStmt::NonBlockingAssign {
                lhs: IrLValue::Signal(id, _),
                rhs,
                ..
            } => {
                if *id == sig_id {
                    if let Some(v) = try_const_u64(rhs) {
                        out.push(v);
                    }
                }
            }
            IrStmt::Block { stmts: inner } => collect_nba_consts(inner, sig_id, out),
            IrStmt::NamedBlock { stmts: inner, .. } => collect_nba_consts(inner, sig_id, out),
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                collect_nba_consts(true_branch, sig_id, out);
                collect_nba_consts(false_branch, sig_id, out);
            }
            _ => {}
        }
    }
}

/// Apakah state register menerima NBA-write dalam sekumpulan statement?
fn has_nba_write_to(stmts: &[IrStmt], sig_id: usize) -> bool {
    for stmt in stmts {
        match stmt {
            IrStmt::NonBlockingAssign {
                lhs: IrLValue::Signal(id, _),
                ..
            } => {
                if *id == sig_id {
                    return true;
                }
            }
            IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                if has_nba_write_to(inner, sig_id) {
                    return true;
                }
            }
            IrStmt::Case { items, default, .. } => {
                for item in items {
                    if has_nba_write_to(&item.body, sig_id) {
                        return true;
                    }
                }
                if has_nba_write_to(default, sig_id) {
                    return true;
                }
            }
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } if (has_nba_write_to(true_branch, sig_id)
                || has_nba_write_to(false_branch, sig_id)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Ekstraksi FSM utama: pindai semua proses Sequential pada top module.
pub fn extract_fsms(design: &IrDesign) -> Vec<FsmInfo> {
    let mut fsms = Vec::new();

    for process in &design.top.processes {
        let (pname, body) = match process {
            Process::Sequential { name, body, .. } => (name.to_string(), body),
            _ => continue,
        };

        // Cari case tingkat atas yang selectornya sinyal.
        for stmt in body {
            let IrStmt::Case {
                expr,
                items,
                default,
                ..
            } = stmt
            else {
                continue;
            };
            let IrExpr::Signal(sig_id, _) = expr else {
                continue;
            };
            // Syarat FSM: selector juga ditulis NBA di proses ini.
            let written = body
                .iter()
                .any(|s| has_nba_write_to(std::slice::from_ref(s), *sig_id));
            if !written {
                continue;
            }

            let Some(sig_info) = design.top.signals.get(*sig_id) else {
                continue;
            };
            let mut info = FsmInfo {
                process_name: pname.clone(),
                state_signal: sig_info.name.to_string(),
                state_width: sig_info.width,
                states: Vec::new(),
                transitions: Vec::new(),
            };

            let push_state = |states: &mut Vec<u64>, v: u64| {
                if !states.contains(&v) {
                    states.push(v);
                }
            };
            let push_trans = |t: &mut Vec<FsmTransition>, tr: FsmTransition| {
                if !t.contains(&tr) {
                    t.push(tr);
                }
            };

            for item in items {
                // State saat ini = konstanta pertama pada label yang bisa
                // dievaluasi.
                let from = item.labels.iter().find_map(try_const_u64);
                // State berikutnya = konstanta NBA pertama ke state reg.
                let mut nexts = Vec::new();
                collect_nba_consts(&item.body, *sig_id, &mut nexts);
                if let Some(from_v) = from {
                    push_state(&mut info.states, from_v);
                    for to in &nexts {
                        push_state(&mut info.states, *to);
                        push_trans(
                            &mut info.transitions,
                            FsmTransition {
                                from: Some(from_v),
                                to: *to,
                            },
                        );
                    }
                }
            }

            // Default branch: transisi dari state mana pun yang belum
            // punya keluar (konservatif — hanya bila ada NBA konstanta).
            let mut default_nexts = Vec::new();
            collect_nba_consts(default, *sig_id, &mut default_nexts);
            for to in &default_nexts {
                push_state(&mut info.states, *to);
                for &from in info.states.clone().iter() {
                    if from != *to && !info.transitions.iter().any(|t| t.from == Some(from)) {
                        push_trans(
                            &mut info.transitions,
                            FsmTransition {
                                from: Some(from),
                                to: *to,
                            },
                        );
                    }
                }
            }

            info.states.sort_unstable();
            if !info.transitions.is_empty() {
                fsms.push(info);
            }
        }
    }

    fsms
}

/// COMP-11 tahap 2a: state terpakai = muncul di ≥1 transisi
/// (sebagai `from` atau `to`). State terisolasi dibuang.
pub fn minimized_states(fsm: &FsmInfo) -> Vec<u64> {
    let mut out: Vec<u64> = fsm
        .states
        .iter()
        .copied()
        .filter(|s| {
            fsm.transitions
                .iter()
                .any(|t| t.from == Some(*s) || t.to == *s)
        })
        .collect();
    out.sort_unstable();
    out
}

/// COMP-11 tahap 2a: encoding padat old → 0..N-1 berdasar urutan
/// `minimized_states`.
pub fn compact_encoding(fsm: &FsmInfo) -> Vec<(u64, u64)> {
    minimized_states(fsm)
        .into_iter()
        .enumerate()
        .map(|(i, s)| (s, i as u64))
        .collect()
}

/// Render laporan FSM ke teks (COMP-11 tahap 2) — dipakai synth CLI.
pub fn render_fsm_report(fsms: &[FsmInfo]) -> String {
    let mut out = String::new();
    out.push_str("\n─── FSM Report ───\n");
    if fsms.is_empty() {
        out.push_str("  (tidak ada FSM terdeteksi)\n");
        return out;
    }
    for f in fsms {
        out.push_str(&format!("  {} [{}]\n", f.process_name, f.state_signal));
        out.push_str(&format!(
            "    states ({}): {}\n",
            f.states.len(),
            f.states
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out.push_str(&format!("    transitions ({}):\n", f.transitions.len()));
        for t in &f.transitions {
            let from = match t.from {
                Some(v) => v.to_string(),
                None => "*".to_string(),
            };
            out.push_str(&format!("      {} → {}\n", from, t.to));
        }
        let mini = minimized_states(f);
        if mini.len() != f.states.len() {
            out.push_str(&format!(
                "    minimized ({}): {}\n",
                mini.len(),
                mini.iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let enc = compact_encoding(f);
        if enc.iter().any(|(o, n)| o != n) {
            out.push_str(&format!(
                "    encoding: {}\n",
                enc.iter()
                    .map(|(o, n)| format!("{o}->{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    out.push_str(&format!("  total: {} FSM\n", fsms.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::intern::Symbol;

    /// Proses sequential 3-state: case(state) 0→1, 1→2, default→0.
    fn make_design() -> IrDesign {
        let nba = |sig: usize, val: u64| IrStmt::NonBlockingAssign {
            lhs: IrLValue::Signal(sig, 2),
            rhs: IrExpr::Const(LogicVec::from_u64(val, 2)),
            delay: None,
        };
        let state_case = IrStmt::Case {
            case_type: CaseType::Normal,
            expr: IrExpr::Signal(0, 0),
            items: vec![
                IrCaseItem {
                    labels: vec![IrExpr::Const(LogicVec::from_u64(0, 2))],
                    body: vec![nba(0, 1)],
                },
                IrCaseItem {
                    labels: vec![IrExpr::Const(LogicVec::from_u64(1, 2))],
                    body: vec![nba(0, 2)],
                },
            ],
            default: vec![nba(0, 0)],
        };
        let mut design = IrDesign::default();
        design.top.name = Symbol::from("fsm_tb");
        design.top.signals = vec![SignalInfo {
            name: Symbol::from("state"),
            width: 2,
            ..Default::default()
        }];
        design.top.processes = vec![Process::Sequential {
            name: Symbol::from("state_machine"),
            clock: ClockEdge::PosEdge(0),
            reset: None,
            body: vec![state_case],
            iff: None,
        }];
        design
    }

    #[test]
    fn test_fsm_extraction_three_states() {
        let design = make_design();
        let fsms = extract_fsms(&design);
        assert_eq!(fsms.len(), 1, "satu FSM terdeteksi");
        let f = &fsms[0];
        assert_eq!(f.state_signal, "state");
        assert_eq!(f.process_name, "state_machine");
        assert_eq!(
            f.states,
            vec![0, 1, 2],
            "3 state: 2 dari label + 1 dari default"
        );
        // Transisi eksplisit: 0→1, 1→2; default menambah keluar untuk
        // state yang belum punya (2→0).
        assert_eq!(f.transitions.len(), 3, "{:?}", f.transitions);
        assert!(f.transitions.contains(&FsmTransition {
            from: Some(0),
            to: 1
        }));
        assert!(f.transitions.contains(&FsmTransition {
            from: Some(1),
            to: 2
        }));
        assert!(f.transitions.contains(&FsmTransition {
            from: Some(2),
            to: 0
        }));
    }

    #[test]
    fn test_fsm_not_detected_without_sequential() {
        // Tanpa proses Sequential → tidak ada FSM.
        let mut design = make_design();
        design.top.processes = vec![];
        assert!(extract_fsms(&design).is_empty());
    }

    #[test]
    fn test_render_fsm_report() {
        let design = make_design();
        let fsms = extract_fsms(&design);
        let report = render_fsm_report(&fsms);
        assert!(report.contains("FSM Report"));
        assert!(report.contains("state_machine [state]"));
        assert!(report.contains("states (3): 0, 1, 2"));
        assert!(report.contains("0 → 1"));
        assert!(report.contains("total: 1 FSM"));
        // Kosong → pesan jelas.
        let empty = render_fsm_report(&[]);
        assert!(empty.contains("tidak ada FSM"));
    }

    #[test]
    fn test_fsm_requires_nba_to_selector() {
        // Case selector sinyal TANPA NBA-write ke selector → bukan FSM.
        let mut design = make_design();
        // Ubah semua NBA target ke signal 9 (tidak ada di design → juga
        // tidak menulis state reg).
        for p in &mut design.top.processes {
            if let Process::Sequential { body, .. } = p {
                for stmt in body.iter_mut() {
                    if let IrStmt::Case { items, default, .. } = stmt {
                        for item in items {
                            for st in item.body.iter_mut() {
                                if let IrStmt::NonBlockingAssign {
                                    lhs: IrLValue::Signal(id, _),
                                    ..
                                } = st
                                {
                                    *id = 9;
                                }
                            }
                        }
                        for st in default.iter_mut() {
                            if let IrStmt::NonBlockingAssign {
                                lhs: IrLValue::Signal(id, _),
                                ..
                            } = st
                            {
                                *id = 9;
                            }
                        }
                    }
                }
            }
        }
        assert!(extract_fsms(&design).is_empty());
    }

    #[test]
    fn test_fsm_minimize_drops_isolated() {
        // State 9 terisolasi (ada di daftar tapi tanpa transisi) dibuang;
        // encoding padat memetakan 0/1/2 → 0/1/2 dan 5 → 3.
        let f = FsmInfo {
            process_name: "p".to_string(),
            state_signal: "s".to_string(),
            state_width: 3,
            states: vec![0, 1, 2, 5, 9],
            transitions: vec![
                FsmTransition { from: Some(0), to: 1 },
                FsmTransition { from: Some(1), to: 2 },
                FsmTransition { from: Some(2), to: 5 },
            ],
        };
        assert_eq!(minimized_states(&f), vec![0, 1, 2, 5]);
        assert_eq!(
            compact_encoding(&f),
            vec![(0, 0), (1, 1), (2, 2), (5, 3)]
        );
        let report = render_fsm_report(&[f]);
        assert!(report.contains("minimized (4): 0, 1, 2, 5"));
        assert!(report.contains("encoding: 0->0, 1->1, 2->2, 5->3"));
    }
}
