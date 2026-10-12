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
//! Tahap 2b (selesai): klasifikasi output Mealy/Moore.
//! Tahap 2c (selesai): pola 2-proses klasik — `state <= next` di
//! always_ff + `case (state)` di combinational yang menulis `next`.
//! Batasan: case tingkat atas proses (tidak dibungkus Block/If).
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
    /// Output FSM terklasifikasi (COMP-11 tahap 2b).
    pub outputs: Vec<FsmOutput>,
}

/// Klasifikasi output FSM: Mealy (kombinational dari input — RHS
/// merujuk sinyal selain state reg) vs Moore (hanya state/konstanta).
#[derive(Debug, Clone, PartialEq)]
pub enum FsmOutputKind {
    Mealy,
    Moore,
}

/// Satu output FSM + klasifikasinya.
#[derive(Debug, Clone)]
pub struct FsmOutput {
    pub signal: String,
    pub kind: FsmOutputKind,
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

/// Kumpulkan nilai konstanta write (Blocking + NBA) ke `sig_id` dalam
/// sekumpulan statement (rekursif Block/If). LHS select/array di-resolve
/// ke id target; HierRef dilewati.
fn collect_const_writes_to(stmts: &[IrStmt], sig_id: usize, out: &mut Vec<u64>) {
    for stmt in stmts {
        match stmt {
            IrStmt::BlockingAssign { lhs, rhs, .. }
            | IrStmt::NonBlockingAssign { lhs, rhs, .. } => {
                if lvalue_sig_id(lhs) == Some(sig_id) {
                    if let Some(v) = try_const_u64(rhs) {
                        out.push(v);
                    }
                }
            }
            IrStmt::Block { stmts: inner } => collect_const_writes_to(inner, sig_id, out),
            IrStmt::NamedBlock { stmts: inner, .. } => collect_const_writes_to(inner, sig_id, out),
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                collect_const_writes_to(true_branch, sig_id, out);
                collect_const_writes_to(false_branch, sig_id, out);
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

/// Kumpulkan id sinyal yang dirujuk sebuah ekspresi RHS.
fn collect_rhs_signals(expr: &IrExpr, out: &mut Vec<usize>) {
    match expr {
        IrExpr::Signal(id, _) | IrExpr::RangeSelect(id, ..) | IrExpr::BitSelect(id, ..) => {
            out.push(*id)
        }
        IrExpr::ArrayIndex { sig_id, index, .. } => {
            out.push(*sig_id);
            collect_rhs_signals(index, out);
        }
        IrExpr::ExprRangeSelect(b, ..)
        | IrExpr::ExprBitSelect(b, ..)
        | IrExpr::UnaryOp(_, b)
        | IrExpr::Signed(b)
        | IrExpr::Cast { expr: b, .. }
        | IrExpr::MemberAccess { obj: b, .. }
        | IrExpr::Replicate(_, b) => collect_rhs_signals(b, out),
        IrExpr::ExprPartSelect(a, b, c) | IrExpr::Cond(a, b, c) => {
            collect_rhs_signals(a, out);
            collect_rhs_signals(b, out);
            collect_rhs_signals(c, out);
        }
        IrExpr::BinaryOp(_, l, r) => {
            collect_rhs_signals(l, out);
            collect_rhs_signals(r, out);
        }
        IrExpr::Concat(parts) => {
            for p in parts {
                collect_rhs_signals(p, out);
            }
        }
        IrExpr::SysFunc { args, .. } | IrExpr::DpiCall { args, .. } | IrExpr::NewCall { args, .. } => {
            for a in args {
                collect_rhs_signals(a, out);
            }
        }
        IrExpr::MethodCall { obj, args, with_clause, .. } => {
            collect_rhs_signals(obj, out);
            for a in args {
                collect_rhs_signals(a, out);
            }
            if let Some(w) = with_clause {
                collect_rhs_signals(w, out);
            }
        }
        IrExpr::Inside { expr, list } => {
            collect_rhs_signals(expr, out);
            for e in list {
                collect_rhs_signals(e, out);
            }
        }
        IrExpr::InsideRange { expr, lo, hi } => {
            collect_rhs_signals(expr, out);
            collect_rhs_signals(lo, out);
            collect_rhs_signals(hi, out);
        }
        IrExpr::StreamingConcat { slices, .. } => {
            for s in slices {
                collect_rhs_signals(s, out);
            }
        }
        IrExpr::Dist { expr, .. } => collect_rhs_signals(expr, out),
        IrExpr::UdpLookup { args, .. } | IrExpr::FuncCall { args, .. } => {
            for a in args {
                collect_rhs_signals(a, out);
            }
        }
        IrExpr::IncDec { read, lv, .. } => {
            collect_rhs_signals(read, out);
            lvalue_index_signals(lv, out);
        }
        _ => {}
    }
}

/// Id sinyal target sebuah lvalue (None utk HierRef/HierRefIndex yang
/// belum ter-resolve — dilewati konservatif).
fn lvalue_sig_id(lv: &IrLValue) -> Option<usize> {
    match lv {
        IrLValue::Signal(id, _)
        | IrLValue::RangeSelect(id, ..)
        | IrLValue::BitSelect(id, ..)
        | IrLValue::ArrayIndex { sig_id: id, .. }
        | IrLValue::ArrayRangeSelect { sig_id: id, .. }
        | IrLValue::ArrayBitSelect { sig_id: id, .. }
        | IrLValue::ExprPartSelect { sig_id: id, .. }
        | IrLValue::ObjectField { sig_id: id, .. } => Some(*id),
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } => None,
        IrLValue::Concat(_) => None,
    }
}

/// Kumpulkan sinyal dari ekspresi index/base dalam lvalue
/// (`arr[i]`, `sig[base +: w]`) — pengaruh input di index dianggap
/// dependensi Mealy (konservatif).
fn lvalue_index_signals(lv: &IrLValue, out: &mut Vec<usize>) {
    match lv {
        IrLValue::ArrayIndex { index, .. }
        | IrLValue::ArrayRangeSelect { index, .. }
        | IrLValue::HierRefIndex { index, .. } => collect_rhs_signals(index, out),
        IrLValue::ArrayBitSelect { index, bit, .. } => {
            collect_rhs_signals(index, out);
            collect_rhs_signals(bit, out);
        }
        IrLValue::ExprPartSelect { base, .. } => collect_rhs_signals(base, out),
        IrLValue::Concat(parts) => {
            for p in parts {
                lvalue_index_signals(p, out);
            }
        }
        _ => {}
    }
}

/// Kumpulkan semua write (Blocking + NBA) ke sinyal dalam statement —
/// dipakai klasifikasi output Mealy/Moore (tahap 2b). LHS non-Signal
/// (RangeSelect/BitSelect/ArrayIndex/Concat) di-resolve ke id target;
/// HierRef dilewati konservatif. Batasan: tidak turun ke For/While/
/// Repeat/Foreach/Delay/Force; kondisi `If` tidak dihitung dependensi
/// (klasifikasi RHS-only).
fn collect_writes(stmts: &[IrStmt], out: &mut Vec<(usize, IrExpr, Vec<usize>)>) {
    fn push_lv(lv: &IrLValue, rhs: &IrExpr, out: &mut Vec<(usize, IrExpr, Vec<usize>)>) {
        match lv {
            IrLValue::Concat(parts) => {
                for p in parts {
                    push_lv(p, rhs, out);
                }
            }
            _ => {
                if let Some(id) = lvalue_sig_id(lv) {
                    let mut idx = Vec::new();
                    lvalue_index_signals(lv, &mut idx);
                    out.push((id, rhs.clone(), idx));
                }
            }
        }
    }
    for stmt in stmts {
        match stmt {
            IrStmt::BlockingAssign { lhs, rhs, .. }
            | IrStmt::NonBlockingAssign { lhs, rhs, .. } => push_lv(lhs, rhs, out),
            IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                collect_writes(inner, out)
            }
            IrStmt::Case { items, default, .. } => {
                for item in items {
                    collect_writes(&item.body, out);
                }
                collect_writes(default, out);
            }
            IrStmt::If { true_branch, false_branch, .. } => {
                collect_writes(true_branch, out);
                collect_writes(false_branch, out);
            }
            _ => {}
        }
    }
}

/// Ekstraksi transisi + output dari satu `case`.
/// Parameter:
///   - `state_sig`: sinyal selector case (state saat ini).
//   - `next_sig`: sinyal yang membawa state berikutnya. Pola 1-proses:
//     sama dengan state_sig; pola lintas-proses 2c: sinyal next-state
//     yang di-copy ke state oleh `state <= next` di always_ff.
// Return (states sorted, transitions, outputs sorted by nama). Output =
// write selain state & next di case; RHS selain state → Mealy.
fn extract_from_case(
    items: &[IrCaseItem],
    default: &[IrStmt],
    state_sig: usize,
    next_sig: usize,
    design: &IrDesign,
) -> (Vec<u64>, Vec<FsmTransition>, Vec<FsmOutput>) {
    let mut states: Vec<u64> = Vec::new();
    let mut transitions: Vec<FsmTransition> = Vec::new();
    let mut outputs: Vec<FsmOutput> = Vec::new();
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
        // State saat ini = konstanta pertama pada label yang bisa dievaluasi.
        let from = item.labels.iter().find_map(try_const_u64);
        // State berikutnya = konstanta write pertama ke next_sig.
        let mut nexts = Vec::new();
        collect_const_writes_to(&item.body, next_sig, &mut nexts);
        if let Some(from_v) = from {
            push_state(&mut states, from_v);
            for to in &nexts {
                push_state(&mut states, *to);
                push_trans(
                    &mut transitions,
                    FsmTransition { from: Some(from_v), to: *to },
                );
            }
        }
    }

    // Default branch: transisi dari state mana pun yang belum punya keluar
    // (konservatif — hanya bila ada write konstanta).
    let mut default_nexts = Vec::new();
    collect_const_writes_to(default, next_sig, &mut default_nexts);
    for to in &default_nexts {
        push_state(&mut states, *to);
        for &from in states.clone().iter() {
            if from != *to && !transitions.iter().any(|t| t.from == Some(from)) {
                push_trans(&mut transitions, FsmTransition { from: Some(from), to: *to });
            }
        }
    }

    states.sort_unstable();

    // Klasifikasi output — write selain state & next di dalam case.
    let mut writes: Vec<(usize, IrExpr, Vec<usize>)> = Vec::new();
    for item in items {
        collect_writes(&item.body, &mut writes);
    }
    collect_writes(default, &mut writes);
    for (id, rhs, idx_deps) in &writes {
        if *id == state_sig || *id == next_sig {
            continue;
        }
        let mut deps = idx_deps.clone();
        collect_rhs_signals(rhs, &mut deps);
        let mealy = deps.iter().any(|d| *d != state_sig);
        let name = design.top.signals.get(*id).map(|s| s.name.to_string());
        let Some(name) = name else { continue };
        match outputs.iter_mut().find(|o| o.signal == name) {
            Some(o) => {
                if mealy {
                    o.kind = FsmOutputKind::Mealy;
                }
            }
            None => outputs.push(FsmOutput {
                signal: name,
                kind: if mealy { FsmOutputKind::Mealy } else { FsmOutputKind::Moore },
            }),
        }
    }
    outputs.sort_by(|a, b| a.signal.cmp(&b.signal));
    (states, transitions, outputs)
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
            let (states, transitions, outputs) =
                extract_from_case(items, default, *sig_id, *sig_id, design);
            let info = FsmInfo {
                process_name: pname.clone(),
                state_signal: sig_info.name.to_string(),
                state_width: sig_info.width,
                states,
                transitions,
                outputs,
            };
            if !info.transitions.is_empty() {
                fsms.push(info);
            }
        }
    }

    // COMP-11 tahap 2c: pola 2-proses klasik — `state <= next` (NBA,
    // RHS tepat satu sinyal) di always_ff + `case (state)` di proses
    // Combinational/CombReactive yang menulis `next` secara Blocking.
    // Lewati state yang sudah punya FSM tahap 1.
    {
        // Petakan state → next dari copy NBA di proses Sequential.
        fn find_nba_copies(stmts: &[IrStmt], out: &mut Vec<(usize, usize)>) {
            for stmt in stmts {
                match stmt {
                    IrStmt::NonBlockingAssign { lhs, rhs, .. } => {
                        if let (Some(id), IrExpr::Signal(next, _)) =
                            (lvalue_sig_id(lhs), rhs)
                        {
                            if *next != id && !out.contains(&(id, *next)) {
                                out.push((id, *next));
                            }
                        }
                    }
                    IrStmt::Block { stmts: inner }
                    | IrStmt::NamedBlock { stmts: inner, .. } => find_nba_copies(inner, out),
                    IrStmt::If { true_branch, false_branch, .. } => {
                        find_nba_copies(true_branch, out);
                        find_nba_copies(false_branch, out);
                    }
                    _ => {}
                }
            }
        }
        let mut copies: Vec<(usize, usize)> = Vec::new();
        for process in &design.top.processes {
            let Process::Sequential { body, .. } = process else {
                continue;
            };
            find_nba_copies(body, &mut copies);
        }
        for (state_sig, next_sig) in &copies {
            if fsms.iter().any(|f| {
                design
                    .top
                    .signals
                    .get(*state_sig)
                    .map(|s| s.name.to_string() == f.state_signal)
                    .unwrap_or(false)
            }) {
                continue;
            }
            for process in &design.top.processes {
                let (pname, body) = match process {
                    Process::Combinational { name, body, .. }
                    | Process::CombReactive { name, body, .. } => (name.to_string(), body),
                    _ => continue,
                };
                for stmt in body {
                    let IrStmt::Case { expr, items, default, .. } = stmt else {
                        continue;
                    };
                    let IrExpr::Signal(sel, _) = expr else {
                        continue;
                    };
                    if *sel != *state_sig {
                        continue;
                    }
                    let Some(sig_info) = design.top.signals.get(*state_sig) else {
                        continue;
                    };
                    let (states, transitions, outputs) =
                        extract_from_case(items, default, *state_sig, *next_sig, design);
                    if transitions.is_empty() {
                        continue;
                    }
                    fsms.push(FsmInfo {
                        process_name: pname.clone(),
                        state_signal: sig_info.name.to_string(),
                        state_width: sig_info.width,
                        states,
                        transitions,
                        outputs,
                    });
                }
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
        if !f.outputs.is_empty() {
            out.push_str(&format!(
                "    outputs ({}): {}\n",
                f.outputs.len(),
                f.outputs
                    .iter()
                    .map(|o| format!(
                        "{}({})",
                        o.signal,
                        if o.kind == FsmOutputKind::Mealy { "Mealy" } else { "Moore" }
                    ))
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
            outputs: vec![],
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

    #[test]
    fn test_fsm_mealy_moore_outputs() {
        // state=0, out_moore <= 0 (konstanta → Moore),
        // out_mealy <= inp (input → Mealy). Design: state(0), inp(1),
        // out_moore(2), out_mealy(3). Item kedua upgrade out_moore ke
        // Mealy (menang) via BitSelect + FuncCall; write state dikecualikan.
        let item = |moore_rhs: IrExpr, mealy_from: usize, with_const: bool| IrCaseItem {
            labels: vec![IrExpr::Const(LogicVec::from_u64(0, 2))],
            body: {
                let mut b = vec![
                    IrStmt::NonBlockingAssign {
                        lhs: IrLValue::Signal(0, 2),
                        rhs: IrExpr::Const(LogicVec::from_u64(1, 2)),
                        delay: None,
                    },
                    IrStmt::BlockingAssign {
                        lhs: IrLValue::Signal(2, 1),
                        rhs: moore_rhs,
                        delay: None,
                    },
                    IrStmt::BlockingAssign {
                        lhs: IrLValue::Signal(3, 1),
                        rhs: IrExpr::Signal(mealy_from, 0),
                        delay: None,
                    },
                ];
                if with_const {
                    b.push(IrStmt::BlockingAssign {
                        lhs: IrLValue::Signal(4, 1),
                        rhs: IrExpr::Const(LogicVec::from_u64(1, 1)),
                        delay: None,
                    });
                }
                b
            },
        };
        let const0 = IrExpr::Const(LogicVec::from_u64(0, 1));
        let upgrade = IrExpr::FuncCall {
            func_name: Symbol::from("f"),
            args: vec![IrExpr::BitSelect(1, 0)],
        };
        let mut design = IrDesign::default();
        design.top.name = Symbol::from("fsm_mo");
        design.top.signals = vec![
            SignalInfo { name: Symbol::from("state"), width: 2, ..Default::default() },
            SignalInfo { name: Symbol::from("inp"), width: 1, ..Default::default() },
            SignalInfo { name: Symbol::from("out_moore"), width: 1, ..Default::default() },
            SignalInfo { name: Symbol::from("out_mealy"), width: 1, ..Default::default() },
            SignalInfo { name: Symbol::from("out_const"), width: 1, ..Default::default() },
        ];
        design.top.processes = vec![Process::Sequential {
            name: Symbol::from("sm"),
            clock: ClockEdge::PosEdge(1),
            reset: None,
            body: vec![IrStmt::Case {
                case_type: CaseType::Normal,
                expr: IrExpr::Signal(0, 0),
                items: vec![item(const0, 1, true), item(upgrade, 1, false)],
                default: vec![],
            }],
            iff: None,
        }];
        let fsms = extract_fsms(&design);
        assert_eq!(fsms.len(), 1, "{fsms:?}");
        let f = &fsms[0];
        // out_moore ditulis Moore di item 1 lalu Mealy (FuncCall atas inp)
        // di item 2 → Mealy menang.
        let moore = f.outputs.iter().find(|o| o.signal == "out_moore").expect("moore ada");
        assert_eq!(moore.kind, FsmOutputKind::Mealy);
        let mealy = f.outputs.iter().find(|o| o.signal == "out_mealy").expect("mealy ada");
        assert_eq!(mealy.kind, FsmOutputKind::Mealy);
        // State reg bukan output; urutan sort by nama.
        assert!(!f.outputs.iter().any(|o| o.signal == "state"));
        let names: Vec<&str> = f.outputs.iter().map(|o| o.signal.as_str()).collect();
        assert_eq!(names, vec!["out_const", "out_mealy", "out_moore"], "{names:?}");
        let oc = f.outputs.iter().find(|o| o.signal == "out_const").expect("const ada");
        assert_eq!(oc.kind, FsmOutputKind::Moore);
        let report = render_fsm_report(&[f.clone()]);
        assert!(report.contains("out_moore(Mealy)"), "{report}");
        assert!(report.contains("out_mealy(Mealy)"), "{report}");
    }

    #[test]
    fn test_fsm_two_process_classic() {
        // Pola 2-proses klasik: always_ff `state <= next` + always_comb
        // `case (state) { 0: next = 1; 1: next = 2; }`.
        // Sinyal: state(0), next(1).
        let item = |label: u64, to: u64| IrCaseItem {
            labels: vec![IrExpr::Const(LogicVec::from_u64(label, 2))],
            body: vec![IrStmt::BlockingAssign {
                lhs: IrLValue::Signal(1, 2),
                rhs: IrExpr::Const(LogicVec::from_u64(to, 2)),
                delay: None,
            }],
        };
        let mut design = IrDesign::default();
        design.top.name = Symbol::from("fsm_2p");
        design.top.signals = vec![
            SignalInfo { name: Symbol::from("state"), width: 2, ..Default::default() },
            SignalInfo { name: Symbol::from("next"), width: 2, ..Default::default() },
        ];
        design.top.processes = vec![
            Process::Sequential {
                name: Symbol::from("seq"),
                clock: ClockEdge::PosEdge(0),
                reset: None,
                body: vec![IrStmt::NonBlockingAssign {
                    lhs: IrLValue::Signal(0, 2),
                    rhs: IrExpr::Signal(1, 0),
                    delay: None,
                }],
                iff: None,
            },
            Process::Combinational {
                name: Symbol::from("comb"),
                sensitivity: vec![],
                body: vec![IrStmt::Case {
                    case_type: CaseType::Normal,
                    expr: IrExpr::Signal(0, 0),
                    items: vec![item(0, 1), item(1, 2)],
                    default: vec![],
                }],
            },
        ];
        let fsms = extract_fsms(&design);
        assert_eq!(fsms.len(), 1, "{fsms:?}");
        let f = &fsms[0];
        assert_eq!(f.state_signal, "state");
        assert_eq!(f.states, vec![0, 1, 2]);
        assert!(f.transitions.contains(&FsmTransition { from: Some(0), to: 1 }), "{f:?}");
        assert!(f.transitions.contains(&FsmTransition { from: Some(1), to: 2 }), "{f:?}");
        // Tanpa copy `state <= next`, comb saja bukan FSM.
        let mut design2 = design.clone();
        design2.top.processes.remove(0);
        assert!(extract_fsms(&design2).is_empty());
    }
}
