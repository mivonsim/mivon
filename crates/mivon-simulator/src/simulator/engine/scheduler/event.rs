use super::super::SimulationEngine;
use crate::simulator::parallel;
use crate::simulator::types::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_ir::*;
use parallel::ParallelWrite;
use std::sync::Arc;

/// Apakah sensitivity process terpenuhi oleh perubahan signal `changed`.
/// Entry range (msb/lsb Some) hanya terpicu bila SLICE tsb berubah; entry
/// whole (None) terpicu pada perubahan apa pun.
fn sensitivity_triggered(
    sensitivity: &[SignalSensitivity],
    changed: &[(usize, LogicVec, LogicVec)],
) -> bool {
    changed.iter().any(|(id, old, new)| {
        sensitivity.iter().any(|s| {
            if s.sig_id != *id {
                return false;
            }
            match (s.msb, s.lsb) {
                (Some(m), Some(l)) => {
                    let (lo, hi) = (l.min(m), m.max(l));
                    let a: &[LogicVal] = old.bits.get(lo..=hi).unwrap_or(&[]);
                    let b: &[LogicVal] = new.bits.get(lo..=hi).unwrap_or(&[]);
                    a != b
                }
                _ => true,
            }
        })
    })
}

/// Kandidat sequential triggered utk batch paralel Par-02 (tahap 3).
/// Tahap 4: pub(crate) — dipakai juga oleh jalur clock-domain
/// (core.rs evaluate_clock_domain Step 1).
pub(crate) struct SeqCand {
    pub(crate) pid: usize,
    pub(crate) clock_trigger: bool,
    pub(crate) iff: Option<mivon_ir::IrExpr>,
    pub(crate) async_trig: bool,
    pub(crate) fused: Option<crate::scheduler::clock_domain::ClockDomain>,
    pub(crate) access: crate::scheduler::sim_dag::SignalAccess,
    pub(crate) iff_reads: std::collections::HashSet<usize>,
    pub(crate) blocking_only: bool,
    pub(crate) own_blocking: std::collections::HashSet<usize>,
    pub(crate) batchable: bool,
}

impl SeqCand {
    /// Klasifikasi tahap 3 utk satu Sequential pid (base all-triggered:
    /// clock_trigger=false/iff=None/async_trig=false/fused=None — pemanggil
    /// event.rs mengisi field trigger; jalur domain memakai apa adanya).
    pub(crate) fn classify(engine: &SimulationEngine, pid: usize) -> SeqCand {
        let access =
            crate::scheduler::sim_dag::analyze_process_access(&engine.design.top.processes[pid]);
        let kinds = engine
            .process_body_arcs
            .get(&pid)
            .and_then(|b| crate::scheduler::sim_dag::body_write_kinds(b.as_slice()));
        let use_dag = engine.use_dag_parallel;
        let (blocking_only, own_blocking, batchable) = match kinds {
            Some((bw, nw)) if nw.is_empty() && use_dag => (true, bw, true),
            Some((bw, nw)) if bw.is_empty() && use_dag => {
                let ok = access.reads.is_disjoint(&nw);
                (false, bw, ok)
            }
            _ => (false, Default::default(), false),
        };
        let batchable = batchable && !access.has_call_expr && !access.has_unresolved;
        SeqCand {
            pid,
            clock_trigger: false,
            iff: None,
            async_trig: false,
            fused: None,
            access,
            iff_reads: Default::default(),
            blocking_only,
            own_blocking,
            batchable,
        }
    }
}
impl SimulationEngine {
    pub(crate) fn process_event(&mut self, event: EventKind, t: usize) -> Result<(), SimError> {
        self.current_time = t as u64;
        match event {
            EventKind::EvalProcess(pid) => {
                if pid >= self.design.top.processes.len() {
                    return Ok(());
                }
                // SIM-25: catat evaluasi process untuk performance dashboard
                self.sim_perf.counters.processes_evaluated += 1;
                let process = self.design.top.processes[pid].clone();

                // Set runtime context: process name + instance path
                let pname = match &process {
                    Process::Combinational { name, .. }
                    | Process::CombReactive { name, .. }
                    | Process::Sequential { name, .. }
                    | Process::Initial { name, .. }
                    | Process::Final { name, .. }
                    | Process::AlwaysWithDelay { name, .. } => name.as_str(),
                };
                self.current_process_name = Some(pname.to_string());
                self.current_instance_path = Some(self.design.top.name.to_string());

                let kind = match &process {
                    Process::Combinational { .. } => "always_comb",
                    Process::CombReactive { .. } => "always_comb_reactive",
                    Process::Sequential { .. } => "always_ff",
                    Process::Initial { .. } => "initial",
                    Process::Final { .. } => "final",
                    Process::AlwaysWithDelay { .. } => "always",
                };
                crate::dbg_sim!(
                    1,
                    "t={} delta={} eval pid={} kind={} '{}'",
                    t,
                    self.current_delta,
                    pid,
                    kind,
                    pname
                );

                match &process {
                    Process::Initial { body, .. } => {
                        if self.state.time == 0 {
                            self.disable_pending = None;
                            self.evaluate_block_with_delay(body)?;
                        }
                    }
                    Process::AlwaysWithDelay { delay, body, .. } => {
                        self.ensure_events(t);
                        self.disable_pending = None;
                        // IEEE 1800: `always #N stmt;` menunda N unit SEBELUM
                        // eksekusi pertama — body TIDAK dijalankan di t=0
                        // (hanya menjadwalkan evaluasi pertama di t=N). Tanpa
                        // ini `always #5 clk = ~clk;` meng-toggle clk di t=0
                        // (fase bergeser → hitungan salah). Evaluasi berikutnya
                        // (t>0) menjalankan body lalu menjadwalkan t+N.
                        if t > 0 {
                            self.evaluate_block_with_delay(body)?;
                        }
                        let next_t = t + *delay as usize;
                        self.ensure_events(next_t);
                        self.push_event(
                            next_t,
                            RegionEvent {
                                region: EventRegion::Active,
                                event: EventKind::EvalProcess(pid),
                            },
                        );
                    }
                    Process::Combinational { body, .. } => {
                        // Try MIR JIT for compiled-code execution path
                        // If use_mir_jit is false, always fall back to interpreted
                        if !self.use_mir_jit || !self.try_evaluate_mir_jit(pid, body)? {
                            self.evaluate_stmt_block(body)?;
                        }
                    }
                    Process::CombReactive { body, .. } => {
                        // Try MIR JIT for compiled-code execution path
                        // If use_mir_jit is false, always fall back to interpreted
                        if !self.use_mir_jit || !self.try_evaluate_mir_jit(pid, body)? {
                            self.evaluate_stmt_block(body)?;
                        }
                    }
                    Process::Sequential { body, .. }
                        // Try MIR JIT for always_ff blocks (edge-triggered)
                        // JIT handles the combinational body; scheduler handles edge wakeup
                        if (!self.use_mir_jit || !self.try_evaluate_mir_jit(pid, body)?) => {
                            self.evaluate_stmt_block(body)?;
                        }
                    _ => {}
                }
            }
            EventKind::ContinueBlock(cont) => {
                self.ensure_events(t);
                // LANG-30: branch fork yang di-disable via `disable fork` —
                // skip eksekusi body, langsung decrement (branch mati).
                if let Some(fid) = cont.fork_id {
                    if fid < self.fork_groups.len() && self.fork_groups[fid].disabled {
                        self.fork_decrement(fid)?;
                        return Ok(());
                    }
                }
                // F47: kontinuasi ini adalah titik resume branch yang memuat
                // label target `disable worker` dari branch lain — branch mati
                // di sini (sisa statement tidak dieksekusi) dan penanda
                // dikonsumsi. Tanpa ini `disable_pending` lama bocor dan
                // loop-top guard menelan continuation `fork...join`.
                if let Some(target) = self.disable_cross {
                    if cont.named_labels.contains(&target) {
                        if std::env::var("DBG_DIS").is_ok() {
                            eprintln!(
                                "[DBG-DIS] consume-cross '{}' kills cont labels={:?} fork={:?} nstmts={}",
                                target.as_str(),
                                cont.named_labels.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                                cont.fork_id,
                                cont.stmts_to_exec.len()
                            );
                        }
                        self.disable_cross = None;
                        if let Some(fid) = cont.fork_id {
                            self.fork_decrement(fid)?;
                        }
                        return Ok(());
                    }
                }
                // LANG-29: restore nama proses saat suspend — ContinueBlock
                // diproses di luar EvalProcess sehingga current_process_name
                // bisa menunjuk proses lain; `wait fork` (dan fitur berbasis
                // nama proses lain) butuh konteks yang benar setelah resume.
                if let Some(pn) = &cont.process_name {
                    self.current_process_name = Some(pn.clone());
                }
                let all_consumed =
                    self.evaluate_block_with_delay_fork(&cont.stmts_to_exec, cont.fork_id)?;
                // Detect natural process completion: when a continuation runs to completion (all_consumed)
                // and has a stored process_id, mark that process as Finished and trigger await continuations
                if all_consumed {
                    if let Some(pid) = cont.process_id {
                        if let Some(pi) = self.process_map.get_mut(&pid) {
                            if pi.status == ProcessStatus::Running {
                                pi.status = ProcessStatus::Finished;
                                let conts = std::mem::take(&mut pi.await_continuations);
                                for c in conts {
                                    self.evaluate_block_with_delay(&c)?;
                                }
                            }
                        }
                    }
                }
                if let Some(fid) = cont.fork_id {
                    if all_consumed {
                        self.fork_decrement(fid)?;
                    }
                }
            }
            EventKind::ContinueAstBlock(stmts, fork_id, this_opt, method_opt) => {
                // LANG-30: branch AST fork yang di-disable — skip eksekusi.
                if let Some(fid) = fork_id {
                    if fid < self.fork_groups.len() && self.fork_groups[fid].disabled {
                        self.fork_decrement(fid)?;
                        return Ok(());
                    }
                }
                // F18: kontinuasi task/method UVM setelah delay — restore konteks
                // `this` + method yang disimpan saat suspend (sama seperti pola
                // PendingAstEventControl). Tanpa ini, body task yang dijalankan
                // via execute_phases/run_test (`run_phase` dkk) kehilangan
                // current_this → `this.field` error "used outside of class method".
                self.ensure_events(t);
                let old_this = self.current_this;
                let old_method = self.current_method;
                self.current_this = this_opt;
                self.current_method = method_opt;
                if std::env::var("DBG_UVM").is_ok() {
                    eprintln!(
                        "[DBG-F26] resume ContinueAstBlock fid={:?} nstmts={}",
                        fork_id,
                        stmts.len()
                    );
                }
                let all_consumed = self.evaluate_ast_block_with_delay_fork(&stmts, fork_id)?;
                // F35 review: return di branch fork (illegal SV tapi parseable)
                // menandai ast_return_pending — clear di sini agar tidak bocor
                // ke evaluasi blok lain.
                self.ast_return_pending = false;
                if std::env::var("DBG_UVM").is_ok() {
                    eprintln!(
                        "[DBG-F26] resume done fid={:?} consumed={}",
                        fork_id, all_consumed
                    );
                }
                // F21: fork_decrement SEBELUM restore konteks — bila branch ini
                // adalah branch TERAKHIR yang selesai, fork_finish mengeksekusi
                // continuation AST setelah join/join_any, yang masih milik
                // method ini (butuh current_this/current_method yang sama).
                // Sebelumnya restore terjadi duluan → cont AST dieksekusi tanpa
                // konteks → field class gagal resolve (warning RT0001 + 0).
                if let Some(fid) = fork_id {
                    if all_consumed {
                        self.fork_decrement(fid)?;
                    }
                }
                if all_consumed {
                    self.current_this = old_this;
                    self.current_method = old_method;
                }
                // Re-suspend: pertahankan konteks — ContinueAstBlock berikutnya
                // sudah menyimpan this/method baru di titik suspend-nya.
            }
            // WAV-13: commit tertunda dari write signal ber-annotasi SDF delay
            // (dijadwalkan `write_lvalue` saat delay_rise/delay_fall > 0).
            // Commit memakai helper yang sama dengan jalur write langsung
            // (multi-driver resolution + record_signal_change) sehingga proses
            // sensitive ikut terpicu pada waktu yang benar (t+delay).
            EventKind::SdfDelayedWrite { sig_id, value } => {
                if sig_id < self.design.top.signals.len() {
                    self.commit_delayed_signal_write(sig_id, value)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn process_pending_waits(&mut self, deltas: &[SignalId]) -> Result<bool, SimError> {
        let mut matched = false;
        let mut remaining = Vec::new();
        let waits = std::mem::take(&mut self.pending_waits);
        for (deps, stmts) in waits {
            if deltas.iter().any(|d| deps.contains(d)) {
                matched = true;
                self.evaluate_block_with_delay(&stmts)?;
            } else {
                remaining.push((deps, stmts));
            }
        }
        let newly_pushed = std::mem::take(&mut self.pending_waits);
        remaining.extend(newly_pushed);
        self.pending_waits = remaining;
        Ok(matched)
    }

    /// Nilai sinyal di AWAL delta berjalan — baseline "sebelum perubahan".
    /// `signal_snapshot` di-refresh tiap delta pass (Sched-04), jadi saat
    /// `process_pending_events` dipanggil nilainya = state sebelum write pada
    /// delta ini. Dipakai untuk deteksi level & edge blocking event control.
    fn snapshot_value(&self, id: SignalId) -> LogicVec {
        self.signal_snapshot
            .as_ref()
            .and_then(|s| s.get(id).cloned())
            .unwrap_or_else(|| LogicVec::new(1))
    }

    /// Resume blocking event control `@(sig)` saat signal berubah.
    /// - Level (edge None): nilai BERUBAH pada delta ini membangunkan.
    /// - Edge: hanya edge yang sesuai (via snapshot awal-delta) yang membangunkan.
    ///
    /// Baseline = nilai awal delta (`signal_snapshot`), BUKAN nilai saat arm:
    /// - `@(posedge clk)` yang di-arm saat clk sudah 1 tetap menangkap posedge
    ///   berikutnya (0→1) — nilai saat arm (1) tidak membatalkan deteksi edge.
    /// - `@(ev)` yang di-arm setelah `-> ev` pada delta yang sama tetap fire
    ///   karena snapshot awal-delta (x) != nilai saat ini (1).
    pub(crate) fn process_pending_events(&mut self, deltas: &[SignalId]) -> Result<bool, SimError> {
        let mut matched = false;
        let mut remaining = Vec::new();
        let pending = std::mem::take(&mut self.pending_events);
        for pe in pending {
            let _ = &deltas; // LANG-51: edge dideteksi via snapshot-vs-current,
                             // bukan deltas gate (perubahan Active-region pada
                             // delta sama tetap terlihat di delta berikut).
            let fire = pe.sigs.iter().any(|(sid, edge)| {
                match edge {
                    None => {
                        // Level: fire hanya jika nilai BERUBAH dalam delta ini
                        // (snapshot awal != sekarang). Cegah re-fire untuk write
                        // nilai sama; tetap memenuhi `@(ev)` pada delta yang sama.
                        if !deltas.contains(sid) {
                            return false;
                        }
                        self.snapshot_value(*sid) != *self.state.read_signal(*sid)
                    }
                    Some(ClockEdge::PosEdge(id)) => {
                        // LANG-51: edge dilihat dari baseline snapshot (awal
                        // delta) vs nilai kini — TIDAK di-gate deltas, karena
                        // perubahan yang di-commit region Active (delta lebih
                        // awal) tetap sah memicu edge di pass Reactive.
                        let new = self.state.read_signal(*id);
                        self.snapshot_value(*id).to_bool() != Some(true)
                            && new.to_bool() == Some(true)
                    }
                    Some(ClockEdge::NegEdge(id)) => {
                        let new = self.state.read_signal(*id);
                        self.snapshot_value(*id).to_bool() != Some(false)
                            && new.to_bool() == Some(false)
                    }
                    // F27: clock/edge hierarkis (`@(posedge b.clk)` via port
                    // interface) — resolve Symbol path via hier_signal_map.
                    Some(ClockEdge::PosEdgeHier(s)) => {
                        let id = match self.design.hier_signal_map.get(s) {
                            Some(&sid) => sid,
                            None => return false,
                        };
                        let new = self.state.read_signal(id);
                        self.snapshot_value(id).to_bool() != Some(true)
                            && new.to_bool() == Some(true)
                    }
                    Some(ClockEdge::NegEdgeHier(s)) => {
                        let id = match self.design.hier_signal_map.get(s) {
                            Some(&sid) => sid,
                            None => return false,
                        };
                        let new = self.state.read_signal(id);
                        self.snapshot_value(id).to_bool() != Some(false)
                            && new.to_bool() == Some(false)
                    }
                }
            });
            if fire {
                // LANG-27: guard `iff (cond)` — lanjutkan continuation hanya
                // bila kondisi guard benar saat event terpenuhi. Jika kondisi
                // salah, tunggu event berikutnya (entry tetap hidup).
                let guard_ok = match &pe.iff {
                    Some(cond) => match self.evaluate_expr(cond) {
                        Ok(v) => v.to_bool().unwrap_or(false),
                        Err(_) => false,
                    },
                    None => true,
                };
                if guard_ok {
                    matched = true;
                    // Branch fork: decrement ForkGroup bila continuation BENAR
                    // selesai (bukan suspend lagi). Tanpa ini join menggantung
                    // (probe v03: @(ev) di branch fork).
                    let fid = pe.fork_id;
                    // LANG-30/F47: branch fork yang sudah di-terminate
                    // (`disable fork` / `disable <label>`) — JANGAN eksekusi
                    // continuation saat event-nya fire; cukup decrement agar
                    // group bisa finish. (Jalur ContinueBlock sudah menjaga ini,
                    // tapi resume `@(edge)` lewat sini — tanpa guard, branch
                    // yang ter-disable tetap menulis setelah disable.)
                    if let Some(f) = fid {
                        if f < self.fork_groups.len() && self.fork_groups[f].disabled {
                            self.fork_decrement(f)?;
                            continue;
                        }
                    }
                    let all_consumed =
                        self.evaluate_block_with_delay_fork(&pe.continuation, fid)?;
                    if let Some(f) = fid {
                        if all_consumed {
                            self.fork_decrement(f)?;
                        }
                    }
                    continue;
                }
            }
            remaining.push(pe);
        }
        // Resume tadi bisa mendaftarkan pending event baru (mis. `forever @(sig)`
        // yang re-suspend). Jangan timpa — gabungkan agar event baru tetap hidup
        // dan membangunkan pada perubahan sinyal berikutnya.
        let newly_pushed = std::mem::take(&mut self.pending_events);
        remaining.extend(newly_pushed);
        self.pending_events = remaining;
        Ok(matched)
    }

    /// Resume blocking event control `@(sig)` di jalur AST (task/method UVM),
    /// dengan restore konteks method (this/locals/method).
    pub(crate) fn process_pending_ast_events(
        &mut self,
        deltas: &[SignalId],
    ) -> Result<bool, SimError> {
        let mut matched = false;
        let mut remaining = Vec::new();
        let pending = std::mem::take(&mut self.pending_ast_events);
        for pe in pending {
            let fire = pe.sigs.iter().any(|(sid, edge)| {
                match edge {
                    None => {
                        if !deltas.contains(sid) {
                            return false;
                        }
                        self.snapshot_value(*sid) != *self.state.read_signal(*sid)
                    }
                    Some(ClockEdge::PosEdge(id)) => {
                        if !deltas.contains(id) {
                            return false;
                        }
                        let new = self.state.read_signal(*id);
                        self.snapshot_value(*id).to_bool() != Some(true)
                            && new.to_bool() == Some(true)
                    }
                    Some(ClockEdge::NegEdge(id)) => {
                        if !deltas.contains(id) {
                            return false;
                        }
                        let new = self.state.read_signal(*id);
                        self.snapshot_value(*id).to_bool() != Some(false)
                            && new.to_bool() == Some(false)
                    }
                    // F27: clock/edge hierarkis (`@(posedge b.clk)` via port
                    // interface) — resolve Symbol path via hier_signal_map.
                    Some(ClockEdge::PosEdgeHier(s)) => {
                        let id = match self.design.hier_signal_map.get(s) {
                            Some(&sid) => sid,
                            None => return false,
                        };
                        if !deltas.contains(&id) {
                            return false;
                        }
                        let new = self.state.read_signal(id);
                        self.snapshot_value(id).to_bool() != Some(true)
                            && new.to_bool() == Some(true)
                    }
                    Some(ClockEdge::NegEdgeHier(s)) => {
                        let id = match self.design.hier_signal_map.get(s) {
                            Some(&sid) => sid,
                            None => return false,
                        };
                        if !deltas.contains(&id) {
                            return false;
                        }
                        let new = self.state.read_signal(id);
                        self.snapshot_value(id).to_bool() != Some(false)
                            && new.to_bool() == Some(false)
                    }
                }
            });
            if !fire {
                remaining.push(pe);
                continue;
            }
            // LANG-27: guard `iff (cond)` di jalur AST — lanjutkan hanya bila
            // kondisi benar. Jika salah, tunggu event berikutnya.
            let guard_ok = match &pe.iff {
                Some(cond) => match self.evaluate_ast_expr(cond) {
                    Ok(v) => v.to_bool().unwrap_or(false),
                    Err(_) => false,
                },
                None => true,
            };
            if !guard_ok {
                remaining.push(pe);
                continue;
            }
            // Restore konteks method task sebelum resume continuation.
            let old_this = self.current_this;
            let old_method = self.current_method;
            let _old_locals = std::mem::replace(&mut self.method_locals, pe.locals.clone());
            self.current_this = pe.this;
            self.current_method = pe.method;
            matched = true;
            let completed = self.evaluate_ast_block_with_delay_fork(&pe.continuation, None)?;
            // F35 review: return di continuation (illegal SV) menandai
            // ast_return_pending — clear di sini agar tidak bocor.
            self.ast_return_pending = false;
            if completed {
                // Task selesai — truncate frame locals task; kembalikan konteks.
                let keep = pe.base_len.saturating_sub(1).min(self.method_locals.len());
                self.method_locals.truncate(keep);
                self.current_this = old_this;
                self.current_method = old_method;
            } else {
                // Task masih re-suspend — pertahankan locals task (old_locals dibuang).
            }
        }
        // Gabungkan pending AST event baru yang didaftarkan saat resume
        // (mis. `forever @(sig)` yang re-suspend), jangan timpa.
        let newly_pushed = std::mem::take(&mut self.pending_ast_events);
        remaining.extend(newly_pushed);
        self.pending_ast_events = remaining;
        Ok(matched)
    }

    /// Resume `wait (cond)` AST (task/method UVM) saat kondisinya menjadi
    /// benar. Dipanggil tiap delta yang punya perubahan sinyal; cond
    /// dievaluasi ulang penuh dengan konteks method tersimpan.
    pub(crate) fn process_pending_ast_waits(
        &mut self,
        deltas: &[SignalId],
    ) -> Result<bool, SimError> {
        if deltas.is_empty() || self.pending_ast_waits.is_empty() {
            return Ok(false);
        }
        let mut matched = false;
        let mut remaining = Vec::new();
        let waits = std::mem::take(&mut self.pending_ast_waits);
        for pw in waits {
            let saved_this = self.current_this;
            let saved_method = self.current_method;
            let saved_locals = std::mem::replace(&mut self.method_locals, pw.locals.clone());
            self.current_this = pw.this;
            self.current_method = pw.method;
            let ok = match self.evaluate_ast_expr(&pw.cond) {
                Ok(v) => v.to_bool().unwrap_or(false),
                Err(_) => false,
            };
            if !ok {
                // Belum benar — restore konteks & tetap menunggu.
                self.method_locals = saved_locals;
                self.current_this = saved_this;
                self.current_method = saved_method;
                remaining.push(pw);
                continue;
            }
            matched = true;
            let completed = self.evaluate_ast_block_with_delay_fork(&pw.continuation, None)?;
            // F35: `return` di continuation (illegal SV) menandai
            // ast_return_pending — clear di sini agar tidak bocor.
            self.ast_return_pending = false;
            if completed {
                let keep = pw.base_len.saturating_sub(1).min(self.method_locals.len());
                self.method_locals.truncate(keep);
                self.current_this = saved_this;
                self.current_method = saved_method;
            }
            // Bila belum completed: task re-suspend — pertahankan locals task.
        }
        let newly_pushed = std::mem::take(&mut self.pending_ast_waits);
        remaining.extend(newly_pushed);
        self.pending_ast_waits = remaining;
        Ok(matched)
    }

    pub(crate) fn process_pending_wait_orders(
        &mut self,
        deltas: &[SignalId],
    ) -> Result<bool, SimError> {
        let mut any_done = false;
        let mut remaining = Vec::new();
        let orders = std::mem::take(&mut self.pending_wait_orders);
        'order: for mut order in orders {
            let mut changed_in_order = Vec::new();
            for d in deltas {
                if let Some(pos) = order.events.iter().position(|e| e == d) {
                    changed_in_order.push(pos);
                }
            }
            changed_in_order.sort();
            for &pos in &changed_in_order {
                if pos == order.expected_idx {
                    order.expected_idx += 1;
                    if order.expected_idx == order.events.len() {
                        if !order.continuation.is_empty() {
                            self.evaluate_block_with_delay(&order.continuation)?;
                        }
                        any_done = true;
                        continue 'order;
                    }
                } else if pos > order.expected_idx {
                    if !order.failure_stmts.is_empty() {
                        self.evaluate_stmt_block(&order.failure_stmts)?;
                    }
                    any_done = true;
                    continue 'order;
                }
            }
            remaining.push(order);
        }
        for item in remaining {
            self.pending_wait_orders.push(item);
        }
        Ok(any_done)
    }

    /// Terapkan write hasil evaluasi paralel ke state. Sama seperti jalur serial
    /// (`write_lvalue`, lvalue.rs:234): net Wire/Inout multi-driver di-RESOLVE
    /// terhadap nilai saat ini (resolve_bit), bukan last-write-wins — tanpa ini
    /// hasil paralel (dipakai saat ≥`min_processes_parallel` proses comb) bisa
    /// beda dari serial (ditemukan mivon-fuzz: EMI dead-code menambah 1 proses →
    /// melewati ambang → multi-driver wire berubah nilai → mismatch EMI palsu).
    /// PINCH-SEMANTICS (fuzz dag bug_0cb12a3dc00b / 63d353b8c78f): write
    /// PARTIAL (`mid > 0` atau patch lebih sempit dari sinyal) = patch
    /// slice `[start..start+len)` — di-apply sebagai overwrite slice
    /// (mirror jalur serial RangeSelect, lvalue.rs:278), BUKAN full-width
    /// resolve: dulu full-vec tail snapshot ikut di-resolve union dgn
    /// X-initial → X menghancurkan bit non-target (dag Q=x vs default z).
    /// Full-write (start==0, len==width): resolusi multi-driver tetap.
    pub(crate) fn apply_parallel_writes(&mut self, writes: &[ParallelWrite]) {
        for (sig_id, start, patch) in writes {
            let info = self.design.top.signals.get(*sig_id);
            let target_width = info.map(|i| i.width).unwrap_or(0);
            let is_full = *start == 0 && patch.width >= target_width;
            if !is_full {
                // Patch slice — mirror serial RangeSelect: RMW lokal tanpa
                // resolution multi-driver (hanya bit ter-patch yang berubah).
                if *start < target_width {
                    let mut cur = self.state.read_signal(*sig_id).clone();
                    let len = patch.width.min(cur.width.saturating_sub(*start));
                    for i in 0..len {
                        cur.bits[start + i] =
                            patch.bits.get(i).copied().unwrap_or(mivon_ir::LogicVal::X);
                    }
                    self.state.write_signal(*sig_id, cur);
                }
                continue;
            }
            if let Some(info) = info.filter(|i| i.multi_driver) {
                if matches!(
                    info.kind,
                    SignalKind::Wire | SignalKind::Inout | SignalKind::Output
                ) {
                    let current = self.state.read_signal(*sig_id).clone();
                    let resolved =
                        crate::simulator::util::resolve_net_values(info.net_type, &current, patch);
                    self.state.write_signal(*sig_id, resolved);
                    continue;
                }
            }
            self.state.write_signal(*sig_id, patch.clone());
        }
    }

    /// Pisahkan process comb jadi kelompok independen (tanpa conflict antar
    /// anggota). Greedy: ambil sebanyak mungkin process yang saling bebas per
    /// kelompok. Kelompok dieval SEQUENTIAL (apply antar kelompok), di dalam
    /// kelompok PARALEL. Menghilangkan ketergantungan implisit urutan thread
    /// saat process ber-WAW/RAW dalam delta yang sama.
    pub(crate) fn layered_groups(
        comb_indices: &[usize],
        access: &[crate::scheduler::sim_dag::SignalAccess],
    ) -> Vec<Vec<usize>> {
        // Debug multi-writer: dua process nulis signal sama → WAW.
        if std::env::var("MIVON_DBG_MW").is_ok() {
            for i in 0..access.len() {
                for j in (i + 1)..access.len() {
                    let w = access[i]
                        .writes
                        .iter()
                        .copied()
                        .collect::<std::collections::HashSet<_>>();
                    let ow = access[j]
                        .writes
                        .iter()
                        .copied()
                        .collect::<std::collections::HashSet<_>>();
                    let inter: Vec<usize> = w.intersection(&ow).copied().collect();
                    if !inter.is_empty() && comb_indices.contains(&i) && comb_indices.contains(&j) {
                        eprintln!("[DBG-MW] proc{i} & proc{j} both write sig {:?}", inter);
                    }
                }
            }
        }
        let conflict = |a: &crate::scheduler::sim_dag::SignalAccess,
                        b: &crate::scheduler::sim_dag::SignalAccess| {
            a.writes.iter().any(|sig| b.reads.contains(sig))
                || b.writes.iter().any(|sig| a.reads.contains(sig))
                || a.writes.iter().any(|sig| b.writes.contains(sig))
        };

        // BTreeSet (bukan HashSet): urutan iterasi HARUS deterministik — urutan
        // group menentukan urutan apply write, dan resolusi net multi-driver
        // akumulatif terhadap nilai saat ini. HashSet ber-seed acak per-instance
        // membuat io_deq_valid flip X/1 antar run (fuzz bug 7313335569a9).
        let mut remaining: std::collections::BTreeSet<usize> =
            comb_indices.iter().copied().collect();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        while !remaining.is_empty() {
            let mut group: Vec<usize> = Vec::new();
            let mut taken = std::collections::HashSet::new();
            // Ambil semua process yang tidak konflik dengan yang sudah di-grup.
            loop {
                let mut added = false;
                for &pid in remaining.clone().iter() {
                    if taken.contains(&pid) {
                        continue;
                    }
                    let pa = access.get(pid);
                    let pa = match pa {
                        Some(a) => a,
                        None => continue,
                    };
                    let free = group
                        .iter()
                        .all(|&other| !conflict(pa, access.get(other).unwrap_or(pa)));
                    if free {
                        group.push(pid);
                        taken.insert(pid);
                        added = true;
                    }
                }
                if !added {
                    break;
                }
            }
            for pid in &group {
                remaining.remove(pid);
            }
            if !group.is_empty() {
                groups.push(group);
            }
        }
        groups
    }

    pub(crate) fn trigger_sensitive_processes(
        &mut self,
        changed: &[(usize, LogicVec, LogicVec)],
        _t: usize,
    ) -> Result<(), SimError> {
        // PERF: Phase 1 — collect triggered pids via immutable scan (NO clone of all 176K processes).
        // Previous code cloned self.design.top.processes (ALL processes) on every delta cycle.
        // For OpenTitan (176K processes × thousands of delta cycles), this caused billions of
        // allocations and was the primary cause of time step stuck at #0.
        let mut comb_indices: Vec<usize> = Vec::new();
        let mut combreactive_indices: Vec<usize> = Vec::new();
        // Sequential: collect (pid, clock, reset, iff) for later evaluation
        let mut seq_candidates: Vec<(
            usize,
            ClockEdge,
            Option<mivon_ir::ResetInfo>,
            Option<IrExpr>,
            bool, // clock_trigger
        )> = Vec::new();

        for (pid, process) in self.design.top.processes.iter().enumerate() {
            match process {
                Process::Combinational { sensitivity, .. } => {
                    // Skip if this process is fused into a clock domain
                    if self.use_cycle_fusion
                        && self
                            .clock_analysis
                            .as_ref()
                            .map(|a| a.fused_processes.contains(&pid))
                            .unwrap_or(false)
                    {
                        continue;
                    }
                    let should_trigger =
                        sensitivity.is_empty() || sensitivity_triggered(sensitivity, changed);
                    if should_trigger {
                        comb_indices.push(pid);
                    }
                }
                Process::CombReactive { sensitivity, .. } => {
                    let should_trigger =
                        sensitivity.is_empty() || sensitivity_triggered(sensitivity, changed);
                    if should_trigger {
                        combreactive_indices.push(pid);
                    }
                }
                Process::Sequential {
                    clock, reset, iff, ..
                } => {
                    let clock_trigger = match clock {
                        ClockEdge::PosEdge(_) | ClockEdge::PosEdgeHier(_) => {
                            let sid = self.clock_edge_signal(clock).unwrap_or(usize::MAX);
                            changed.iter().any(|(id, old, new)| {
                                *id == sid
                                    && old.to_bool() != Some(true)
                                    && new.to_bool() == Some(true)
                            })
                        }
                        ClockEdge::NegEdge(_) | ClockEdge::NegEdgeHier(_) => {
                            let sid = self.clock_edge_signal(clock).unwrap_or(usize::MAX);
                            changed.iter().any(|(id, old, new)| {
                                *id == sid
                                    && old.to_bool() != Some(false)
                                    && new.to_bool() == Some(false)
                            })
                        }
                    };
                    let reset_trigger = reset
                        .as_ref()
                        .filter(|r| r.r#async)
                        .map(|r| {
                            let sid = r.signal;
                            let active_high = r.polarity;
                            changed.iter().any(|(id, old, new)| {
                                *id == sid
                                    && if active_high {
                                        old.to_bool() != Some(true) && new.to_bool() == Some(true)
                                    } else {
                                        old.to_bool() != Some(false) && new.to_bool() == Some(false)
                                    }
                            })
                        })
                        .unwrap_or(false);
                    if clock_trigger || reset_trigger {
                        seq_candidates.push((
                            pid,
                            clock.clone(),
                            reset.clone(),
                            iff.clone(),
                            clock_trigger,
                        ));
                    }
                }
                _ => {}
            }
        }
        // ── End Phase 1: immutable borrow released ──

        // If enough processes to parallelize and config allows it, use parallel eval
        crate::dbg_sim!(
            2,
            "t={} delta={} trigger_sensitive: {} comb process(es), {} changed",
            self.current_time,
            self.current_delta,
            comb_indices.len(),
            changed.len()
        );
        if comb_indices.len() >= self.parallel_config.min_processes_parallel
            && self.parallel_config.parallel_processes
            && !self.has_wide_signals()
        {
            use rayon::prelude::*;
            let signal_count = self.state.signals.len();

            // ── SIM-28: snapshot SPARSE ──
            // Bangun cache akses per-process sekali (lazy). Bila ada process
            // dengan akses tak-resolve (HierRef dll) → mode FULL (snapshot
            // seluruh sinyal). Bila semua ter-resolve → mode SPARSE: base
            // hanya berisi UNION sinyal yang diakses process terpicu —
            // per-process setup O(0) (overlay kosong), bukan clone O(S).
            if !self.comb_access_ready {
                self.comb_access = self
                    .design
                    .top
                    .processes
                    .iter()
                    .map(crate::scheduler::sim_dag::analyze_process_access)
                    .collect();
                self.comb_access_ready = true;
            }
            let needs_full = comb_indices.iter().any(|&pid| {
                self.comb_access
                    .get(pid)
                    .map(|a| a.has_unresolved)
                    .unwrap_or(true)
            });

            // ── LAYERING: pisah process comb yang conflict (RAW/WAR/WAW)
            //    jadi kelompok independen, eval+apply tiap kelompok suksesif,
            //    refresh snapshot antar kelompok. Tanpa ini semua process
            //    sekaligus eval dari snapshot tunggal → sesuai urutan thread
            //    (mis. `lsu_pipe_vld` = `0xx` default vs `xx0` DAG — fuzzer
            //    differential OpenC910 ct_had_dbg_info).
            let groups: Vec<Vec<usize>> = Self::layered_groups(&comb_indices, &self.comb_access);
            let dbg_t = self.current_time;
            let dbg_delta = self.current_delta;
            for group in &groups {
                let group: Vec<usize> = group.clone();
                if group.is_empty() {
                    continue;
                }
                // Snapshot per-kelompok dari state TERKINI (pending applied
                // setelah kelompok sebelumnya).
                let snapshot: Vec<Arc<LogicVec>> = (0..signal_count)
                    .map(|i| {
                        // Baca PENDING (next) bila changed — konsisten dgn
                        // evaluate_eval_processes_parallel. Sinyal besar yang
                        // belum ditulis di-snapshot LAZY (tanpa materialisasi).
                        let v = self.state.snapshot_signal(i);
                        Arc::new(v)
                    })
                    .collect();

                let eval_group = if needs_full {
                    let identity: Vec<Option<usize>> = (0..signal_count).map(Some).collect();
                    let results: Vec<Result<Vec<parallel::ParallelWrite>, SimError>> = group
                        .par_iter()
                        .map(|&pid| {
                            let use_packed = self.use_packed_eval;
                            if let Process::Combinational { body, .. } =
                                &self.design.top.processes[pid]
                            {
                                crate::dbg_sim!(
                                    3,
                                    "t={} delta={} par-eval pid={}",
                                    dbg_t,
                                    dbg_delta,
                                    pid
                                );
                                let mut overlay = std::collections::HashMap::new();
                                let mut view =
                                    parallel::SignalView::new(&snapshot, &identity, &mut overlay);
                                let mut writes = Vec::new();
                                match parallel::with_packed_eval(use_packed, || {
                                    parallel::evaluate_stmt_block_parallel(
                                        body,
                                        &mut view,
                                        &mut writes,
                                        &self.design.top.signals,
                                    )
                                }) {
                                    Ok(()) => Ok(writes),
                                    Err(e) => Err(SimError::with_diag(
                                        DiagCode::InternalError,
                                        format!("parallel eval error: {}", e),
                                    )),
                                }
                            } else {
                                Ok(Vec::new())
                            }
                        })
                        .collect();
                    results
                } else {
                    // ── Mode SPARSE: base = union sinyal yang diakses ──
                    let mut needed = vec![false; signal_count];
                    for &pid in &group {
                        if let Some(a) = self.comb_access.get(pid) {
                            for &r in &a.reads {
                                if r < signal_count {
                                    needed[r] = true;
                                }
                            }
                            for &w in &a.writes {
                                if w < signal_count {
                                    needed[w] = true;
                                }
                            }
                        }
                    }
                    let mut id_map: Vec<Option<usize>> = vec![None; signal_count];
                    let mut base: Vec<Arc<LogicVec>> = Vec::new();
                    for i in 0..signal_count {
                        if needed[i] {
                            id_map[i] = Some(base.len());
                            base.push(snapshot[i].clone());
                        }
                    }
                    crate::dbg_sim!(
                        2,
                        "  sparse: base {} sig dari {} (union akses), {} process",
                        base.len(),
                        signal_count,
                        group.len()
                    );
                    let results: Vec<Result<Vec<parallel::ParallelWrite>, SimError>> = group
                        .par_iter()
                        .map(|&pid| {
                            let use_packed = self.use_packed_eval;
                            if let Process::Combinational { body, .. } =
                                &self.design.top.processes[pid]
                            {
                                crate::dbg_sim!(
                                    3,
                                    "t={} delta={} par-eval pid={}",
                                    dbg_t,
                                    dbg_delta,
                                    pid
                                );
                                let mut overlay = std::collections::HashMap::new();
                                let mut view =
                                    parallel::SignalView::new(&base, &id_map, &mut overlay);
                                let mut writes = Vec::new();
                                match parallel::with_packed_eval(use_packed, || {
                                    parallel::evaluate_stmt_block_parallel(
                                        body,
                                        &mut view,
                                        &mut writes,
                                        &self.design.top.signals,
                                    )
                                }) {
                                    Ok(()) => Ok(writes),
                                    Err(e) => Err(SimError::with_diag(
                                        DiagCode::InternalError,
                                        format!("parallel eval error: {}", e),
                                    )),
                                }
                            } else {
                                Ok(Vec::new())
                            }
                        })
                        .collect();
                    results
                };
                for result in eval_group {
                    let writes = result?;
                    self.apply_parallel_writes(&writes);
                }
            }
        } else {
            // Sequential path: evaluate triggered comb processes inline
            // PERF: clone only triggered bodies, not all 176K processes
            for &pid in &comb_indices {
                let body = match &self.design.top.processes[pid] {
                    Process::Combinational { body, .. } => body.clone(),
                    _ => continue,
                };
                crate::dbg_sim!(
                    3,
                    "t={} delta={} seq-eval pid={}",
                    self.current_time,
                    self.current_delta,
                    pid
                );
                self.evaluate_stmt_block(&body)?;
            }
        }

        // ── Phase 2b: CombReactive — push to reactive_events (just pids, no clone) ──
        for pid in combreactive_indices {
            self.reactive_events.push(EventKind::EvalProcess(pid));
        }

        // ── Phase 2c: Sequential — evaluate clock/reset/iff per candidate ──
        // Par-02 tahap 3: klasifikasi body per kandidat (NbaOnly /
        // BlockingOnly / Serial) via body_write_kinds; dua jalur apply:
        // Blocking → apply_parallel_writes immediate, Nba → push_nba_pending
        // (commit identik serial). Guards:
        //   (A') tolak kandidat yang membaca sinyal NBA-writen dirinya
        //        (overlay terlihat; serial defer); blocking RAW intra-body
        //        AMAN (overlay = blocking serial);
        //   (B)  tolak kandidat yang membaca tulisan kandidat serial inline;
        //   (B2) demosi berbasis pembaca-lain: blocking write yang dibaca
        //        kandidat lain (body ATAU iff — re-review: iff membaca
        //        blocking write kandidat lain = BUG-1 R186) → demosi penulis;
        //        kandidat Nba yang membaca blocking write lain (body/iff)
        //        → demosi (stale); WAW blocking vs serial/blocking → demosi;
        //   (call/unresolved) FuncCall/MethodCall/HierRef → serial;
        //   (fused) ada kandidat cycle-fusion → fallback loop serial lama.
        let mut cands: Vec<SeqCand> = Vec::new();
        let mut any_fused = false;
        for (pid, _clock, reset, iff, clock_trigger) in seq_candidates {
            // F40 fix: async reset edge (immutable — aman di fase 1)
            let async_trig = reset
                .as_ref()
                .filter(|r| r.r#async)
                .map(|r| {
                    let sid = r.signal;
                    let active_high = r.polarity;
                    changed.iter().any(|(id, old, new)| {
                        *id == sid
                            && if active_high {
                                old.to_bool() != Some(true) && new.to_bool() == Some(true)
                            } else {
                                old.to_bool() != Some(false) && new.to_bool() == Some(false)
                            }
                    })
                })
                .unwrap_or(false);
            if !clock_trigger && !async_trig {
                continue;
            }
            let fused_domain = if self.use_cycle_fusion {
                self.clock_analysis.as_ref().and_then(|a| {
                    if a.fused_processes.contains(&pid) {
                        a.domains
                            .iter()
                            .find(|d| d.sequential_processes.contains(&pid))
                            .cloned()
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            if fused_domain.is_some() {
                any_fused = true;
            }
            let mut cand = SeqCand::classify(self, pid);
            cand.clock_trigger = clock_trigger;
            cand.iff = iff;
            cand.async_trig = async_trig;
            cand.fused = fused_domain;
            cand.iff_reads = match &cand.iff {
                Some(cond) => crate::scheduler::sim_dag::expr_signal_reads_of(cond),
                None => Default::default(),
            };
            cands.push(cand);
        }
        if any_fused {
            // Re-review BUG-2 (R184): satu loop interleaved — iff dan fused
            // domain dieval pada posisi kandidat, persis pra-Par-02.
            for c in &cands {
                let mut trigger = c.clock_trigger
                    && match &c.iff {
                        Some(cond) => match self.evaluate_expr(cond) {
                            Ok(v) => v.to_bool().unwrap_or(false),
                            Err(_) => false,
                        },
                        None => true,
                    };
                trigger = trigger || c.async_trig;
                if !trigger {
                    continue;
                }
                if let Some(domain) = &c.fused {
                    self.evaluate_clock_domain(domain)?;
                    continue;
                }
                let body = match &self.design.top.processes[c.pid] {
                    Process::Sequential { body, .. } => body.clone(),
                    _ => continue,
                };
                self.evaluate_stmt_block(&body)?;
            }
            return Ok(());
        }
        self.run_seq_batch(&cands)
    }

    /// Agregat fase 1 + fase 2 (demosi) + dispatch batch Par-02 tahap
    /// 3/4 — dipakai event.rs Phase 2c (trigger/iff/fused ditangani
    /// pemanggil) dan core.rs evaluate_clock_domain Step 1 (semua
    /// kandidat domain triggered, iff kosong).
    pub(crate) fn run_seq_batch(&mut self, cands: &[SeqCand]) -> Result<(), SimError> {
        // Agregat fase 1: serial_writes (non-batchable) dipecah blocking
        // vs NBA — hanya blocking write serial yang berbahaya dibaca batch
        // (diterapkan inline segera; NBA write serial commit di NBA region
        // sehingga aman dibaca stale — review R186 BUG-1). Kind None →
        // semua tulisannya dianggap blocking (konservatif). blocking_writes
        // + counts (blocking batchable), reads_count (efektif = body ∪ iff,
        // per kandidat sekali per sinyal).
        let mut serial_writes: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut serial_blocking_writes: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let mut blocking_writes: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let mut blocking_write_counts: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        let mut nba_write_counts: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        let mut reads_count: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for c in cands {
            if !c.batchable {
                serial_writes.extend(c.access.writes.iter().copied());
                let kinds = self
                    .process_body_arcs
                    .get(&c.pid)
                    .and_then(|b| crate::scheduler::sim_dag::body_write_kinds(b.as_slice()));
                match kinds {
                    Some((bw, nw)) => {
                        for s in c.access.writes.iter() {
                            if bw.contains(s) || !nw.contains(s) {
                                serial_blocking_writes.insert(*s);
                            }
                        }
                    }
                    None => {
                        serial_blocking_writes.extend(c.access.writes.iter().copied());
                    }
                }
            }
            if c.batchable && c.blocking_only {
                for s in c.own_blocking.iter() {
                    *blocking_write_counts.entry(*s).or_insert(0) += 1;
                }
                blocking_writes.extend(c.own_blocking.iter().copied());
            }
            // Review R186 BUG-2: WAW antar kandidat NbaOnly batch — bila
            // salah satu ter-demosi-inline (mis. baca blocking write), push
            // NBA-nya terjadi saat fase 2 sementara kandidat lain di-defer
            // (push setelahnya) → pemenang WAW terbalik. Tulisannya dihitung
            // di sini; overlap → demosi keduanya (konservatif).
            if c.batchable && !c.blocking_only {
                for s in c.access.writes.iter() {
                    *nba_write_counts.entry(*s).or_insert(0) += 1;
                }
            }
            let mut effective = c.access.reads.clone();
            effective.extend(c.iff_reads.iter().copied());
            for s in effective {
                *reads_count.entry(s).or_insert(0) += 1;
            }
        }
        // Fase 2 — urutan pid: iff dieval pada posisi kandidat; demosi;
        // defer hanya kandidat yang lolos semua guard.
        let mut seq_batch: Vec<&SeqCand> = Vec::new();
        for c in cands {
            // LANG-27: guard `iff (cond)` — pada POSISI kandidat.
            let mut trigger = c.clock_trigger
                && match &c.iff {
                    Some(cond) => match self.evaluate_expr(cond) {
                        Ok(v) => v.to_bool().unwrap_or(false),
                        Err(_) => false,
                    },
                    None => true,
                };
            trigger = trigger || c.async_trig;
            if !trigger {
                continue;
            }
            if !c.batchable {
                let body = match &self.design.top.processes[c.pid] {
                    Process::Sequential { body, .. } => body.clone(),
                    _ => continue,
                };
                self.evaluate_stmt_block(&body)?;
                continue;
            }
            let own_effective = |s: &usize| c.access.reads.contains(s) || c.iff_reads.contains(s);
            let others_read = |s: &usize| {
                reads_count.get(s).copied().unwrap_or(0) > if own_effective(s) { 1 } else { 0 }
            };
            let demote = if c.blocking_only {
                // Blocking: (i) membaca blocking write serial inline;
                // (ii) membaca blocking write kandidat batch lain (urutan
                // grup ≠ pid — review R186 BUG-2; RAW intra-body dikecualikan
                // via own_blocking, aman via overlay); (iii) WAW dgn
                // serial/blocking lain; (iv) menulis sinyal yang dibaca
                // kandidat lain (defer → basi).
                !c.access.reads.is_disjoint(&serial_blocking_writes)
                    || !c.iff_reads.is_disjoint(&serial_blocking_writes)
                    || !c.access.writes.is_disjoint(&serial_writes)
                    || c.access
                        .reads
                        .iter()
                        .chain(c.iff_reads.iter())
                        .any(|s| blocking_writes.contains(s) && !c.own_blocking.contains(s))
                    || c.own_blocking.iter().any(|s| {
                        blocking_writes.contains(s)
                            && blocking_write_counts.get(s).copied().unwrap_or(0) > 1
                    })
                    || c.own_blocking.iter().any(&others_read)
            } else {
                // Nba: membaca blocking write (serial inline ATAU batch
                // lain, body/iff — review R186 BUG-1: blocking write serial
                // diterapkan inline segera, snapshot batch melihat nilai
                // baru sementara serial asli membaca nilai lama); WAW dgn
                // serial inline (urutan push NBA berubah bila defer).
                !c.access.reads.is_disjoint(&blocking_writes)
                    || !c.access.reads.is_disjoint(&serial_blocking_writes)
                    || !c.iff_reads.is_disjoint(&blocking_writes)
                    || !c.iff_reads.is_disjoint(&serial_blocking_writes)
                    || !c.access.writes.is_disjoint(&serial_writes)
                    || c.access
                        .writes
                        .iter()
                        .any(|s| nba_write_counts.get(s).copied().unwrap_or(0) > 1)
            };
            if demote {
                let body = match &self.design.top.processes[c.pid] {
                    Process::Sequential { body, .. } => body.clone(),
                    _ => continue,
                };
                self.evaluate_stmt_block(&body)?;
                continue;
            }
            seq_batch.push(c);
        }
        let batch_pids: Vec<usize> = seq_batch.iter().map(|c| c.pid).collect();
        if batch_pids.len() >= 2 {
            self.evaluate_sequential_batch_parallel(&seq_batch)?;
        } else {
            self.evaluate_batch_serially(&batch_pids)?;
        }
        Ok(())
    }

    /// Fallback serial seluruh batch (urutan pid) — dipakai saat batch
    /// terlalu kecil, ada kandidat cycle-fusion, sinyal ultra-lebar (E),
    /// atau evaluator paralel gagal (D).
    fn evaluate_batch_serially(&mut self, batch: &[usize]) -> Result<(), SimError> {
        for &pid in batch {
            let body = match &self.design.top.processes[pid] {
                Process::Sequential { body, .. } => body.clone(),
                _ => continue,
            };
            self.evaluate_stmt_block(&body)?;
        }
        Ok(())
    }

    /// Par-02 tahap 3: batch-parallel eval triggered Sequential processes.
    /// Dua jalur apply per grup: BlockingOnly → apply_parallel_writes
    /// (blocking immediate, snapshot rebuild grup berikutnya melihatnya);
    /// NbaOnly → push_nba_pending terurut pid (commit identik serial).
    fn evaluate_sequential_batch_parallel(&mut self, batch: &[&SeqCand]) -> Result<(), SimError> {
        // (E) sinyal ultra-lebar: snapshot penuh berisiko OOM / lazy-X.
        if self.has_wide_signals() {
            let pids: Vec<usize> = batch.iter().map(|c| c.pid).collect();
            return self.evaluate_batch_serially(&pids);
        }
        let access: Vec<crate::scheduler::sim_dag::SignalAccess> =
            batch.iter().map(|c| c.access.clone()).collect();
        let dense_batch: Vec<usize> = (0..batch.len()).collect();
        let groups = Self::layered_groups(&dense_batch, &access);
        crate::dbg_sim!(
            3,
            "t={} delta={} par-02 seq-batch: {} kandidat, {} grup",
            self.current_time,
            self.current_delta,
            batch.len(),
            groups.len()
        );
        let body_map: std::collections::HashMap<usize, std::sync::Arc<Vec<IrStmt>>> = batch
            .iter()
            .filter_map(|c| {
                self.process_body_arcs
                    .get(&c.pid)
                    .map(|b| (c.pid, std::sync::Arc::clone(b)))
            })
            .collect();
        let blocking_only_by_pid: std::collections::HashMap<usize, bool> =
            batch.iter().map(|c| (c.pid, c.blocking_only)).collect();
        // (C) per grup: blocking writes di-apply SEGERA (snapshot grup
        // berikutnya melihatnya), NBA writes dikumpulkan ter-tag lalu
        // di-push terurut pid setelah semua grup (commit identik serial).
        // Err grup → fallback serial grup itu + grup berikutnya (urutan
        // pid); flush NBA batch tetap jalan di akhir (review R186 BUG-3:
        // return dini membuat push grup sukses hilang senyap). Urutan
        // konsisten dgn jalur normal: inline serial dulu, flush batch
        // terakhir.
        let mut nba_pushes: Vec<(usize, Vec<crate::scheduler::sim_dag::ParallelWrite>)> =
            Vec::new();
        let mut err_rest: Option<Vec<usize>> = None;
        for (gi, group) in groups.iter().enumerate() {
            let pids: Vec<&usize> = group.iter().map(|&i| &batch[i].pid).collect();
            // Snapshot penuh per grup — blocking writes grup sebelumnya
            // wajib terlihat (rantai comb antar grup = urutan serial).
            let snapshot: Vec<std::sync::Arc<LogicVec>> = (0..self.state.signals.len())
                .map(|i| std::sync::Arc::new(self.state.snapshot_signal(i)))
                .collect();
            match crate::scheduler::sim_dag::evaluate_bodies_parallel_tagged(
                &pids,
                &body_map,
                &snapshot,
                &self.design.top.signals,
            ) {
                Ok(part) => {
                    for (pid, ws) in part {
                        if blocking_only_by_pid.get(&pid).copied().unwrap_or(false) {
                            self.apply_parallel_writes(&ws);
                        } else {
                            nba_pushes.push((pid, ws));
                        }
                    }
                }
                Err(_) => {
                    let mut rest: Vec<usize> = Vec::new();
                    for g in &groups[gi..] {
                        rest.extend(g.iter().copied());
                    }
                    rest.sort_unstable();
                    err_rest = Some(rest);
                }
            }
            if err_rest.is_some() {
                break;
            }
        }
        // (D) fallback serial: kandidat grup Err + semua grup berikutnya
        // dieval inline (urutan pid). Flush NBA batch tetap jalan di akhir
        // — push grup sukses TIDAK boleh hilang (review R186 BUG-3).
        if let Some(rest) = err_rest {
            for &i in &rest {
                let pid = batch[i].pid;
                let body = match &self.design.top.processes[pid] {
                    Process::Sequential { body, .. } => body.clone(),
                    _ => continue,
                };
                self.evaluate_stmt_block(&body)?;
            }
        }
        nba_pushes.sort_by_key(|(pid, _)| *pid);
        for (_, writes) in nba_pushes {
            for (sig_id, start, val) in writes {
                debug_assert_eq!(start, 0, "batch whitelist hanya full-signal NBA");
                let width = self
                    .design
                    .top
                    .signals
                    .get(sig_id)
                    .map(|i| i.width)
                    .unwrap_or(val.width);
                self.push_nba_pending(IrLValue::Signal(sig_id, width), val);
            }
        }
        Ok(())
    }

    pub(crate) fn commit_nba(&mut self) {
        let pending = std::mem::take(&mut self.nba_pending);
        self.nba_signal_map.clear();
        for (lvalue, val) in pending {
            if !self.is_forced(&lvalue) {
                let _ = self.write_lvalue(&lvalue, val, false);
            }
        }
    }

    /// SIM-14: Push NBA write ke pending list dengan deteksi write conflict.
    /// PERF-15: O(1) conflict detection via nba_signal_map.
    /// Jika signal yang sama sudah punya NBA pending → warning RT1006.
    pub(crate) fn push_nba_pending(&mut self, lvalue: IrLValue, val: LogicVec) {
        if let Some(new_id) = self.signal_id_from_lvalue(&lvalue) {
            if self.nba_signal_map.contains_key(&new_id) {
                self.emit_warning(
                    mivon_core::diagnostics::DiagCode::NbaWriteConflict,
                    format!(
                        "NBA write conflict on signal id={} at time {}: \
                         multiple non-blocking assignments to the same signal in one delta cycle",
                        new_id, self.state.time
                    ),
                );
            }
            self.nba_signal_map.insert(new_id, self.nba_pending.len());
        }
        self.nba_pending.push((lvalue, val));
    }

    pub(crate) fn signal_id_from_lvalue(&self, lvalue: &IrLValue) -> Option<SignalId> {
        match lvalue {
            IrLValue::Signal(id, _) => Some(*id),
            IrLValue::RangeSelect(id, _, _) => Some(*id),
            IrLValue::BitSelect(id, _) => Some(*id),
            IrLValue::ArrayIndex { sig_id, .. } => Some(*sig_id),
            IrLValue::ArrayRangeSelect { sig_id, .. } => Some(*sig_id),
            IrLValue::ArrayBitSelect { sig_id, .. } => Some(*sig_id),
            IrLValue::ExprPartSelect { sig_id, .. } => Some(*sig_id),
            IrLValue::ObjectField { sig_id, .. } => Some(*sig_id),
            IrLValue::HierRef(name) | IrLValue::HierRefIndex { name, .. } => {
                self.find_signal(name.as_str())
            }
            IrLValue::VifField { .. } => None,
            IrLValue::Concat(_) => None,
        }
    }

    pub(crate) fn is_forced(&self, lvalue: &IrLValue) -> bool {
        self.signal_id_from_lvalue(lvalue)
            .is_some_and(|id| self.forced_signals.contains(&id))
    }
}
