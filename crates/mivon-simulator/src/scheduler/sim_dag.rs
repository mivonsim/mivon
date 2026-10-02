//! SimulationDag — dependency graph untuk parallel process evaluation.
//!
//! # Strategi
//!
//! 1. Analisis sinyal yang di-**read** dan di-**write** oleh setiap process
//! 2. Bangun conflict graph: process A 🤝 process B jika A write sinyal yang B read/write
//! 3. Kelompokkan process independent ke dalam **layers** (topological sort)
//! 4. Evaluasi setiap layer secara **paralel** via rayon work-stealing
//!
//! # Zero-Lock Design
//!
//! Setiap process layer di-evaluasi pada sinyal array sendiri-sendiri.
//! Writes dikumpulkan sebagai `Vec<(SignalId, LogicVec)>` dan diterapkan
//! sekuensial setelah semua process dalam layer selesai — lock-free karena
//! tidak ada shared mutable state antar process dalam satu layer.

use mivon_ir::*;
use std::collections::HashSet;
use std::sync::Arc;

// ─── Signal Access Analysis ───

/// Hasil analisis akses sinyal untuk satu process.
#[derive(Debug, Clone, Default)]
pub struct SignalAccess {
    /// Signal IDs yang dibaca process ini.
    pub reads: HashSet<SignalId>,
    /// Signal IDs yang ditulis process ini.
    pub writes: HashSet<SignalId>,
    /// True bila ada akses yang TIDAK bisa di-resolve ke SignalId (HierRef,
    /// VirtualIfaceAccess, This, dst.) — process dengan flag ini TIDAK boleh
    /// memakai snapshot sparse (harus full snapshot agar tidak salah hasil).
    pub has_unresolved: bool,
}

/// Analyze which signals a process reads and writes.
pub(crate) fn analyze_process_access(process: &Process) -> SignalAccess {
    match process {
        Process::Combinational {
            sensitivity, body, ..
        }
        | Process::CombReactive {
            sensitivity, body, ..
        } => {
            let mut access = SignalAccess::default();
            // Sensitivity list = reads (trigger signals)
            for s in sensitivity {
                access.reads.insert(s.sig_id);
            }
            // Scan body for additional reads + all writes
            stmt_signal_access(body, &mut access);
            access
        }
        Process::Sequential {
            clock, reset, body, ..
        } => {
            let mut access = SignalAccess::default();
            // Clock signal = read
            match clock {
                ClockEdge::PosEdge(sid) | ClockEdge::NegEdge(sid) => {
                    access.reads.insert(*sid);
                }
                // F27: clock hierarkis (path Symbol via port interface) — belum
                // dikenal saat DAG build; akses tetap terwakili process lain.
                ClockEdge::PosEdgeHier(_) | ClockEdge::NegEdgeHier(_) => {}
            }
            // Reset signal = read
            if let Some(r) = reset {
                access.reads.insert(r.signal);
            }
            // Scan body for additional reads + writes
            stmt_signal_access(body, &mut access);
            access
        }
        Process::Initial { body, .. }
        | Process::Final { body, .. }
        | Process::AlwaysWithDelay { body, .. } => {
            let mut access = SignalAccess::default();
            stmt_signal_access(body, &mut access);
            access
        }
    }
}

/// Walk through IR statements to collect signal reads and writes.
fn stmt_signal_access(stmts: &[IrStmt], access: &mut SignalAccess) {
    for stmt in stmts {
        match stmt {
            IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                stmt_signal_access(inner, access);
            }
            IrStmt::BlockingAssign { lhs, rhs, .. }
            | IrStmt::NonBlockingAssign { lhs, rhs, .. } => {
                lvalue_signal_writes(lhs, access);
                expr_signal_reads(rhs, access);
            }
            IrStmt::Force { lvalue, rhs, .. } => {
                lvalue_signal_writes(lvalue, access);
                expr_signal_reads(rhs, access);
            }
            IrStmt::Release { lvalue } | IrStmt::Deassign { lvalue } => {
                lvalue_signal_writes(lvalue, access);
            }
            IrStmt::If {
                cond,
                true_branch,
                false_branch,
                ..
            } => {
                expr_signal_reads(cond, access);
                stmt_signal_access(true_branch, access);
                stmt_signal_access(false_branch, access);
            }
            IrStmt::Case {
                expr: case_expr,
                items,
                default,
                ..
            } => {
                expr_signal_reads(case_expr, access);
                for item in items {
                    for pat in &item.labels {
                        expr_signal_reads(pat, access);
                    }
                    stmt_signal_access(&item.body, access);
                }
                stmt_signal_access(default, access);
            }
            IrStmt::LoopFor {
                init,
                cond,
                step,
                body,
                ..
            } => {
                if let Some(init_stmt) = init {
                    stmt_signal_access(&[init_stmt.as_ref().clone()], access);
                }
                expr_signal_reads(cond, access);
                if let Some(step_stmt) = step {
                    stmt_signal_access(&[step_stmt.as_ref().clone()], access);
                }
                stmt_signal_access(body, access);
            }
            IrStmt::LoopWhile { cond, body, .. } => {
                expr_signal_reads(cond, access);
                stmt_signal_access(body, access);
            }
            IrStmt::LoopDoWhile { cond, body, .. } => {
                stmt_signal_access(body, access);
                expr_signal_reads(cond, access);
            }
            IrStmt::Repeat { count, body, .. } => {
                expr_signal_reads(count, access);
                stmt_signal_access(body, access);
            }
            IrStmt::Foreach {
                array_var, body, ..
            } => {
                expr_signal_reads(array_var, access);
                stmt_signal_access(body, access);
            }
            IrStmt::Delay { body, .. } => {
                stmt_signal_access(body, access);
            }
            IrStmt::Wait { cond, body, .. } => {
                expr_signal_reads(cond, access);
                stmt_signal_access(body, access);
            }
            IrStmt::EventControl { sigs, body, .. } => {
                for (sid, _) in sigs {
                    access.reads.insert(*sid);
                }
                stmt_signal_access(body, access);
            }
            IrStmt::EventTrigger { sig_id, .. } => {
                access.writes.insert(*sig_id);
            }
            IrStmt::SysCall { args, .. } => {
                for arg in args {
                    expr_signal_reads(arg, access);
                }
            }
            IrStmt::Assert {
                cond,
                pass_stmt,
                fail_stmt,
                disable_iff,
                ..
            }
            | IrStmt::Assume {
                cond,
                pass_stmt,
                fail_stmt,
                disable_iff,
                ..
            } => {
                expr_signal_reads(cond, access);
                if let Some(di) = disable_iff {
                    expr_signal_reads(di, access);
                }
                stmt_signal_access(pass_stmt, access);
                stmt_signal_access(fail_stmt, access);
            }
            // LANG-14: expect — baca cond + walk pass/fail stmts.
            IrStmt::Expect {
                cond,
                pass_stmt,
                fail_stmt,
                ..
            } => {
                expr_signal_reads(cond, access);
                stmt_signal_access(pass_stmt, access);
                stmt_signal_access(fail_stmt, access);
            }
            IrStmt::Cover {
                cond,
                pass_stmt,
                disable_iff,
                ..
            } => {
                expr_signal_reads(cond, access);
                if let Some(di) = disable_iff {
                    expr_signal_reads(di, access);
                }
                stmt_signal_access(pass_stmt, access);
            }
            IrStmt::WaitOrder { events, .. } => {
                for &sid in events {
                    access.reads.insert(sid);
                }
            }
            IrStmt::Fork { processes, .. } => {
                for p in processes {
                    stmt_signal_access(p, access);
                }
            }
            IrStmt::MethodCallStmt { obj, args, .. } => {
                expr_signal_reads(obj, access);
                for arg in args {
                    expr_signal_reads(arg, access);
                }
            }
            IrStmt::RandCase { items, .. } => {
                for (w, body) in items {
                    expr_signal_reads(w, access);
                    stmt_signal_access(body, access);
                }
            }
            IrStmt::RandSequence { productions, .. } => {
                for (_, items) in productions {
                    for (w, body) in items {
                        expr_signal_reads(w, access);
                        stmt_signal_access(body, access);
                    }
                }
            }
            // Statements that don't access signals
            IrStmt::SysFinish
            | IrStmt::Null
            | IrStmt::Break
            | IrStmt::Continue
            | IrStmt::WaitFork
            | IrStmt::Disable { .. } => {}
        }
    }
}

/// Extract signal IDs written by an lvalue.
fn lvalue_signal_writes(lvalue: &IrLValue, access: &mut SignalAccess) {
    match lvalue {
        IrLValue::Signal(sig_id, _)
        | IrLValue::RangeSelect(sig_id, _, _)
        | IrLValue::BitSelect(sig_id, _) => {
            access.writes.insert(*sig_id);
        }
        IrLValue::ArrayIndex { sig_id, .. }
        | IrLValue::ArrayRangeSelect { sig_id, .. }
        | IrLValue::ArrayBitSelect { sig_id, .. }
        | IrLValue::ExprPartSelect { sig_id, .. }
        | IrLValue::ObjectField { sig_id, .. } => {
            access.writes.insert(*sig_id);
        }
        IrLValue::Concat(items) => {
            for item in items {
                lvalue_signal_writes(item, access);
            }
        }
        // Lvalue hierarkis belum ter-resolve di fase ini → proses memakai
        // path full-snapshot (bukan sparse) agar perilaku tidak berubah.
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } => {
            access.has_unresolved = true;
        }
    }
}

/// Extract signal IDs read by an expression.
fn expr_signal_reads(expr: &IrExpr, access: &mut SignalAccess) {
    match expr {
        IrExpr::Signal(sig_id, _)
        | IrExpr::RangeSelect(sig_id, _, _)
        | IrExpr::BitSelect(sig_id, _) => {
            access.reads.insert(*sig_id);
        }
        IrExpr::ExprRangeSelect(inner, _, _) | IrExpr::ExprBitSelect(inner, _) => {
            expr_signal_reads(inner, access);
        }
        IrExpr::ExprPartSelect(inner, base, width) => {
            expr_signal_reads(inner, access);
            expr_signal_reads(base, access);
            expr_signal_reads(width, access);
        }
        IrExpr::ArrayIndex { sig_id, index, .. } => {
            // ArrayIndex reads signal + index expression
            access.reads.insert(*sig_id);
            expr_signal_reads(index, access);
        }
        IrExpr::VirtualIfaceAccess { .. } => {
            // Virtual interface access reads resolved at runtime
        }
        IrExpr::Concat(exprs) | IrExpr::StreamingConcat { slices: exprs, .. } => {
            for e in exprs {
                expr_signal_reads(e, access);
            }
        }
        IrExpr::Replicate(_, inner) => {
            expr_signal_reads(inner, access);
        }
        IrExpr::UnaryOp(_, inner) => {
            expr_signal_reads(inner, access);
        }
        IrExpr::BinaryOp(_, lhs, rhs) | IrExpr::Cond(_, lhs, rhs) => {
            expr_signal_reads(lhs, access);
            expr_signal_reads(rhs, access);
        }
        IrExpr::Signed(inner) | IrExpr::Cast { expr: inner, .. } => {
            expr_signal_reads(inner, access);
        }
        IrExpr::Inside { expr: inner, list } => {
            expr_signal_reads(inner, access);
            for item in list {
                expr_signal_reads(item, access);
            }
        }
        IrExpr::InsideRange {
            expr: inner,
            lo,
            hi,
        } => {
            expr_signal_reads(inner, access);
            expr_signal_reads(lo, access);
            expr_signal_reads(hi, access);
        }
        IrExpr::Dist { expr: inner, .. } => {
            expr_signal_reads(inner, access);
        }
        IrExpr::DpiCall { args, .. }
        | IrExpr::SysFunc { args, .. }
        | IrExpr::NewCall { args, .. } => {
            for arg in args {
                expr_signal_reads(arg, access);
            }
        }
        IrExpr::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            expr_signal_reads(obj, access);
            for arg in args {
                expr_signal_reads(arg, access);
            }
            if let Some(wc) = with_clause {
                expr_signal_reads(wc, access);
            }
        }
        IrExpr::MemberAccess { obj, .. } => {
            expr_signal_reads(obj, access);
        }
        IrExpr::UdpLookup { args, .. } => {
            for arg in args {
                expr_signal_reads(arg, access);
            }
        }
        IrExpr::FuncCall { args, .. } => {
            for arg in args {
                expr_signal_reads(arg, access);
            }
        }
        // RMW: `i++` membaca operand DAN menulis balik ke lvalue-nya.
        IrExpr::IncDec { read, lv, .. } => {
            expr_signal_reads(read, access);
            lvalue_signal_writes(lv, access);
        }
        // These don't access signals — tapi beberapa TIDAK bisa di-resolve ke
        // SignalId (HierRef/VirtualIfaceAccess/This/VifBinding) → tandai
        // has_unresolved agar proses tidak dipakai di snapshot sparse.
        IrExpr::Const(_) | IrExpr::RealConst(_) | IrExpr::FillLit(_) | IrExpr::String(_) => {}
        IrExpr::This | IrExpr::HierRef(_) | IrExpr::VifBinding { .. } => {
            access.has_unresolved = true;
        }
    }
}

// ─── SimulationDag ───

/// Process dependency graph untuk parallel evaluation.
///
/// Dibangun sekali di awal simulasi berdasarkan analisis sinyal.
/// Menyimpan layers process yang bisa di-evaluasi paralel.
pub struct SimulationDag {
    /// Signal access patterns per process (indexed by process ID).
    access: Vec<SignalAccess>,
    /// Pre-computed topological layers.
    /// Setiap layer berisi process IDs yang bisa dijalankan paralel.
    layers: Vec<Vec<usize>>,
    /// Total number of processes.
    num_processes: usize,
}

impl SimulationDag {
    /// Build a SimulationDag from an IrDesign's processes.
    ///
    /// # Complexity
    /// O(P²) untuk conflict detection, di mana P = jumlah processes.
    /// Untuk 10K processes, ini ~100M checks — masih reasonable untuk startup.
    pub fn build(design: &IrDesign) -> Self {
        let num_processes = design.top.processes.len();
        if num_processes == 0 {
            return SimulationDag {
                access: Vec::new(),
                layers: Vec::new(),
                num_processes: 0,
            };
        }

        // Phase 1: analyze signal access for each process
        let mut access: Vec<SignalAccess> = Vec::with_capacity(num_processes);
        for process in &design.top.processes {
            access.push(analyze_process_access(process));
        }

        // Phase 2: build conflict graph
        // conflict[i] = set of process IDs that conflict with process i
        let mut conflict: Vec<HashSet<usize>> = vec![HashSet::new(); num_processes];
        for i in 0..num_processes {
            for j in (i + 1)..num_processes {
                if processes_conflict(&access[i], &access[j]) {
                    conflict[i].insert(j);
                    conflict[j].insert(i);
                }
            }
        }

        // Phase 3: greedy topological layering
        // Algoritma: process yang tidak punya conflict → layer 0
        // Hapus mereka, lalu cari process yang tersisa yang tidak conflict
        // dengan sesama process yang tersisa → layer 1, dst.
        let layers = greedy_layering(num_processes, &conflict);

        SimulationDag {
            access,
            layers,
            num_processes,
        }
    }

    /// Number of layers in the DAG.
    pub fn num_layers(&self) -> usize {
        self.layers.len()
    }

    /// Get all layers (process IDs per layer).
    pub fn layers(&self) -> &[Vec<usize>] {
        &self.layers
    }

    /// Get signal access for a specific process.
    pub fn process_access(&self, pid: usize) -> Option<&SignalAccess> {
        self.access.get(pid)
    }

    /// Number of processes in the DAG.
    pub fn num_processes(&self) -> usize {
        self.num_processes
    }

    /// Estimate parallelism: average number of independent processes per layer.
    pub fn avg_parallelism(&self) -> f64 {
        if self.layers.is_empty() {
            return 0.0;
        }
        let total: usize = self.layers.iter().map(|l| l.len()).sum();
        total as f64 / self.layers.len() as f64
    }
}

/// Check if two processes conflict (cannot be evaluated in parallel).
///
/// Konflik terjadi jika:
/// - Process A **menulis** sinyal yang dibaca Process B (RAW hazard)
/// - Process A **menulis** sinyal yang ditulis Process B (WAW hazard)
/// - Process A **membaca** sinyal yang ditulis Process B (WAR hazard)
///
/// Reads-reads tidak konflik — dua process bisa membaca sinyal yang sama.
fn processes_conflict(a: &SignalAccess, b: &SignalAccess) -> bool {
    // A writes, B reads (RAW)
    if a.writes.iter().any(|sig| b.reads.contains(sig)) {
        return true;
    }
    // B writes, A reads (WAR)
    if b.writes.iter().any(|sig| a.reads.contains(sig)) {
        return true;
    }
    // A writes, B writes (WAW)
    if a.writes.iter().any(|sig| b.writes.contains(sig)) {
        return true;
    }
    false
}

/// Greedy topological layering algorithm.
///
/// 1. Mulai dengan semua process dalam pool
/// 2. Layer 0: semua process yang tidak conflict satu sama lain
/// 3. Hapus process layer 0 dari pool
/// 4. Layer 1: dari sisa pool, cari maximal set yang tidak conflict
/// 5. Ulangi sampai pool kosong
fn greedy_layering(num_processes: usize, conflict: &[HashSet<usize>]) -> Vec<Vec<usize>> {
    let mut remaining: HashSet<usize> = (0..num_processes).collect();
    let mut layers: Vec<Vec<usize>> = Vec::new();

    while !remaining.is_empty() {
        let mut layer = Vec::new();
        let mut in_layer = HashSet::new();
        let mut candidates: Vec<usize> = remaining.iter().copied().collect();
        // Sort untuk determinisme
        candidates.sort_unstable();

        for &pid in &candidates {
            // Check if pid conflicts with any process already in the layer
            let has_conflict = conflict[pid].iter().any(|other| in_layer.contains(other));
            if !has_conflict {
                layer.push(pid);
                in_layer.insert(pid);
            }
        }

        // Remove layer processes from remaining pool
        for &pid in &layer {
            remaining.remove(&pid);
        }

        if layer.is_empty() {
            // Fallback: no independent processes found, put one remaining in layer
            if let Some(&pid) = remaining.iter().next() {
                layer.push(pid);
                remaining.remove(&pid);
            }
        }

        layers.push(layer);
    }

    layers
}

/// Layer indices in topological order (for debugging).
pub fn layer_to_string(layers: &[Vec<usize>]) -> String {
    layers
        .iter()
        .enumerate()
        .map(|(i, layer)| format!("  Layer {}: {} processes", i, layer.len()))
        .collect::<Vec<_>>()
        .join("\n")
}

// ═══════════════════════════════════════════════════════════════════════════
// Parallel Process Evaluation
// ═══════════════════════════════════════════════════════════════════════════

/// Evaluate a single process body in parallel context.
/// Menggunakan sinyal snapshot (read-only) dan mengumpulkan writes.
///
/// Mirip dengan evaluate_stmt_block_parallel() di src/simulator/parallel.rs
/// tapi bekerja dengan body process standalone.
pub fn evaluate_process_body(
    body: &[IrStmt],
    signals: &[Arc<LogicVec>],
    writes: &mut Vec<(SignalId, LogicVec)>,
    // For fill-lit width resolution, pass signal count as signal_ref
) -> Result<(), mivon_core::error::SimError> {
    // Reuse the existing parallel-safe evaluator. Per-process overlay kosong
    // (copy-on-write) — tidak ada clone seluruh sinyal per process (SIM-28).
    let signal_count = signals.len();
    let identity: Vec<Option<usize>> = (0..signal_count).map(Some).collect();
    let mut overlay = std::collections::HashMap::new();
    let mut view = crate::simulator::parallel::SignalView::new(signals, &identity, &mut overlay);

    // sig_info kosong (scheduler ini tidak punya SignalInfo; jalur tak
    // terpakai — exported tapi tidak ada caller) → is_signed_expr false →
    // perilaku unsigned default seperti sebelumnya.
    crate::simulator::parallel::evaluate_stmt_block_parallel(body, &mut view, writes, &[])?;
    Ok(())
}

/// Evaluate a layer of processes in parallel using rayon work-stealing.
///
/// # Lock-Free Design
///
/// Setiap process bekerja pada snapshot sinyal sendiri (read-only).
/// Writes dikumpulkan dan digabungkan setelah semua process dalam layer selesai —
/// tanpa mutex/lock karena tidak ada shared mutable state.
///
/// # Returns
/// Kumpulan (SignalId, LogicVec) dari semua writes, siap diaplikasikan.
pub fn evaluate_layer_parallel(
    layer: &[usize],
    processes: &[Process],
    signals: &[Arc<LogicVec>],
) -> Result<Vec<(SignalId, LogicVec)>, mivon_core::error::SimError> {
    use rayon::prelude::*;

    // Evaluate each process in the layer in parallel
    let identity: Vec<Option<usize>> = (0..signals.len()).map(Some).collect();
    let results: Vec<Result<Vec<(SignalId, LogicVec)>, mivon_core::error::SimError>> = layer
        .par_iter()
        .map(
            |pid| -> Result<Vec<(SignalId, LogicVec)>, mivon_core::error::SimError> {
                if *pid >= processes.len() {
                    return Ok(Vec::new());
                }
                let process = &processes[*pid];
                let body = match process {
                    Process::Combinational { body, .. } | Process::CombReactive { body, .. } => {
                        body
                    }
                    Process::Initial { body, .. }
                    | Process::Final { body, .. }
                    | Process::AlwaysWithDelay { body, .. } => body,
                    Process::Sequential { body, .. } => body,
                };

                let mut writes: Vec<(SignalId, LogicVec)> = Vec::new();
                let mut overlay = std::collections::HashMap::new();
                let mut view =
                    crate::simulator::parallel::SignalView::new(signals, &identity, &mut overlay);
                crate::simulator::parallel::evaluate_stmt_block_parallel(
                    body,
                    &mut view,
                    &mut writes,
                    &[],
                )?;
                Ok(writes)
            },
        )
        .collect();

    // Merge all writes
    let mut all_writes: Vec<(SignalId, LogicVec)> = Vec::new();
    for result in results {
        let writes = result?;
        all_writes.extend(writes);
    }

    Ok(all_writes)
}

/// Evaluate a layer of processes using cloned bodies (no design reference needed).
///
/// # Lock-Free Design
///
/// Setiap process bekerja pada snapshot sinyal sendiri (clone).
/// Writes dikumpulkan dan digabungkan setelah semua process dalam layer selesai.
pub fn evaluate_bodies_parallel(
    layer_pids: &[&usize],
    body_map: &std::collections::HashMap<usize, Vec<IrStmt>>,
    signals: &[Arc<LogicVec>],
    sig_info: &[mivon_ir::SignalInfo],
) -> Result<Vec<(SignalId, LogicVec)>, mivon_core::error::SimError> {
    use rayon::prelude::*;

    // SIM-28: per-process overlay kosong — tanpa clone seluruh sinyal.
    let identity: Vec<Option<usize>> = (0..signals.len()).map(Some).collect();
    let results: Vec<Result<Vec<(SignalId, LogicVec)>, mivon_core::error::SimError>> = layer_pids
        .par_iter()
        .map(
            |&&pid| -> Result<Vec<(SignalId, LogicVec)>, mivon_core::error::SimError> {
                let body = match body_map.get(&pid) {
                    Some(b) => b,
                    None => return Ok(Vec::new()),
                };

                let mut writes: Vec<(SignalId, LogicVec)> = Vec::new();
                let mut overlay = std::collections::HashMap::new();
                let mut view =
                    crate::simulator::parallel::SignalView::new(signals, &identity, &mut overlay);
                crate::simulator::parallel::evaluate_stmt_block_parallel(
                    body,
                    &mut view,
                    &mut writes,
                    sig_info,
                )?;
                Ok(writes)
            },
        )
        .collect();

    let mut all_writes: Vec<(SignalId, LogicVec)> = Vec::new();
    for result in results {
        let writes = result?;
        all_writes.extend(writes);
    }

    Ok(all_writes)
}

/// Check if a process type is suitable for DAG-parallel evaluation.
/// Sequential and AlwaysWithDelay processes have timing/state that
/// makes parallel evaluation unsafe — skip them.
///
/// Initial blocks (termasuk decl-init `string name = "..."`) TIDAK
/// di-paralelkan: one-shot (manfaat paralel minimal) tapi resiko semantik
/// besar — parallel.rs meng-skip `@(posedge ...)`, `$display`, `$finish`,
/// dan representasi string/real beda jalur → hasil divergen (fuzzer
/// differential: `string name` kosong → `$sformatf("hello %s", name)`
/// = "hello "). Serial = referensi benar.
pub fn is_process_parallelizable(process: &Process) -> bool {
    matches!(
        process,
        Process::Combinational { .. } | Process::CombReactive { .. }
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::intern::Symbol;
    use std::collections::HashMap;

    #[test]
    fn test_signal_access_analysis() {
        // Process: always_comb a = b & c;
        let body = vec![IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs: IrExpr::BinaryOp(
                BinaryIrOp::BitAnd,
                Box::new(IrExpr::Signal(1, 8)),
                Box::new(IrExpr::Signal(2, 8)),
            ),
            delay: None,
        }];
        let process = Process::Combinational {
            name: Symbol::intern("test"),
            sensitivity: vec![SignalSensitivity::whole(1), SignalSensitivity::whole(2)],
            body,
        };
        let access = analyze_process_access(&process);
        // Reads: sensitivity[1,2] + rhs signals[1,2]
        assert!(access.reads.contains(&1));
        assert!(access.reads.contains(&2));
        // Writes: lhs signal[0]
        assert!(access.writes.contains(&0));
    }

    #[test]
    fn test_no_conflict_read_only() {
        let a = SignalAccess {
            reads: [1, 2].into_iter().collect(),
            writes: HashSet::new(),
            has_unresolved: false,
        };
        let b = SignalAccess {
            reads: [2, 3].into_iter().collect(),
            writes: HashSet::new(),
            has_unresolved: false,
        };
        // Both only read — no conflict
        assert!(!processes_conflict(&a, &b));
    }

    #[test]
    fn test_conflict_write_read() {
        let a = SignalAccess {
            reads: HashSet::new(),
            writes: [5].into_iter().collect(),
            has_unresolved: false,
        };
        let b = SignalAccess {
            reads: [5].into_iter().collect(),
            writes: HashSet::new(),
            has_unresolved: false,
        };
        // A writes signal 5, B reads it → conflict
        assert!(processes_conflict(&a, &b));
    }

    #[test]
    fn test_conflict_write_write() {
        let a = SignalAccess {
            reads: HashSet::new(),
            writes: [10].into_iter().collect(),
            has_unresolved: false,
        };
        let b = SignalAccess {
            reads: HashSet::new(),
            writes: [10].into_iter().collect(),
            has_unresolved: false,
        };
        // Both write signal 10 → conflict
        assert!(processes_conflict(&a, &b));
    }

    #[test]
    fn test_greedy_layering_independent() {
        // 3 processes, no conflicts → all in layer 0
        let num = 3;
        let conflict = vec![HashSet::new(), HashSet::new(), HashSet::new()];
        let layers = greedy_layering(num, &conflict);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].len(), 3);
    }

    #[test]
    fn test_greedy_layering_chain() {
        // Chain: 0→1→2 (each depends on previous)
        // Process 0 dan 2 independent (no direct conflict via shared signal)
        // Mereka bisa di-layer yang sama: Layer 0 = [0, 2], Layer 1 = [1]
        let num = 3;
        let mut conflict = vec![HashSet::new(); 3];
        conflict[1].insert(0);
        conflict[0].insert(1);
        conflict[2].insert(1);
        conflict[1].insert(2);
        let layers = greedy_layering(num, &conflict);
        // 2 layers: [0, 2] dan [1] (0 dan 2 independent)
        assert_eq!(
            layers.len(),
            2,
            "expected 2 layers, got {}: {:?}",
            layers.len(),
            layers
        );
        // Layer 0 should have 2 processes (0 and 2)
        assert_eq!(layers[0].len(), 2, "layer 0 should have 2 processes");
        // Layer 1 should have 1 process (1)
        assert_eq!(layers[1].len(), 1, "layer 1 should have 1 process");
    }

    #[test]
    fn test_build_empty_design() {
        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("empty"),
                signals: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                inouts: Vec::new(),
                processes: Vec::new(),
                sub_instances: Vec::new(),
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: Vec::new(),
            dpi_imports: Vec::new(),
            hier_signal_map: HashMap::new(),
            udp_defs: Vec::new(),
            specify_items: Vec::new(),
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };
        let dag = SimulationDag::build(&design);
        assert_eq!(dag.num_layers(), 0);
        assert_eq!(dag.num_processes(), 0);
    }

    #[test]
    fn test_layer_to_string() {
        let layers = vec![vec![0, 1], vec![2]];
        let s = layer_to_string(&layers);
        assert!(s.contains("Layer 0: 2 processes"));
        assert!(s.contains("Layer 1: 1 processes"));
    }

    #[test]
    fn test_is_process_parallelizable() {
        let comb = Process::Combinational {
            name: Symbol::intern("c"),
            sensitivity: vec![],
            body: vec![],
        };
        let seq = Process::Sequential {
            name: Symbol::intern("s"),
            clock: ClockEdge::PosEdge(0),
            reset: None,
            body: vec![],
            iff: None,
        };
        assert!(is_process_parallelizable(&comb));
        assert!(!is_process_parallelizable(&seq));
    }

    #[test]
    fn test_build_with_conflicts() {
        // Create processes that share signals
        let body_a = vec![IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs: IrExpr::Signal(1, 8),
            delay: None,
        }];
        let body_b = vec![IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(1, 8),
            rhs: IrExpr::Signal(0, 8),
            delay: None,
        }];
        let body_c = vec![IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(2, 8),
            rhs: IrExpr::Signal(3, 8),
            delay: None,
        }];

        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::intern("a"),
                        width: 8,
                        ..default_signal_info()
                    },
                    SignalInfo {
                        name: Symbol::intern("b"),
                        width: 8,
                        ..default_signal_info()
                    },
                    SignalInfo {
                        name: Symbol::intern("c"),
                        width: 8,
                        ..default_signal_info()
                    },
                    SignalInfo {
                        name: Symbol::intern("d"),
                        width: 8,
                        ..default_signal_info()
                    },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![
                    Process::Combinational {
                        name: Symbol::intern("p0"),
                        sensitivity: vec![SignalSensitivity::whole(1)],
                        body: body_a,
                    },
                    Process::Combinational {
                        name: Symbol::intern("p1"),
                        sensitivity: vec![SignalSensitivity::whole(0)],
                        body: body_b,
                    },
                    Process::Combinational {
                        name: Symbol::intern("p2"),
                        sensitivity: vec![SignalSensitivity::whole(3)],
                        body: body_c,
                    },
                ],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: Vec::new(),
            dpi_imports: Vec::new(),
            hier_signal_map: HashMap::new(),
            udp_defs: Vec::new(),
            specify_items: Vec::new(),
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let dag = SimulationDag::build(&design);
        // p0 dan p1 conflict (a↔b dependency)
        // p2 independent (c,d signals)
        // Expected: layer 0 = [p0, p2] or [p1, p2], then layer 1 = remaining
        assert_eq!(dag.num_processes(), 3);
        assert!(
            dag.num_layers() >= 2,
            "should have at least 2 layers, got {}",
            dag.num_layers()
        );
        // Each layer should have at most 2 processes
        for layer in dag.layers() {
            assert!(layer.len() <= 2, "layer too large: {:?}", layer);
        }
    }

    fn default_signal_info() -> SignalInfo {
        SignalInfo {
            name: Symbol::intern(""),
            width: 1,
            kind: SignalKind::Wire,
            net_type: NetType::Wire,
            multi_driver: false,
            init_val: LogicVec::new(1),
            array_depth: 0,
            elem_width: 1,
            array_dims: vec![],
            class_name: None,
            is_string: false,
            is_real: false,
            is_mailbox: false,
            is_semaphore: false,
            is_2state: false,
            is_dynamic: false,
            is_queue: false,
            is_associative: false,
            is_signed: false,
            is_const: false,
            msb: 0,
            lsb: 0,
            struct_fields: vec![],
            packed_dims: vec![],
            delay_rise: None,
            delay_fall: None,
            iface_type: None,
            iface_modport: None,
        }
    }
}
