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
    fn layered_groups(
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
        // Par-02 tahap 1: dua fase agar urutan serial terjaga.
        //   Fase 1 (klasifikasi, tanpa evaluasi kandidat): hitung akses per
        //   kandidat triggered; tulisan kandidat non-batchable dikumpulkan.
        //   Fase 2 (urutan pid): kandidat serial/ditolak dieval inline pada
        //   posisi aslinya; hanya kandidat batchable yang TIDAK membaca
        //   sinyal tulisan kandidat serial yang ditunda ke batch — guards:
        //   (A) tolak intra-body RAW (evaluator paralel memperlakukan NBA
        //       sebagai blocking intra-body via overlay, serial tidak);
        //   (B) tolak kandidat batch yang membaca sinyal yang ditulis
        //       kandidat serial inline (snapshot post-fase-2 vs pre-tulis);
        //   (call) tolak FuncCall/MethodCall (evaluator paralel -> Err/X);
        //   (fused) bila ada kandidat cycle-fusion, TIDAK ada defer sama
        //       sekali round ini (commit domain sinkron mengubah urutan).
        let mut triggered: Vec<(usize, crate::scheduler::sim_dag::SignalAccess, bool)> = Vec::new();
        let mut serial_writes: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut saw_fused = false;
        for (pid, _clock, reset, iff, clock_trigger) in seq_candidates {
            // LANG-27: guard `iff (cond)` — evaluate with &mut self
            let trigger = clock_trigger
                && match &iff {
                    Some(cond) => match self.evaluate_expr(cond) {
                        Ok(v) => v.to_bool().unwrap_or(false),
                        Err(_) => false,
                    },
                    None => true,
                };
            // F40 fix: async reset edge
            let trigger = trigger
                || reset
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
            if !trigger {
                continue;
            }
            // ── Cycle-Based Fusion ──
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
            if let Some(domain) = fused_domain {
                self.evaluate_clock_domain(&domain)?;
                saw_fused = true;
                continue;
            }
            let access =
                crate::scheduler::sim_dag::analyze_process_access(&self.design.top.processes[pid]);
            let body_ok = self.process_body_arcs.get(&pid).is_some_and(|b| {
                crate::scheduler::sim_dag::is_sequential_body_batch_safe(b.as_slice())
            });
            let batchable = self.use_dag_parallel
                && body_ok
                && access.reads.is_disjoint(&access.writes)
                && !access.has_call_expr;
            if !batchable {
                serial_writes.extend(access.writes.iter().copied());
            }
            triggered.push((pid, access, batchable));
        }
        // Fase 2 — urutan pid, eval inline utk non-batch; defer utk batch.
        let mut seq_batch: Vec<(usize, crate::scheduler::sim_dag::SignalAccess)> = Vec::new();
        for (pid, access, batchable) in triggered {
            if !saw_fused && batchable && access.reads.is_disjoint(&serial_writes) {
                seq_batch.push((pid, access));
                continue;
            }
            // Clone body per-pid (only triggered, not all 176K)
            let body = match &self.design.top.processes[pid] {
                Process::Sequential { body, .. } => body.clone(),
                _ => continue,
            };
            self.evaluate_stmt_block(&body)?;
        }
        let batch_pids: Vec<usize> = seq_batch.iter().map(|(p, _)| *p).collect();
        if !saw_fused && batch_pids.len() >= 2 {
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

    /// Par-02 tahap 1: batch-parallel eval triggered Sequential processes
    /// (whitelist `is_sequential_body_batch_safe`, write set disjoint via
    /// `layered_groups`, guard RAW/call/serial-writes di situs pemanggil).
    /// Snapshot pre-edge SEKALI untuk semua grup — NBA pending tidak
    /// menyentuh storage current sebelum commit. Writes di-tag per-pid lalu
    /// di-push ke nba_pending terurut pid ascending = urutan push serial
    /// persis (WAW antar grup → last-write-wins identik serial).
    fn evaluate_sequential_batch_parallel(
        &mut self,
        batch: &[(usize, crate::scheduler::sim_dag::SignalAccess)],
    ) -> Result<(), SimError> {
        // (E) sinyal ultra-lebar: snapshot penuh berisiko OOM / lazy-X —
        // mirror penggerbang jalur comb (evaluate_eval_processes_parallel).
        if self.has_wide_signals() {
            let pids: Vec<usize> = batch.iter().map(|(p, _)| *p).collect();
            return self.evaluate_batch_serially(&pids);
        }
        // Analisis akses dense (batch pids -> index 0..n) — hindari alloc
        // access vec seukuran seluruh process design.
        let access: Vec<crate::scheduler::sim_dag::SignalAccess> =
            batch.iter().map(|(_, a)| a.clone()).collect();
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
        // Snapshot pre-edge sekali untuk semua grup.
        let snapshot: Vec<std::sync::Arc<LogicVec>> = (0..self.state.signals.len())
            .map(|i| std::sync::Arc::new(self.state.snapshot_signal(i)))
            .collect();
        let body_map: std::collections::HashMap<usize, std::sync::Arc<Vec<IrStmt>>> = batch
            .iter()
            .filter_map(|(pid, _)| {
                self.process_body_arcs
                    .get(pid)
                    .map(|b| (*pid, std::sync::Arc::clone(b)))
            })
            .collect();
        // (C) writes ter-tag per-pid; TIDAK ada push sebelum semua grup
        // selesai — Err grup mana pun (D) membatalkan seluruh hasil dan
        // fallback serial tanpa efek sisa.
        let mut tagged: Vec<(usize, Vec<crate::scheduler::sim_dag::ParallelWrite>)> = Vec::new();
        for group in &groups {
            let pids: Vec<&usize> = group.iter().map(|&i| &batch[i].0).collect();
            match crate::scheduler::sim_dag::evaluate_bodies_parallel_tagged(
                &pids,
                &body_map,
                &snapshot,
                &self.design.top.signals,
            ) {
                Ok(part) => tagged.extend(part),
                Err(_) => {
                    let pids: Vec<usize> = batch.iter().map(|(p, _)| *p).collect();
                    return self.evaluate_batch_serially(&pids);
                }
            }
        }
        tagged.sort_by_key(|(pid, _)| *pid);
        for (_, writes) in tagged {
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
