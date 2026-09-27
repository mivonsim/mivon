use super::SimulationEngine;
use super::SimulationLimit;
use super::EVENT_COMPACT_THRESHOLD;
use crate::foreign::ForeignEvent;
use crate::scheduler::clock_domain::ClockDomain;
use crate::simulator::parallel::ParallelConfig;
use crate::simulator::sdf::SdfData;
use crate::simulator::state::SimulationState;
use crate::simulator::types::*;
use crate::waveform::{CsvWaveWriter, FstWaveWriter, SignalStats, VcdWriter};
use mivon_core::diagnostics::diagnostic::{
    DiagCode, DiagLevel, Diagnostic, RuntimeContext, SourceSnippet,
};
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use rand::SeedableRng;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

impl SimulationEngine {
    pub fn new(design: IrDesign, max_time: u64) -> Self {
        Self::new_with_limit(design, SimulationLimit::Finite(max_time))
    }

    /// Konstruktor dengan batas eksplisit (`Unlimited` atau `Finite(n)`).
    pub fn new_with_limit(design: IrDesign, sim_limit: SimulationLimit) -> Self {
        let state = SimulationState::new(&design);
        SimulationEngine {
            state,
            coverage_exclusions: design.coverage_exclusions.clone(),
            stmt_lines: design.stmt_lines.clone(),
            design,
            sim_limit,
            signal_lookup: std::sync::OnceLock::new(),
            report_progress: false,
            running: true,
            fatal_hit: false,
            sev_info_count: 0,
            sev_warning_count: 0,
            sev_error_count: 0,
            sev_fatal_count: 0,
            cancel_flag: None,
            events: Vec::new(),
            events_base: 0,
            nba_pending: Vec::new(),
            nba_signal_map: HashMap::new(),
            auto_checkpoint: None,
            vcd: None,
            fst: None,
            csv: None,
            signal_stats: None,
            current_this: None,
            method_locals: Vec::new(),
            static_locals: HashMap::new(),
            current_method: None,
            disable_pending: None,
            rng: rand::rngs::StdRng::seed_from_u64(42),
            rand_call_count: 0,
            rand_seed: 42,
            current_scope_name: None,
            file_handles: HashMap::new(),
            file_ungetc_buf: HashMap::new(),
            file_read_pos: HashMap::new(),
            next_file_handle: 1,
            monitor_args: None,
            monitor_last_values: None,
            control_flow: None,
            expr_recursion_depth: 0,
            forced_signals: HashSet::new(),
            pre_force_values: HashMap::new(),
            signal_snapshot: None,
            preponed_snapshot: None,
            signal_seq_history: VecDeque::new(),
            coverage_snapshot: None,
            pending_waits: Vec::new(),
            pending_events: Vec::new(),
            pending_ast_events: Vec::new(),
            pending_await_target: None,
            pending_wait_orders: Vec::new(),
            loop_continuation: None,
            ast_loop_continuation: None,
            active_fork_id: None,
            task_suspended: false,
            post_loop_tail: Vec::new(),
            current_time: 0,
            fork_groups: Vec::new(),
            fork_free: Vec::new(),
            pending_wait_forks: Vec::new(),
            reactive_events: Vec::new(),
            strobe_events: Vec::new(),
            fstrobe_events: Vec::new(),
            fmonitor_map: HashMap::new(),
            mailbox_queues: HashMap::new(),
            mailbox_bounds: HashMap::new(),
            semaphore_counts: HashMap::new(),
            constraint_modes: HashMap::new(),
            static_constraint_modes: HashMap::new(),
            assoc_data: HashMap::new(),
            uvm_object_data: HashMap::new(),
            uvm_component_data: HashMap::new(),
            uvm_sequencer_data: HashMap::new(),
            uvm_driver_data: HashMap::new(),
            uvm_analysis_port_data: HashMap::new(),
            uvm_analysis_imp_data: HashMap::new(),
            uvm_config_db_data: HashMap::new(),
            uvm_config_db_waiters: HashMap::new(),
            uvm_event_data: HashMap::new(),
            uvm_barrier_data: HashMap::new(),
            uvm_cmdline_id: None,
            uvm_root_id: None,
            uvm_current_phase: None,
            uvm_phase_jump: None,
            uvm_phase_handle: None,
            uvm_tr_db_id: None,
            uvm_tr_streams: HashMap::new(),
            tr_stream_names: HashMap::new(),
            tr_db_default_stream: None,
            tr_records: Vec::new(),
            tr_open: HashMap::new(),
            tr_obj_stream: HashMap::new(),
            uvm_cmdline_last_value: String::new(),
            uvm_cmdline_values: Vec::new(),
            uvm_sync_waiters: HashMap::new(),
            uvm_tlm_fifo_data: HashMap::new(),
            uvm_fifo_export_data: HashMap::new(),
            uvm_comparator_data: HashMap::new(),
            uvm_heartbeat_data: HashMap::new(),
            uvm_seq_item_port_data: HashMap::new(),
            ast_fork_cont: HashMap::new(),
            sdf_timing_checks: Vec::new(),
            uvm_resource_db_data: HashMap::new(),
            uvm_reg_data: std::collections::HashMap::new(),
            uvm_reg_field_data: std::collections::HashMap::new(),
            uvm_reg_block_data: std::collections::HashMap::new(),
            uvm_reg_map_data: std::collections::HashMap::new(),
            callback_queues: HashMap::new(),
            factory_type_overrides: HashMap::new(),
            root_test_obj_id: None,
            uvm_phases_started: false,
            process_map: HashMap::new(),
            _next_process_id: 1,
            current_process_id: None,
            cover_hits: HashMap::new(),
            assertion_stats: HashMap::new(),
            sequence_coverage: HashMap::new(),
            cover_total: HashMap::new(),
            cover_bins: HashMap::new(),
            covergroup_prev: HashMap::new(),
            covergroup_const_bins: HashMap::new(),
            ast_loop_iters: 0,
            zero_delay_seen_t: 0,
            zero_delay_same_time: 0,
            last_processed_t: 0,
            same_time_revisits: 0,
            plusargs: HashMap::new(),
            debug_mode: DebugMode::Normal,
            breakpoints: Vec::new(),
            watchpoints: Vec::new(),
            signal_history: crate::simulator::signal_history::SignalHistoryStore::new(10000, None),
            signal_last_change: HashMap::new(),
            signal_last_dir: HashMap::new(),
            signal_prev_change: HashMap::new(),
            signal_prev_value: HashMap::new(),
            sdf_pulse_controls: HashMap::new(),
            timing_reported: HashMap::new(),
            udp_prev_args: HashMap::new(),
            parallel_config: ParallelConfig::default(),
            sysfunc_prev: HashMap::new(),
            sysfunc_history: HashMap::new(),
            snapshots: Vec::new(),
            paused: false,
            step_mode: StepMode::Running,
            event_log: Vec::new(),
            snapshot_interval: 1000,
            assert_off_all: false,
            assert_kill_all: false,
            assert_modules_off: HashSet::new(),
            coverage_options: HashMap::new(),
            coverage_enabled: true,
            coverage_enabled_types: std::collections::HashSet::new(),
            coverage_report_silent: false,
            trace_snapshots: Vec::new(),
            trace_interval: None,
            trace_last_time: u64::MAX,
            glitch_window: 0,
            glitch_prev: std::collections::HashMap::new(),
            cover_line: HashMap::new(),
            cover_toggle: HashMap::new(),
            cover_branches: HashMap::new(),
            cover_fsm: HashMap::new(),
            cover_branch_counter: 0,
            coverage_model_handles: HashMap::new(),
            next_coverage_model_handle: 1,
            sequence_attempts: Vec::new(),
            recursion_depth: HashMap::new(),
            max_recursion_depth: 256,
            ast_return_pending: false,
            objection_count: 0,
            objection_triggered: false,
            uvm_objection_data: std::collections::HashMap::new(),
            jit_evaluator: Some(crate::simulator::JITEvaluator::new()),
            use_packed_eval: true,
            use_jit_expression: false, // Expression-level JIT: disabled by default (opt-in for stability)
            sim_arena: crate::simulator::arena::SimulationArena::with_bump_size(4 * 1024 * 1024), // 4MB initial
            sim_dag: None,
            use_dag_parallel: false,
            resource_guard: crate::simulator::guard::SimResourceGuard::from_env(),
            clock_analysis: None,
            use_cycle_fusion: false,
            cycle_based: false,
            cycle_period: 10,
            mir_jit: mivon_compiler::mir::MirJitCompiler::new(),
            use_mir_jit: false,
            diag_sink: mivon_core::diagnostics::DiagSink::new(),
            current_delta: 0,
            current_process_name: None,
            current_instance_path: None,
            cur_src_line: std::cell::Cell::new(0),
            cur_src_col: std::cell::Cell::new(0),
            signal_writers: std::collections::HashMap::new(),
            signal_write_types: std::collections::HashMap::new(),
            signal_write_count: std::collections::HashMap::new(),
            // Delta limit default 100_000 (bukan 20M) — siklus delta > 100k
            // pada SATU time step adalah tanda kombinational loop / osilasi;
            // dengan limit 20M, kasus osilasi menghabiskan berjam-jam sebelum
            // error InfiniteDelta muncul. Naikkan via set_delta_limit bila
            // desain sah membutuhkan lebih banyak.
            delta_limit: 100_000,
            osc_state_hashes: std::collections::HashSet::new(),
            osc_last_state_hash: None,
            comb_access: Vec::new(),
            comb_access_ready: false,

            timing_wheel: None,
            use_timing_wheel: false,
            sim_perf: mivon_compiler::profiling::PerfDashboard::new(),

            cosim_state: None,
            cosim_signals: Vec::new(),
            foreign_events: Vec::new(),
            event_alloc_exceeded: false,
            signal_delays: std::collections::HashMap::new(),
            power_intent: None,
            process_body_cache: HashMap::new(),
        }
    }

    /// Batas numerik saat ini (u64::MAX bila unlimited) — kompatibilitas
    /// dengan API legacy yang masih bertipe u64.
    pub fn max_time_limit(&self) -> u64 {
        self.sim_limit.bound()
    }

    /// Buat RuntimeContext dari state engine saat ini.
    /// Menyertakan: time, delta, module, process, seed.
    pub fn runtime_context(&self) -> RuntimeContext {
        let mut ctx = RuntimeContext::new()
            .with_time(format!("{} ns", self.state.time))
            .with_delta(self.current_delta)
            .with_module(self.design.top.name.as_str());
        if let Some(ref pname) = self.current_process_name {
            ctx = ctx.with_process(pname.clone());
        }
        if let Some(ref inst) = self.current_instance_path {
            ctx = ctx.with_instance(inst.clone());
        }
        ctx
    }

    /// Catat posisi source terakhir yang diketahui (dari evaluasi ekspresi
    /// berposisi). Diabaikan bila line==0 (posisi tidak tersedia).
    pub fn set_cur_src_pos(&self, line: usize, col: usize) {
        if line > 0 {
            self.cur_src_line.set(line);
            self.cur_src_col.set(col);
        }
    }

    /// Posisi source terakhir yang diketahui — (0,0) bila belum ada.
    pub fn cur_src_pos(&self) -> (usize, usize) {
        (self.cur_src_line.get(), self.cur_src_col.get())
    }

    /// Lokasi source saat ini sebagai string "file:line:col" — None bila
    /// posisi belum diketahui (line==0). Dipakai emit_severity & pesan runtime
    /// lain yang tidak lewat DiagSink agar tetap menunjuk ke baris source.
    pub fn cur_src_loc_str(&self) -> Option<String> {
        let (line, col) = self.cur_src_pos();
        if line == 0 {
            return None;
        }
        let (file, display_line) = self.resolve_source_location(line);
        Some(format!("{}:{}:{}", file, display_line, col))
    }

    /// Emit diagnostic runtime ke DiagSink dengan lokasi source saat ini
    /// (file:line:col via resolve_source_location bila line>0). Level
    /// non-error/warning (Info/Note/Help/dll.) tetap dipertahankan — hanya
    /// Warning yang dipetakan ke diag_warn_at (F20).
    pub fn emit_diag(&self, level: DiagLevel, code: DiagCode, message: impl Into<String>) {
        let msg: String = message.into();
        let (line, col) = self.cur_src_pos();
        match level {
            DiagLevel::Error => {
                let _ = self.diag_error_at(code, msg, line, col);
            }
            DiagLevel::Fatal => {
                let _ = self.diag_fatal_at(code, msg, line, col);
            }
            DiagLevel::Warning => self.diag_warn_at(code, msg, line, col),
            _ => {
                // Level lain (Info/Note/Help/Bug/Debug/dll.): push dengan level
                // aslinya + snippet bila posisi tersedia (jangan coerce ke Warning).
                let mut diag = Diagnostic::new(level, code, msg)
                    .with_runtime_context(self.runtime_context())
                    .with_code_context();
                if line > 0 {
                    if let Some(ref source_lines) = self.design.source_lines {
                        if line <= source_lines.len() {
                            let source_line = &source_lines[line - 1];
                            let (file, display_line) = self.resolve_source_location(line);
                            diag = diag.with_source_snippet(SourceSnippet::new(
                                file,
                                display_line,
                                col,
                                source_line,
                            ));
                        }
                    }
                }
                self.diag_sink.push(diag);
            }
        }
    }

    /// Emit warning diagnostic ke DiagSink dengan lokasi source saat ini.
    pub fn emit_warning(&self, code: DiagCode, message: impl Into<String>) {
        let msg: String = message.into();
        let (line, col) = self.cur_src_pos();
        self.diag_warn_at(code, msg, line, col);
    }

    /// Resolve nama file sumber dari directive `` `line `` di merged source,
    /// scan mundur dari baris error (1-based). Fallback ke `source_file`.
    /// Resolve nama file + baris relatif-file dari directive `` `line `` di merged source.
    fn resolve_source_location(&self, line: usize) -> (String, usize) {
        let source_lines = self.design.source_lines.as_deref().unwrap_or(&[]);
        let default_file = self.design.source_file.as_deref().unwrap_or("<unknown>");
        mivon_core::diagnostics::resolve_source_location(source_lines, default_file, line)
    }

    /// Emit error diagnostic ke DiagSink dan return SimError dengan full context.
    pub fn diag_error(&self, code: DiagCode, message: impl Into<String>) -> SimError {
        let (line, col) = self.cur_src_pos();
        self.diag_error_at(code, message, line, col)
    }

    /// Emit error diagnostic dengan posisi source (line, col).
    pub fn diag_error_at(
        &self,
        code: DiagCode,
        message: impl Into<String>,
        line: usize,
        col: usize,
    ) -> SimError {
        let msg: String = message.into();
        let mut diag = Diagnostic::new(DiagLevel::Error, code, msg)
            .with_runtime_context(self.runtime_context())
            .with_code_context();
        // Add source snippet if source lines are available
        if line > 0 {
            if let Some(ref source_lines) = self.design.source_lines {
                if line <= source_lines.len() {
                    let source_line = &source_lines[line - 1];
                    let (file, display_line) = self.resolve_source_location(line);
                    diag = diag.with_source_snippet(SourceSnippet::new(
                        file,
                        display_line,
                        col,
                        source_line,
                    ));
                }
            }
        }
        self.diag_sink.push(diag.clone());
        SimError::Diagnostic(diag)
    }

    /// Emit warning diagnostic dengan posisi source (line, col).
    pub fn diag_warn_at(
        &self,
        code: DiagCode,
        message: impl Into<String>,
        line: usize,
        col: usize,
    ) {
        let msg: String = message.into();
        let mut diag = Diagnostic::new(DiagLevel::Warning, code, msg)
            .with_runtime_context(self.runtime_context())
            .with_code_context();
        if line > 0 {
            if let Some(ref source_lines) = self.design.source_lines {
                if line <= source_lines.len() {
                    let source_line = &source_lines[line - 1];
                    let (file, display_line) = self.resolve_source_location(line);
                    diag = diag.with_source_snippet(SourceSnippet::new(
                        file,
                        display_line,
                        col,
                        source_line,
                    ));
                }
            }
        }
        self.diag_sink.push(diag);
    }

    /// Emit fatal diagnostic ke DiagSink dan return SimError dengan full context.
    pub fn diag_fatal(&self, code: DiagCode, message: impl Into<String>) -> SimError {
        let (line, col) = self.cur_src_pos();
        self.diag_fatal_at(code, message, line, col)
    }

    /// Emit fatal diagnostic dengan posisi source.
    pub fn diag_fatal_at(
        &self,
        code: DiagCode,
        message: impl Into<String>,
        line: usize,
        col: usize,
    ) -> SimError {
        let msg: String = message.into();
        let mut diag = Diagnostic::new(DiagLevel::Fatal, code, msg)
            .with_runtime_context(self.runtime_context())
            .with_code_context();
        if line > 0 {
            if let Some(ref source_lines) = self.design.source_lines {
                if line <= source_lines.len() {
                    let source_line = &source_lines[line - 1];
                    let (file, display_line) = self.resolve_source_location(line);
                    diag = diag.with_source_snippet(SourceSnippet::new(
                        file,
                        display_line,
                        col,
                        source_line,
                    ));
                }
            }
        }
        self.diag_sink.push(diag.clone());
        SimError::Diagnostic(diag)
    }

    /// Flush diagnostics from DiagSink and return them.
    pub fn flush_diagnostics(&self) -> Vec<Diagnostic> {
        self.diag_sink.diagnostics()
    }

    pub fn set_vcd(&mut self, vcd: VcdWriter) {
        self.vcd = Some(vcd);
    }

    pub fn set_fst(&mut self, fst: FstWaveWriter) {
        self.fst = Some(fst);
    }

    pub fn set_csv(&mut self, csv: CsvWaveWriter) {
        self.csv = Some(csv);
    }

    pub fn set_signal_stats(&mut self, stats: SignalStats) {
        self.signal_stats = Some(stats);
    }

    pub fn set_parallel_config(&mut self, config: ParallelConfig) {
        self.parallel_config = config;
    }

    pub fn set_use_packed_eval(&mut self, enabled: bool) {
        self.use_packed_eval = enabled;
    }

    pub fn set_use_jit_expression(&mut self, enabled: bool) {
        self.use_jit_expression = enabled;
    }

    pub fn set_delta_limit(&mut self, limit: u64) {
        self.delta_limit = limit;
    }

    /// Sembunyikan laporan coverage verbose akhir-run (dipakai mivon-fuzz
    /// via `simulate_signals_with_coverage`; coverage keys tetap tersedia).
    pub fn set_coverage_report_silent(&mut self) {
        self.coverage_report_silent = true;
    }

    /// Aktifkan trace sampling per-interval-waktu (0/None = nonaktif).
    /// Snapshot sinyal top di-push ke `trace_snapshots` (opsional; dipakai
    /// mivon-fuzz fingerprint MID-SIMULATION).
    pub fn set_trace_interval(&mut self, interval: u64) {
        self.trace_snapshots.clear();
        self.trace_last_time = u64::MAX;
        self.trace_interval = if interval == 0 { None } else { Some(interval) };
    }

    /// Set glitch detection window (in time units). 0 = disabled.
    pub fn set_glitch_window(&mut self, window: u64) {
        self.glitch_window = window;
    }

    pub fn set_use_dag_parallel(&mut self, enabled: bool) {
        self.use_dag_parallel = enabled;
    }

    pub fn set_use_cycle_fusion(&mut self, enabled: bool) {
        self.use_cycle_fusion = enabled;
    }

    /// SIM-20: aktifkan mode cycle-based (`--cycle`). Desain yang tidak
    /// cocok otomatis fallback ke event-driven (pesan fallback dicetak).
    pub fn set_cycle_based(&mut self, enabled: bool) {
        self.cycle_based = enabled;
    }

    /// SIM-20: set periode clock mode cycle-based (unit waktu desain).
    pub fn set_cycle_period(&mut self, period: u64) {
        self.cycle_period = period.max(2);
    }

    pub fn set_use_mir_jit(&mut self, enabled: bool) {
        self.use_mir_jit = enabled;
    }

    /// SIM-18: aktifkan auto-checkpoint (crash recovery) — simpan state tiap
    /// `interval` cycle ke `path`. `interval=0` menonaktifkan.
    pub fn set_auto_checkpoint(&mut self, path: &str, interval: u64) {
        if interval == 0 {
            self.auto_checkpoint = None;
        } else {
            self.auto_checkpoint = Some((path.to_string(), interval));
        }
    }

    /// Ensure the events Vec is large enough to hold events at time `t`.
    /// Indeks relatif terhadap `events_base` (lihat `retire_events`).
    ///
    /// Guard OOM: delay ekstrem (`#9000000000000000000`) meminta resize
    /// miliaran slot → capacity-overflow panic / alokasi puluhan GB.
    /// Melebihi MAX_EVENT_SPAN → set `event_alloc_exceeded` (run loop akan
    /// abort dengan diagnostic), TIDAK mengalokasi.
    pub fn ensure_events(&mut self, t: usize) {
        let idx = t - self.events_base;
        if idx >= self.events.len() {
            if idx > crate::simulator::engine::MAX_EVENT_SPAN {
                self.event_alloc_exceeded = true;
                return;
            }
            self.events.resize(idx + 1, Vec::new());
        }
    }

    /// Reclaim slot event untuk time step `processed` setelah selesai diproses.
    /// 1) Bebaskan alokasi inner Vec slot yang sudah lewat.
    /// 2) Periodik: buang slot-slot leading yang sudah retired sehingga `events`
    ///    tidak tumbuh O(max_time). Aman karena event selalu dijadwalkan pada
    ///    waktu >= waktu saat ini (delay `#0`/`#N` tidak pernah ke masa lalu).
    pub fn retire_events(&mut self, processed: usize) {
        let idx = processed - self.events_base;
        if idx < self.events.len() {
            self.events[idx] = Vec::new();
        }
        if idx >= EVENT_COMPACT_THRESHOLD {
            self.events.drain(..idx);
            self.events_base += idx;
        }
    }

    /// Alokasikan slot ForkGroup — reuse slot retired bila ada (anti-leak
    /// untuk fork di dalam loop).
    pub(crate) fn alloc_fork_group(
        &mut self,
        remaining: usize,
        continuation: Vec<IrStmt>,
        reclaimable: bool,
    ) -> usize {
        let spawner = self.current_process_name.clone();
        if let Some(fid) = self.fork_free.pop() {
            self.fork_groups[fid] = ForkGroup {
                remaining,
                continuation,
                fired: false,
                active: true,
                reclaimable,
                spawner,
                disabled: false,
            };
            fid
        } else {
            let fid = self.fork_groups.len();
            self.fork_groups.push(ForkGroup {
                remaining,
                continuation,
                fired: false,
                active: true,
                reclaimable,
                spawner,
                disabled: false,
            });
            fid
        }
    }

    /// Retire ForkGroup yang sudah selesai: free continuation, tandai non-aktif,
    /// kembalikan slot ke free list. Idempoten (guard `active`).
    pub(crate) fn retire_fork_group(&mut self, fid: usize) {
        if fid < self.fork_groups.len() && self.fork_groups[fid].active {
            let g = &mut self.fork_groups[fid];
            g.active = false;
            g.fired = true;
            g.remaining = 0;
            g.continuation.clear();
            self.fork_free.push(fid);
        }
    }

    /// Decrement counter branch fork. Dipanggil hanya saat satu branch BENAR-BENAR
    /// selesai (all_consumed). Bila semua branch selesai, finish fork.
    pub(crate) fn fork_decrement(&mut self, fid: usize) -> Result<(), SimError> {
        if fid >= self.fork_groups.len() || !self.fork_groups[fid].active {
            return Ok(());
        }
        if self.fork_groups[fid].remaining > 0 {
            self.fork_groups[fid].remaining -= 1;
        }
        self.fork_finish(fid)
    }

    /// Finalisasi fork saat remaining == 0: eksekusi continuation (sekali via
    /// `fired`), lalu reclaim slot bila aman (reclaimable) atau hanya free
    /// continuation untuk JoinAny.
    pub(crate) fn fork_finish(&mut self, fid: usize) -> Result<(), SimError> {
        if fid >= self.fork_groups.len() || !self.fork_groups[fid].active {
            return Ok(());
        }
        if self.fork_groups[fid].remaining > 0 {
            return Ok(());
        }
        if !self.fork_groups[fid].fired {
            self.fork_groups[fid].fired = true;
            // F21: continuation AST (`fork join` di task/method UVM) disimpan
            // terpisah di ast_fork_cont — ForkGroup.continuation hanya Vec<IrStmt>.
            if let Some(ast_cont) = self.ast_fork_cont.remove(&fid) {
                if !ast_cont.is_empty() {
                    self.evaluate_ast_block_with_delay_fork(&ast_cont, None)?;
                    // F35 review: return di continuation fork (illegal SV)
                    // menandai ast_return_pending — clear agar tidak bocor.
                    self.ast_return_pending = false;
                }
            } else {
                let cont = std::mem::take(&mut self.fork_groups[fid].continuation);
                if !cont.is_empty() {
                    self.evaluate_block_with_delay_fork(&cont, None)?;
                }
            }
        }
        // LANG-29: group ini baru saja selesai — resume `wait fork` yang
        // menunggunya (bila semua fid yang ditunggu sudah selesai).
        self.check_wait_forks(fid)?;
        if self.fork_groups[fid].reclaimable {
            self.retire_fork_group(fid);
        } else {
            self.fork_groups[fid].continuation.clear();
        }
        Ok(())
    }

    /// LANG-29: pilih fork group yang masih berjalan (remaining > 0, active)
    /// milik proses ini (`spawner == current_process_name`). Group JoinAny yang
    /// sudah selesai (remaining==0) TIDAK ikut — branch yang kalah balapan tidak
    /// dilacak (keterbatasan akuntansi fork engine).
    pub(crate) fn active_fork_groups_for_current_process(&self) -> Vec<usize> {
        let pname = self.current_process_name.clone();
        self.fork_groups
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                g.active && g.remaining > 0 && g.spawner.as_deref() == pname.as_deref()
            })
            .map(|(fid, _)| fid)
            .collect()
    }

    /// LANG-32/33: cek apakah constraint block aktif. Block STATIC dicek di
    /// `static_constraint_modes` (key class+block — global antar semua
    /// instance, IEEE 1800-2017 §18.5.10); block biasa dicek di
    /// `constraint_modes` (key obj+block). Absen = enabled (default).
    pub(crate) fn constraint_block_enabled(
        &self,
        obj_id: ObjId,
        class_name: Symbol,
        block_name: Symbol,
        is_static: bool,
    ) -> bool {
        if is_static {
            self.static_constraint_modes
                .get(&(class_name, block_name))
                .copied()
                .unwrap_or(true)
        } else {
            self.constraint_modes
                .get(&(obj_id, block_name))
                .copied()
                .unwrap_or(true)
        }
    }

    /// LANG-29: pangkas fid yang sudah selesai dari semua `wait fork` yang
    /// menunggu; entry yang kehabisan fid di-resume (kontinuasi IR atau AST).
    fn check_wait_forks(&mut self, fid: usize) -> Result<(), SimError> {
        let mut resumed: Vec<WaitForkState> = Vec::new();
        let mut remaining = Vec::new();
        for mut wf in std::mem::take(&mut self.pending_wait_forks) {
            wf.fids.retain(|&f| f != fid);
            if wf.fids.is_empty() {
                resumed.push(wf);
            } else {
                remaining.push(wf);
            }
        }
        // Kontinuasi `wait fork` baru yang didaftarkan saat resume — jangan timpa.
        let newly_pushed = std::mem::take(&mut self.pending_wait_forks);
        remaining.extend(newly_pushed);
        self.pending_wait_forks = remaining;
        for wf in resumed {
            // LANG-29: restore nama proses pemilik wait fork — fork_finish
            // berjalan di luar EvalProcess sehingga current_process_name bisa
            // menunjuk proses lain.
            if let Some(pn) = &wf.process_name {
                self.current_process_name = Some(pn.clone());
            }
            if !wf.ast_continuation.is_empty() {
                // Jalur AST (task/method): restore konteks sebelum resume.
                let old_this = self.current_this;
                let old_method = self.current_method;
                let _old_locals = std::mem::replace(&mut self.method_locals, wf.locals.clone());
                self.current_this = wf.this;
                self.current_method = wf.method;
                let completed =
                    self.evaluate_ast_block_with_delay_fork(&wf.ast_continuation, None)?;
                self.ast_return_pending = false;
                if completed {
                    let keep = wf.base_len.saturating_sub(1).min(self.method_locals.len());
                    self.method_locals.truncate(keep);
                    self.current_this = old_this;
                    self.current_method = old_method;
                }
            } else if !wf.continuation.is_empty() {
                self.evaluate_block_with_delay_fork(&wf.continuation, None)?;
            }
        }
        Ok(())
    }

    /// Push an event at time `t` with dynamic allocation.
    /// When `use_timing_wheel` is enabled, events at future times (t > current_time)
    /// are stored in the hierarchical timing wheel for O(1) scheduling.
    /// Events at the current time are still stored in `events[t]` directly
    /// so the delta loop can process them without wheel advance overhead.
    pub fn push_event(&mut self, t: usize, event: RegionEvent) {
        if self.use_timing_wheel {
            if let Some(ref mut wheel) = self.timing_wheel {
                let current = self.state.time as usize;
                if t > current {
                    // Future event — store in timing wheel
                    wheel.add_event(t, event.region, event.event);
                    return;
                }
                // Current or past time — fall through to events[t]
            }
        }
        let idx = t - self.events_base;
        if idx >= self.events.len() {
            self.ensure_events(t);
            // ensure_events bisa menolak (span ekstrem) — jangan index.
            if idx >= self.events.len() {
                return;
            }
        }
        self.events[t - self.events_base].push(event);
    }

    /// Enable the hierarchical timing wheel for O(1) event scheduling.
    /// Call before run(). Allocates the wheel on first use.
    pub fn set_use_timing_wheel(&mut self, enabled: bool) {
        self.use_timing_wheel = enabled;
        if enabled && self.timing_wheel.is_none() {
            self.timing_wheel = Some(
                crate::simulator::engine::scheduler::timing_wheel::HierarchicalTimingWheel::new(),
            );
        }
    }

    /// Cek flag pembatalan eksternal (GUI "Stop"). Dipanggil di run loop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Check if src_class is same as or subclass of dest_class.
    /// Used by $cast for dynamic class handle casting.
    pub(crate) fn is_subclass_or_same(&self, src_class: Symbol, dest_class: Symbol) -> bool {
        if src_class == dest_class {
            return true;
        }
        // Walk up the inheritance chain
        let mut current = src_class;
        while let Some(class_def) = self.design.classes.get(&current) {
            if let Some(parent) = class_def.extends {
                if parent == dest_class {
                    return true;
                }
                current = parent;
            } else {
                break;
            }
        }
        false
    }

    /// Queue a foreign event (VPI/VHPI/PLI/DPI) for processing in the scheduler.
    /// Events are processed in the appropriate IEEE 1800 region.
    pub fn queue_foreign_event(&mut self, event: ForeignEvent) {
        self.foreign_events.push(event);
    }

    /// Queue multiple foreign events at once.
    pub fn queue_foreign_events(&mut self, events: Vec<ForeignEvent>) {
        self.foreign_events.extend(events);
    }

    pub fn run(&mut self) -> Result<(), SimError> {
        // ── VPI: Register engine for VPI callbacks ──
        crate::vpi::set_vpi_engine(self);
        crate::vpi::callback::dispatch_start_of_simulation();
        // ── VHPI (IEEE 1076-2008): engine hook — library VHDL eksternal
        // membutuhkan akses object Mivon selama sim (vhpi_handle_by_name dll).
        crate::vhpi::object::set_vhpi_engine(self);
        crate::vhpi::api::dispatch_start_of_simulation();
        // Guard: pastikan VPI/VHPI engine ter-deregistrasi di semua path
        // keluar `run()` (lihat ForeignEngineGuard).
        let _foreign_engine_guard = crate::foreign::ForeignEngineGuard;

        self.initialize_time_zero()?;
        // F19: auto-detect fase UVM HANYA bila source TIDAK memanggil
        // run_test() eksplisit. Sebelumnya execute_phases() selalu dipanggil
        // di sini (sebelum event loop) dan menang duluan: class phase dipilih
        // asal dari iterasi HashMap, lalu guard uvm_phases_started memblokir
        // `initial run_test("my_test")` — test build_phase (mis. berisi
        // uvm_config_db::set) tidak pernah dieksekusi. Deteksi eksplisit
        // membuat run_test user menang; tanpa run_test, auto-detect tetap jalan.
        if !self.design_has_explicit_run_test() {
            self.execute_phases()?;
        }

        // ── Register thread-local arena untuk zero-deallocation ──
        // Semua LogicVec::new(), fill(), from_u64() otomatis alokasi dari arena
        // selama event loop berjalan. Tidak perlu ubah evaluate_expr() call sites.
        crate::simulator::arena::set_thread_arena(Some(&mut self.sim_arena));
        // RAII guard: arena di-deregister otomatis saat run() keluar (termasuk
        // early-return error) — cegah pointer menggantung ke sim_arena.
        let _arena_guard = crate::simulator::arena::ArenaGuard;

        // ── Build DAG untuk parallel process evaluation ──
        // Hanya untuk Combinational/CombReactive processes yang aman di-paralelkan.
        if self.use_dag_parallel && self.sim_dag.is_none() {
            let dag = crate::scheduler::SimulationDag::build(&self.design);
            if dag.num_processes() > 0 {
                let n_layers = dag.num_layers();
                let avg_par = dag.avg_parallelism();
                eprintln!(
                    "DAG: {} processes, {} layers, avg {:.1} parallelism",
                    dag.num_processes(),
                    n_layers,
                    avg_par
                );
                self.sim_dag = Some(dag);
            }
        }

        // ── Build clock domains untuk cycle-based simulation fusion ──
        if self.use_cycle_fusion && self.clock_analysis.is_none() {
            let analysis = crate::scheduler::ClockDomainAnalysis::analyze(&self.design);
            if analysis.num_domains() > 0 {
                eprintln!(
                    "Cycle fusion: {} clock domains, {} processes fused",
                    analysis.num_domains(),
                    analysis.num_fused_processes(),
                );
                self.clock_analysis = Some(analysis);
            }
        }

        // ── Start co-simulation server if configured ──
        if let Some(ref cosim_state) = self.cosim_state {
            if let Ok(mut cs) = cosim_state.lock() {
                cs.current_time = self.state.time;
            }
        }

        // ── Build process body cache for DAG parallel eval ──
        // Hindari clone bodies setiap cycle dengan cache sekali.
        if self.use_dag_parallel {
            self.process_body_cache.clear();
            for (pid, process) in self.design.top.processes.iter().enumerate() {
                if crate::scheduler::sim_dag::is_process_parallelizable(process) {
                    if let Some(body) = match process {
                        Process::Combinational { body, .. }
                        | Process::CombReactive { body, .. }
                        | Process::Initial { body, .. } => Some(body.clone()),
                        _ => None,
                    } {
                        self.process_body_cache.insert(pid, body);
                    }
                }
            }
        }

        // ── Progress + stall detection ──
        // Laporan berkala tiap `PROGRESS_INTERVAL` tick ke stderr; deteksi
        // stall: tidak ada event yang diproses selama `STALL_WALL_LIMIT`
        // wall-clock → peringatan (bukan langsung mematikan simulasi).
        let progress_interval: u64 = 1_000_000;
        let stall_wall_limit = std::time::Duration::from_secs(10);
        /// Interval pemeriksaan quiescence (amortisasi scan slot events).
        const QUIESCENCE_CHECK_INTERVAL: u64 = 16;
        let mut last_report_wall = std::time::Instant::now();
        let mut last_active_wall = std::time::Instant::now();
        let mut stall_warned = false;

        // ── SIM-20: cycle-based simulation mode (opt-in --cycle) ──
        // Clock didrive internal scheduler tanpa iterasi delta; desain yang
        // tidak cocok (multi-clock, timed wait) → Ok(false) → fallback
        // event-driven di bawah. End-of-run cleanup (final blocks, reports,
        // VPI/VHPI cleanup) TETAP jalan lewat tail run() yang sama.
        let mut cycle_mode_done = false;
        if self.cycle_based {
            cycle_mode_done = crate::simulator::engine::cycle_based::run_cycle_based(self)?;
        }

        while !cycle_mode_done
            && self.running
            && self.sim_limit.allows(self.state.time)
            && !self.is_cancelled()
        {
            let step_start_events = self.sim_perf.counters.events_processed;
            let t = self.state.time as usize;

            // ── Guard HANG-1: zero-advance time-step revisit (BUG FIX fuZZ) ──
            // `#0`/zero-delay di dalam loop (`forever #0 clk = ~clk`) tidak
            // pernah maju waktu; event-nya diproses di iterasi OUTER berikutnya
            // (delta_count di-reset tiap iterasi, line 1034) sehingga delta-limit
            // 100k tidak pernah kena → simulasi hang sejati (>12s, terkonfirmasi
            // fuzz MV: `forever { #0 clk = ~clk }`). Guard: akumulasi revisit
            // SAME-TIME; lewat delta_limit → InfiniteDelta (sama dgn delta-kap).
            if t as u64 == self.last_processed_t {
                self.same_time_revisits += 1;
                if self.same_time_revisits > self.delta_limit {
                    let mut diag = Diagnostic::new(
                        DiagLevel::Error,
                        DiagCode::InfiniteDelta,
                        format!(
                            "simulation exceeded zero-advance time-step limit ({}) at time {} — kemungkinan loop dengan `#0`/delay nol tanpa time advance (mis. `forever #0 clk = ~clk`). Periksa desain atau naikkan limit via set_delta_limit()",
                            self.delta_limit, self.state.time
                        ),
                    );
                    diag = diag.with_runtime_context(
                        RuntimeContext::new()
                            .with_time(format!("{} ns", self.state.time))
                            .with_delta(self.same_time_revisits)
                            .with_module(self.current_instance_path.as_deref().unwrap_or("top")),
                    );
                    return Err(SimError::Diagnostic(diag));
                }
            } else {
                self.same_time_revisits = 0;
                self.last_processed_t = t as u64;
            }

            // ── Layer resource guard (MIVON-SIM-34): poll RSS tiap interval
            // untuk mencegah kernel OOM-kill pada design besar (OpenTitan).
            if self.resource_guard.is_enabled()
                && (t as u64).is_multiple_of(self.resource_guard.check_interval())
            {
                self.resource_guard.poll(self.state.time, t as u64)?;
            }

            // ── Guard: event di luar jendela alokasi → abort graceful ──
            if self.event_alloc_exceeded {
                return Err(SimError::with_diag(
                    DiagCode::InternalError,
                    format!(
                        "delay/event scheduled beyond MAX_EVENT_SPAN ({} ticks from current window) \
                         — gunakan delay lebih kecil atau aktifkan timing wheel",
                        10_000_000
                    ),
                ));
            }
            let base = self.events_base;

            // ── Timing wheel: advance to current time ──
            // Populates events[t] with all events scheduled for this time step
            // from the hierarchical timing wheel. Only uses wheel if enabled.
            if self.use_timing_wheel {
                if let Some(ref mut wheel) = self.timing_wheel {
                    let wheel_events = wheel.advance(t);
                    if !wheel_events.is_empty() {
                        self.ensure_events(t);
                        self.events[t - base].extend(wheel_events);
                    }
                }
            }

            // ── DPI: update thread-local scope path and simulation time ──
            #[cfg(feature = "dpi")]
            if let Some(ref path) = self.current_instance_path {
                crate::simulator::dpi::set_current_dpi_scope(path);
            }
            #[cfg(feature = "dpi")]
            crate::simulator::dpi::set_current_dpi_time(self.state.time);

            // ── Zero-deallocation: reset cycle arena (O(1) — bump pointer reset) ──
            self.sim_arena.reset_cycle();

            // Anti-RSS-akumulasi arena glibc pada mesin kecil: trim free pages
            // periodik (overhead <ms, hanya saat arena punya sisa besar).
            #[cfg(all(target_os = "linux", not(target_env = "musl")))]
            if self.current_delta.is_multiple_of(200_000) {
                unsafe { libc::malloc_trim(0) };
            }

            // ── Preponed region: initial snapshot for edge detection ──
            // Updated every delta cycle for correct edge detection (Sched-04 fix).
            // Sinyal ultra-lebar di-snapshot LAZY (tanpa materialisasi) —
            // clone penuh 1.5e9 bit tiap delta = OOM (preponed + signal +
            // seq_history × N salinan hidup).
            let num_sigs = self.state.signals.len();
            let mut snapshot = Vec::with_capacity(num_sigs);
            for i in 0..num_sigs {
                snapshot.push(self.state.snapshot_signal(i));
            }
            self.preponed_snapshot = Some(snapshot.clone());
            self.signal_snapshot = Some(snapshot.clone());
            // Simpan ke history untuk evaluator sequence temporal (##N)
            self.signal_seq_history.push_front(snapshot);
            const MAX_SEQ_DEPTH: usize = 8;
            while self.signal_seq_history.len() > MAX_SEQ_DEPTH {
                self.signal_seq_history.pop_back();
            }
            // SIM-30: coverage snapshot hanya di-capture di awal time step (tidak
            // di-refresh per delta). Diff-nya vs state akhir dipakai untuk toggle/FSM
            // coverage — kalau ikut signal_snapshot (refresh per delta), diff selalu kosong.
            // Gate: hanya clone saat Toggle/FSM coverage aktif (set kosong = semua tipe
            // aktif). Hindari O(N) clone per time step saat coverage tak dipakai.
            let need_cov_snap = self.coverage_enabled
                && (self.coverage_enabled_types.is_empty()
                    || self.coverage_enabled_types.contains(&CoverageType::Toggle)
                    || self.coverage_enabled_types.contains(&CoverageType::Fsm));
            self.coverage_snapshot = if need_cov_snap {
                self.signal_snapshot.clone()
            } else {
                None
            };

            self.dump_vcd_time()?;
            self.dump_fst_time()?;

            // ── IEEE 1800 stratified event loop ──
            self.sim_perf.counters.time_steps += 1;
            let mut delta_count = 0u64;
            crate::dbg_sim!(
                1,
                "time-step {} delta-loop start (events[same-time]={})",
                t,
                self.events.len()
            );
            loop {
                self.sim_perf.counters.delta_cycles += 1;
                crate::dbg_sim!(3, "t={} delta={} region-loop", t, delta_count);
                let mut activity = false;
                let mut deltas: Vec<SignalId> = Vec::new();

                for &region in &IEEE_REGIONS[..] {
                    match region {
                        EventRegion::Preponed => {
                            // Already handled above — skip during re-circulation
                        }
                        EventRegion::PreActive
                        | EventRegion::PreNba
                        | EventRegion::PostNba
                        | EventRegion::PreObserved
                        | EventRegion::PostObserved
                        | EventRegion::PostReactive => {
                            // PLI regions: process any events in this region
                            self.ensure_events(t);
                            let mut matched = true;
                            while matched {
                                matched = false;
                                let mut to_process = Vec::new();
                                self.events[t - base].retain(|re| {
                                    if re.region == region {
                                        to_process.push(re.event.clone());
                                        false
                                    } else {
                                        true
                                    }
                                });
                                self.sim_perf.counters.events_processed += to_process.len() as u64;
                                if !to_process.is_empty() {
                                    activity = true;
                                    matched = true;
                                    for event in to_process {
                                        self.process_event(event, t)?;
                                    }
                                }
                            }
                        }
                        EventRegion::Postponed => {
                            // Postponed region: process once per time step, does NOT re-circulate
                            self.ensure_events(t);
                            let mut to_process = Vec::new();
                            self.events[t - base].retain(|re| {
                                if re.region == EventRegion::Postponed {
                                    to_process.push(re.event.clone());
                                    false
                                } else {
                                    true
                                }
                            });
                            if !to_process.is_empty() {
                                for event in to_process {
                                    self.process_event(event, t)?;
                                }
                            }
                        }
                        EventRegion::Observed => {
                            // Observed region: evaluate concurrent assertions (SVA).
                            // Process any assertion-evaluation events scheduled here.
                            self.ensure_events(t);
                            let mut matched = true;
                            while matched {
                                matched = false;
                                let mut to_process = Vec::new();
                                self.events[t - base].retain(|re| {
                                    if re.region == EventRegion::Observed {
                                        to_process.push(re.event.clone());
                                        false
                                    } else {
                                        true
                                    }
                                });
                                if !to_process.is_empty() {
                                    activity = true;
                                    matched = true;
                                    for event in to_process {
                                        self.process_event(event, t)?;
                                    }
                                }
                            }
                        }
                        EventRegion::Active | EventRegion::Inactive => {
                            self.ensure_events(t);
                            loop {
                                let events: Vec<RegionEvent> = self.events[t - base]
                                    .drain(..)
                                    .filter(|re| re.region == region)
                                    .collect();
                                self.sim_perf.counters.events_processed += events.len() as u64;
                                if events.is_empty() {
                                    break;
                                }
                                activity = true;

                                // ── DAG-Parallel: batch EvalProcess events ──
                                // Hanya Combinational/CombReactive/Initial yang aman
                                // di-paralelkan. Sequential, AlwaysWithDelay, dan Final
                                // butuh event loop semantics (clock edges, delay).
                                if self.use_dag_parallel && self.sim_dag.is_some() {
                                    let mut eval_pids: Vec<usize> = Vec::new();
                                    let mut other_events: Vec<RegionEvent> = Vec::new();
                                    for re in events {
                                        if let EventKind::EvalProcess(pid) = re.event {
                                            if pid < self.design.top.processes.len() && {
                                                // Safe access with bounds check
                                                let process = self
                                                    .design
                                                    .top
                                                    .processes
                                                    .get(pid)
                                                    .expect(
                                                    "process pid bounds check failed in DAG loop",
                                                );
                                                crate::scheduler::is_process_parallelizable(process)
                                            } {
                                                eval_pids.push(pid);
                                            } else {
                                                other_events.push(re);
                                            }
                                        } else {
                                            other_events.push(re);
                                        }
                                    }

                                    // Process non-EvalProcess events (sequential, initial, always-with-delay)
                                    // TERLEBIH DAHULU: initial menulis input
                                    // (mis. `a = 8'hF0`) yang DIBACA process comb
                                    // parallel di delta yg sama — snapshot comb
                                    // harus dapat nilai BARU. Reorder parallel-
                                    // dulu (percobaan utk reset-counter) SILAH
                                    // SAAT comb membaca initial-write: comb
                                    // baca stale (0/X) → hasil salah (generate
                                    // `b[i]=~a[i]` DAG=0 vs default=15).
                                    // Reset-counter justru ter-fix oleh
                                    // initial-timing-serial (EventControl/Delay
                                    // → is_process_parallelizable=false), bukan
                                    // urutan ini.
                                    for re in other_events {
                                        self.process_event(re.event, t)?;
                                    }

                                    // Process EvalProcess events via DAG parallel
                                    if !eval_pids.is_empty() {
                                        // SIM-25: jalur DAG-parallel tidak lewat
                                        // process_event → hitung di sini agar counter
                                        // processes_evaluated akurat.
                                        self.sim_perf.counters.processes_evaluated +=
                                            eval_pids.len() as u64;
                                        self.evaluate_eval_processes_parallel(&eval_pids)?;
                                    }
                                } else {
                                    // Sequential: process all events one by one
                                    for re in events {
                                        self.process_event(re.event, t)?;
                                    }
                                }

                                // Inactive re-drains; Active drains once (outer loop
                                // re-circulates if new events appear later)
                                if region == EventRegion::Active {
                                    break;
                                }
                            }
                        }
                        EventRegion::Nba => {
                            // NBA region: commit pending non-blocking assignments
                            self.commit_nba();
                            self.sim_perf.counters.nba_commits += 1;
                            self.ensure_events(t);
                            let events: Vec<RegionEvent> = self.events[t - base]
                                .drain(..)
                                .filter(|re| re.region == EventRegion::Nba)
                                .collect();
                            self.sim_perf.counters.events_processed += events.len() as u64;
                            if !events.is_empty() {
                                activity = true;
                                for re in events {
                                    self.process_event(re.event, t)?;
                                }
                            }
                        }
                        EventRegion::Reactive => {
                            // Commit changes and trigger sensitive processes
                            let changed = self.state.commit_changes();
                            if !changed.is_empty() {
                                activity = true;
                                crate::dbg_sim!(
                                    3,
                                    "t={} delta={} commit: {} signal change(s)",
                                    t,
                                    delta_count,
                                    changed.len()
                                );
                                if crate::simulator::dbg_sim_level() >= 4 {
                                    let names: Vec<String> = changed
                                        .iter()
                                        .take(16)
                                        .map(|(id, _, _)| {
                                            self.design
                                                .top
                                                .signals
                                                .get(*id)
                                                .map(|s| s.name.as_str().to_string())
                                                .unwrap_or_else(|| format!("#{}", id))
                                        })
                                        .collect();
                                    crate::dbg_sim!(4, "  changed: {:?}", names);
                                }
                                for (id, _, _) in &changed {
                                    if !deltas.contains(id) {
                                        deltas.push(*id);
                                    }
                                }
                                self.sim_perf.counters.sensitive_triggers += 1;
                                // ── Foreign value-change callback (VPI cbValueChange
                                // + VHPI vhpiCbValueChange) — signal yang berubah
                                // di-fire sebagai ForeignEvent::ValueChange ke
                                // scheduler (poin 5 arsitektur user), bukan dari
                                // thread library.
                                for (id, old_v, new_v) in &changed {
                                    if let Some(sig) = self.design.top.signals.get(*id) {
                                        let name = sig.name.as_str();
                                        // VPI callback memakai t_vpi_value —
                                        // bangun dari LogicVec (format IntVal).
                                        let old_vpi = crate::vpi::types::t_vpi_value {
                                            format: crate::vpi::types::vpiIntVal,
                                            value: crate::vpi::types::vpi_value_union {
                                                integer: old_v.to_u64() as i32,
                                            },
                                        };
                                        let new_vpi = crate::vpi::types::t_vpi_value {
                                            format: crate::vpi::types::vpiIntVal,
                                            value: crate::vpi::types::vpi_value_union {
                                                integer: new_v.to_u64() as i32,
                                            },
                                        };
                                        crate::vpi::callback::fire_value_change_callbacks(
                                            name, &old_vpi, &new_vpi,
                                        );
                                        crate::vhpi::callback::fire_value_change_callbacks(
                                            *id, old_v, new_v,
                                        );
                                    }
                                }
                                self.trigger_sensitive_processes(&changed, t)?;
                            }
                            // Process Reactive events (from events[t] and reactive_events buffer)
                            self.ensure_events(t);
                            let events: Vec<RegionEvent> = self.events[t - base]
                                .drain(..)
                                .filter(|re| re.region == EventRegion::Reactive)
                                .collect();
                            self.sim_perf.counters.events_processed += events.len() as u64;
                            if !events.is_empty() {
                                activity = true;
                                for re in events {
                                    self.process_event(re.event, t)?;
                                }
                            }
                            let buffered: Vec<EventKind> =
                                std::mem::take(&mut self.reactive_events);
                            if !buffered.is_empty() {
                                activity = true;
                                for event in buffered {
                                    self.process_event(event, t)?;
                                }
                            }
                        }
                    }
                }

                if delta_count > self.delta_limit {
                    let changed_names: Vec<String> = deltas
                        .iter()
                        .take(16)
                        .map(|id| {
                            self.design
                                .top
                                .signals
                                .get(*id)
                                .map(|s| s.name.as_str().to_string())
                                .unwrap_or_else(|| format!("#{}", id))
                        })
                        .collect();
                    let mut diag = Diagnostic::new(
                        DiagLevel::Error,
                        DiagCode::InfiniteDelta,
                        format!(
                            "simulation exceeded max delta cycles per time step ({}) at time {} — kemungkinan kombinational loop / osilasi (signal tidak stabil dalam satu timestep). Periksa `always_comb`/`assign` yang membentuk loop, atau naikkan limit via set_delta_limit()",
                            self.delta_limit, self.state.time
                        ),
                    );
                    if !changed_names.is_empty() {
                        diag = diag.with_note(format!(
                            "sinyal terakhir berubah: {}",
                            changed_names.join(", ")
                        ));
                    }
                    diag = diag.with_runtime_context(
                        RuntimeContext::new()
                            .with_time(format!("{} ns", self.state.time))
                            .with_delta(delta_count)
                            .with_module(self.current_instance_path.as_deref().unwrap_or("top")),
                    );
                    return Err(SimError::Diagnostic(diag));
                }
                let report_interval = if self.delta_limit >= 100_000 {
                    100_000
                } else {
                    self.delta_limit / 10
                }
                .max(1);
                if delta_count > 0 && delta_count.is_multiple_of(report_interval) {
                    eprintln!(
                        "warning: {} delta cycles at time {} (limit {})",
                        delta_count, self.state.time, self.delta_limit
                    );
                }
                delta_count += 1;
                self.current_delta = delta_count;
                // ── Layer resource guard (MIVON-SIM-34): poll RSS tiap delta
                // pada delta KUMULATIF awal (lonjakan time-0 sering terjadi di
                // delta kecil, mis. big_30000 RSS 1.5GB dalam delta=2), lalu
                // tiap 256 delta setelahnya agar overhead baca /proc tidak
                // memberatkan. PENTING: pakai delta_cycles (kumulatif lintas
                // time step) — `delta_count` RESET tiap time step sehingga
                // kondisi lama `delta_count < 1024` selalu true → baca
                // /proc/self/status (~37k instr + alokasi) SETIAP delta tiap
                // step (probe perf2: 14.8% instruction di check_rss).
                let cum_delta = self.sim_perf.counters.delta_cycles;
                if self.resource_guard.is_enabled()
                    && (cum_delta < 1024 || cum_delta.is_multiple_of(256))
                {
                    self.resource_guard.check_limit(self.state.time)?;
                }

                // ── Oscillation detection (SIM-28) ──
                // Kombinational loop (cycle) membuat state sinyal berulang
                // non-kontigu dalam satu time step. Deteksi via hash state
                // commit: catat urutan state BERBEDA; bila hash yang sama
                // muncul lagi setelah berubah → cycle → abort cepat, tanpa
                // menunggu delta_limit (yang di desain osilasi bisa berjam-jam).
                // Plateau (state stabil, hanya event yang churn) TIDAK dihitung
                // sebagai osilasi (hash hanya di-insert saat berubah).
                // Dihitung mulai delta ke-1000 agar simulasi normal (settle
                // cepat) tidak pernah membayar biaya hash.
                if delta_count >= 1000 {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    self.state.signals.hash(&mut h);
                    let hv = h.finish();
                    if self.osc_last_state_hash != Some(hv) {
                        if !self.osc_state_hashes.insert(hv) {
                            // Kumpulkan nama sinyal yang berubah di delta ini
                            // untuk diagnostic — membantu user melacak loop.
                            let changed_names: Vec<String> = deltas
                                .iter()
                                .take(16)
                                .map(|id| {
                                    self.design
                                        .top
                                        .signals
                                        .get(*id)
                                        .map(|s| s.name.as_str().to_string())
                                        .unwrap_or_else(|| format!("#{}", id))
                                })
                                .collect();
                            // Kumpulkan process writer untuk sinyal berubah —
                            // petunjuk ke always_comb/assign yang membentuk loop.
                            let mut writers: Vec<String> = Vec::new();
                            for id in deltas.iter().take(16) {
                                if let Some(&Some(writer_id)) = self.signal_writers.get(id) {
                                    let pname = if let Some(obj) = self.state.get_object(writer_id)
                                    {
                                        obj.class_name.as_str().to_string()
                                    } else {
                                        format!("process#{}", writer_id)
                                    };
                                    if !writers.contains(&pname) {
                                        writers.push(pname);
                                    }
                                }
                            }
                            let mut diag = Diagnostic::new(
                                DiagLevel::Error,
                                DiagCode::InfiniteDelta,
                                format!(
                                    "kombinational loop / osilasi terdeteksi: state sinyal berulang pada delta {} di time {} (cycle). Periksa always_comb/assign yang membentuk feedback tanpa state.",
                                    delta_count, self.state.time
                                ),
                            );
                            if !changed_names.is_empty() {
                                diag = diag.with_note(format!(
                                    "sinyal berubah: {}",
                                    changed_names.join(", ")
                                ));
                            }
                            if !writers.is_empty() {
                                diag = diag
                                    .with_note(format!("process penulis: {}", writers.join(", ")));
                            }
                            diag = diag.with_runtime_context(
                                RuntimeContext::new()
                                    .with_time(format!("{} ns", self.state.time))
                                    .with_delta(delta_count)
                                    .with_module(
                                        self.current_instance_path.as_deref().unwrap_or("top"),
                                    ),
                            );
                            return Err(SimError::Diagnostic(diag));
                        }
                        self.osc_last_state_hash = Some(hv);
                    }
                }

                // Check pending $wait conditions
                if !self.pending_waits.is_empty()
                    && !deltas.is_empty()
                    && self.process_pending_waits(&deltas)?
                {
                    activity = true;
                }

                // Check pending blocking event control @(sig)
                if !self.pending_events.is_empty()
                    && !deltas.is_empty()
                    && self.process_pending_events(&deltas)?
                {
                    activity = true;
                }

                // Check pending blocking event control @(sig) jalur AST (UVM task)
                if !self.pending_ast_events.is_empty()
                    && !deltas.is_empty()
                    && self.process_pending_ast_events(&deltas)?
                {
                    activity = true;
                }

                // Check pending wait_order conditions
                if !self.pending_wait_orders.is_empty()
                    && !deltas.is_empty()
                    && self.process_pending_wait_orders(&deltas)?
                {
                    activity = true;
                }

                // Re-circulate if any events remain or NBA is pending
                // Postponed events do NOT re-circulate (they fire once per time step)
                self.ensure_events(t);
                let has_remaining = self.events[t - base].iter().any(|re| {
                    matches!(
                        re.region,
                        EventRegion::PreActive
                            | EventRegion::Active
                            | EventRegion::Inactive
                            | EventRegion::PreNba
                            | EventRegion::Nba
                            | EventRegion::PostNba
                            | EventRegion::PreObserved
                            | EventRegion::Observed
                            | EventRegion::PostObserved
                            | EventRegion::Reactive
                            | EventRegion::PostReactive
                    )
                }) || !self.nba_pending.is_empty();

                if has_remaining {
                    activity = true;
                }

                if !activity {
                    break;
                }

                // Race detection: reset writer tracking setiap delta baru
                self.signal_writers.clear();
                self.signal_write_types.clear();

                // PERF: Only refresh signal_snapshot when something actually needs it
                // for edge/level detection. Full snapshot = 146K LogicVec clones per delta.
                // For OpenTitan at time 0 with thousands of deltas, this was 5+ GB → OOM.
                let needs_snapshot = !self.pending_events.is_empty()
                    || !self.pending_waits.is_empty()
                    || !self.pending_ast_events.is_empty()
                    || !self.pending_wait_orders.is_empty();
                if needs_snapshot {
                    let num_sigs = self.state.signals.len();
                    let mut snap = Vec::with_capacity(num_sigs);
                    for i in 0..num_sigs {
                        snap.push(self.state.read_signal(i).clone());
                    }
                    self.signal_snapshot = Some(snap);
                }
            }

            // ── UPF: Evaluate power states based on current supply net values ──
            if let Some(ref mut pi) = self.power_intent {
                if pi.enabled {
                    // Auto-bind supply net values from design signals with matching names
                    for net_name in pi.supply_nets.keys() {
                        if let Some(sig_id) = self
                            .design
                            .top
                            .signals
                            .iter()
                            .position(|s| s.name.as_str() == *net_name)
                        {
                            let val = self.state.read_signal(sig_id);
                            let is_high = val.to_bool().unwrap_or(false);
                            pi.supply_values.insert(net_name.clone(), is_high);
                        }
                    }
                    pi.evaluate_power_states();
                }
            }

            // ── Delta oscillation detection ──
            // Check if any signal toggled excessively within delta cycles
            for (sig_id, count) in self.signal_write_count.iter() {
                if *count > 10 {
                    if let Some(sig) = self.design.top.signals.get(*sig_id) {
                        self.emit_warning(
                            DiagCode::CombinationalLoop,
                            format!(
                                "possible combinational loop: signal '{}' toggled {} times in delta cycles",
                                sig.name, count
                            ),
                        );
                    }
                }
            }
            self.signal_write_count.clear();

            // ── Coverage tracking: toggle + FSM — record from committed changes ──
            self.record_coverage_after_commit();

            // ── Postponed region: $strobe, $monitor, VCD, timing checks ──
            // Postponed region events from events[t] are processed in the region loop above.
            // Standalone postponed operations execute here, once per time step.
            self.process_strobe()?;
            self.dump_vcd_state()?;
            self.dump_fst_state()?;
            self.dump_csv_state()?;
            self.record_signal_stats();
            self.check_monitor()?;
            self.check_timing_constraints()?;
            // SIM-06/07/08/10: timing checks dari SDF TIMINGCHECK (di-parse
            // annotate_sdf, dievaluasi di sini — sebelumnya tidak pernah dipakai).
            self.check_sdf_timing_constraints()?;

            // ── SIM-18: auto-checkpoint (crash recovery) — simpan tiap
            // interval cycle ke file. File ditimpa (bukan append) sehingga
            // selalu merepresentasikan titik terakhir; resume memakai
            // --restore + --max-time lanjutan.
            if let Some((ref path, interval)) = self.auto_checkpoint.clone() {
                if interval > 0 && self.state.time.is_multiple_of(interval) {
                    let _ = self.save_checkpoint(std::path::Path::new(&path));
                }
            }

            // ── VPI: Read-Write Synch callback after all signal updates ──
            crate::vpi::callback::dispatch_read_write_synch();
            // ── VHPI (IEEE 1076-2008): time step + ReadWrite/ReadOnly synch
            // callback tiap time step — callback foreign masuk antrian
            // scheduler (bukan thread library), poin 5 arsitektur user.
            crate::vhpi::api::dispatch_time_step();
            crate::vhpi::api::dispatch_synch();

            // ── Process ForeignEvent queue (VPI/VHPI/PLI/DPI) ——
            // Events queued by dispatch_* functions are now processed in the
            // appropriate IEEE 1800 region.
            self.process_foreign_events()?;

            // ── Debug check at start of cycle ──
            if self.debug_mode != DebugMode::Normal {
                self.debug_check()?;
                if self.paused {
                    break;
                }
            }

            // ── Co-simulation: update shared state ──
            if let Some(ref cosim_state) = self.cosim_state {
                if let Ok(mut cs) = cosim_state.lock() {
                    cs.current_time = self.state.time;
                    // Copy output signal values (input signals of the external sim)
                    cs.outgoing_signals.clear();
                    for (sig_id, _, is_output) in &self.cosim_signals {
                        if *is_output && *sig_id < self.state.signals.len() {
                            let val = self.state.read_signal(*sig_id);
                            let bytes = val.to_u64().to_le_bytes().to_vec();
                            cs.outgoing_signals.push((*sig_id as u32, bytes));
                        }
                    }
                    // Poll incoming signals from external simulator
                    if cs.data_ready {
                        for (sig_id, val_bytes) in cs.incoming_signals.drain(..) {
                            if let Some(inner) = self
                                .cosim_signals
                                .iter()
                                .find(|(id, _, _)| *id as u32 == sig_id)
                            {
                                let sid = inner.0;
                                if sid < self.state.signals.len() {
                                    let width = self
                                        .design
                                        .top
                                        .signals
                                        .get(sid)
                                        .map(|s| s.width)
                                        .unwrap_or(1);
                                    let val = u64::from_le_bytes(
                                        val_bytes[..8.min(val_bytes.len())]
                                            .try_into()
                                            .unwrap_or([0u8; 8]),
                                    );
                                    let lv = LogicVec::from_u64(val, width);
                                    if *self.state.read_signal(sid) != lv {
                                        self.state.write_signal(sid, lv);
                                    }
                                }
                            }
                        }
                        cs.data_ready = false;
                    }
                }
            }

            // Advance and evaluate sequence attempts
            self.evaluate_sequence_attempts()?;
            // Reclaim slot event untuk time step yang baru selesai (anti-leak:
            // events tidak pernah tumbuh O(max_time)).
            self.retire_events(t);
            crate::dbg_sim!(
                1,
                "time {} -> {} selesai (deltas={}, events_step={})",
                t,
                self.state.time,
                delta_count,
                self.sim_perf.counters.events_processed - step_start_events
            );
            self.state.time += 1;
            // BUG FIX (step mode): pause SETELAH time increment — sebelumnya break
            // terjadi sebelum `state.time += 1` sehingga step_cycle() berulang
            // memproses time step yang SAMA dan waktu tidak pernah maju.
            if self.step_mode == StepMode::StepCycle {
                self.paused = true;
                break;
            }
            // Reset deteksi osilasi untuk time step berikutnya.
            self.osc_state_hashes.clear();
            self.osc_last_state_hash = None;

            // ── Progress report + deteksi stall ──
            let processed_this_step = self.sim_perf.counters.events_processed - step_start_events;
            if processed_this_step > 0 {
                last_active_wall = std::time::Instant::now();
                stall_warned = false;
            } else if !stall_warned && last_active_wall.elapsed() >= stall_wall_limit {
                eprintln!(
                    "[mivon] Warning: simulation appears stalled — no event processed \
                     for {:.0}s (time={}, delta={}).",
                    last_active_wall.elapsed().as_secs_f64(),
                    self.state.time,
                    self.current_delta
                );
                stall_warned = true;
            }
            if self.report_progress && self.state.time.is_multiple_of(progress_interval) {
                let elapsed = last_report_wall.elapsed().as_secs_f64().max(1e-9);
                let speed = progress_interval as f64 / elapsed / 1e6; // M steps/s
                eprintln!(
                    "[mivon] progress: time={} events={} processes={} speed={:.2} M steps/s",
                    self.state.time,
                    self.sim_perf.counters.events_processed,
                    self.design.top.processes.len(),
                    speed
                );
                last_report_wall = std::time::Instant::now();
            }

            // ── Quiescence detection: tidak ada event masa depan & tidak ada
            // pekerjaan tertunda → desain tanpa $finish/$stop berhenti
            // gracefully (bukan spin unlimited sampai di-kill). Event selalu
            // dijadwalkan pada waktu >= sekarang, jadi slot events kosong +
            // antrian region kosong = tidak ada yang bisa mengubah state lagi.
            // Gate cosim/foreign: thread eksternal bisa memasukkan event baru.
            if self.state.time.is_multiple_of(QUIESCENCE_CHECK_INTERVAL)
                && !self.use_timing_wheel
                && self.foreign_events.is_empty()
                && self.cosim_state.is_none()
                && self.pending_events.is_empty()
                && self.pending_ast_events.is_empty()
                && self.reactive_events.is_empty()
                && self.strobe_events.is_empty()
                && self.fstrobe_events.is_empty()
                && self.events.iter().all(|v| v.is_empty())
            {
                eprintln!(
                    "[mivon] Simulation quiesced at time {} — no pending events \
                     (design finished without explicit $finish/$stop).",
                    self.state.time
                );
                break;
            }

            if !self.sim_limit.allows(self.state.time) {
                break;
            }

            self.ensure_events(self.state.time as usize);
        }

        if !self.paused {
            self.execute_final_blocks()?;
            // ── VPI: End of simulation callback (setelah final blocks) ──
            crate::vpi::callback::dispatch_end_of_simulation();

            if !self.coverage_report_silent {
                self.report_full_coverage();
                self.report_coverage();
            }
            // VERIF-17/18/19: ringkasan transaction recording (bila ada).
            if !self.tr_records.is_empty() {
                eprintln!("\n=== Transaction Recording ===");
                eprint!("{}", self.report_tr_records());
            }
            self.check_post_simulation_warnings();
            self.report_severity_summary();
        }

        // ── Cleanup: deregister thread-local arena untuk cegah dangling pointer ──
        // (ArenaGuard di atas juga menanganinya saat early-return; panggilan
        // eksplisit ini tetap ada untuk path normal.)
        crate::simulator::arena::set_thread_arena(None);

        // ── VPI Cleanup ──
        crate::vpi::handle::clear_cstring_cache();
        crate::vpi::handle::vpi_clear_all_objects();
        crate::vpi::callback::clear_all_callbacks();
        crate::vpi::systf::clear_all_systfs();
        crate::vpi::clear_vpi_engine();
        // ── VHPI Cleanup (IEEE 1076-2008) ──
        crate::vhpi::api::dispatch_end_of_simulation();
        crate::vhpi::loader::vhpi_cleanup();
        // ── PLI Cleanup (IEEE 1364 PLI 1.0/2.0) ──
        crate::pli::loader::pli_cleanup();

        Ok(())
    }

    /// Ringkasan severity system task di akhir sim (F15). Hanya dicetak bila
    /// ada warning/error/fatal — sim bersih tidak berisik.
    fn report_severity_summary(&self) {
        if self.sev_warning_count == 0 && self.sev_error_count == 0 && self.sev_fatal_count == 0 {
            return;
        }
        eprintln!("\n── Severity Summary ──");
        if self.sev_fatal_count > 0 {
            eprintln!("  fatals:  {}", self.sev_fatal_count);
        }
        if self.sev_error_count > 0 {
            eprintln!("  errors:  {}", self.sev_error_count);
        }
        if self.sev_warning_count > 0 {
            eprintln!("  warnings: {}", self.sev_warning_count);
        }
        if self.sev_info_count > 0 {
            eprintln!("  info:    {}", self.sev_info_count);
        }
    }

    /// F27: SignalId dari ClockEdge — varian *Hier (clock lewat port interface,
    /// path `b.clk`) di-resolve via hier_signal_map (design sudah di-flatten).
    pub(crate) fn clock_edge_signal(&self, edge: &ClockEdge) -> Option<SignalId> {
        match edge {
            ClockEdge::PosEdge(id) | ClockEdge::NegEdge(id) => Some(*id),
            ClockEdge::PosEdgeHier(s) | ClockEdge::NegEdgeHier(s) => {
                self.design.hier_signal_map.get(s).copied()
            }
        }
    }

    /// F27: normalize sigs EventControl — varian ClockEdge::*Hier(Symbol)
    /// (event `@(posedge b.clk)`) di-resolve ke SignalId nyata via
    /// hier_signal_map saat arm. Entry yang tak ter-resolve dibuang (perilaku
    /// lama: signal tak dikenal di @(...) di-skip).
    pub(crate) fn normalize_event_sigs(
        &self,
        sigs: &[(SignalId, Option<ClockEdge>)],
    ) -> Vec<(SignalId, Option<ClockEdge>)> {
        sigs.iter()
            .filter_map(|(sid, edge)| {
                let real = match edge {
                    Some(ClockEdge::PosEdgeHier(s)) | Some(ClockEdge::NegEdgeHier(s)) => {
                        self.design.hier_signal_map.get(s).copied()
                    }
                    _ => Some(*sid),
                }?;
                let norm_edge = edge.as_ref().map(|e| match e {
                    ClockEdge::PosEdgeHier(_) => ClockEdge::PosEdge(real),
                    ClockEdge::NegEdgeHier(_) => ClockEdge::NegEdge(real),
                    other => other.clone(),
                });
                Some((real, norm_edge))
            })
            .collect()
    }

    /// Check post-simulation warnings: uninitialized registers, unused signals,
    /// clock never toggles, reset permanently asserted.
    fn check_post_simulation_warnings(&self) {
        // Collect clock signal IDs from Sequential processes
        let clock_sigs: HashSet<SignalId> = self
            .design
            .top
            .processes
            .iter()
            .filter_map(|p| {
                if let Process::Sequential { clock, .. } = p {
                    match clock {
                        ClockEdge::PosEdge(id) | ClockEdge::NegEdge(id) => Some(*id),
                        // F27: clock lewat port interface — resolve hier path.
                        ClockEdge::PosEdgeHier(s) | ClockEdge::NegEdgeHier(s) => {
                            self.design.hier_signal_map.get(s).copied()
                        }
                    }
                } else {
                    None
                }
            })
            .collect();

        // Collect reset signal IDs
        let reset_sigs: HashSet<SignalId> = self
            .design
            .top
            .processes
            .iter()
            .filter_map(|p| {
                if let Process::Sequential { reset: Some(r), .. } = p {
                    Some(r.signal)
                } else {
                    None
                }
            })
            .collect();

        for (sig_id, sig) in self.design.top.signals.iter().enumerate() {
            let sig_name = sig.name.as_str();
            let last_change = self.signal_last_change.get(&sig_id);
            let never_changed = last_change.is_none();

            // Skip input-only signals (driven by testbench, not by design)
            if sig.kind == SignalKind::Input || sig.kind == SignalKind::Inout {
                continue;
            }

            // ── WR0014: Uninitialized Register ──
            // Signal is all-X at init and was never written to
            if never_changed
                && (sig.kind == SignalKind::Reg || sig.kind == SignalKind::Logic)
                && sig.init_val.all_x()
            {
                self.emit_warning(
                    DiagCode::UninitializedRegister,
                    format!("uninitialized register '{}' was never assigned", sig_name),
                );
            }

            // ── WR0104: Unused Signal ──
            // Signal has a defined init value but never changed during simulation
            if never_changed && !sig.init_val.all_x() && !sig.init_val.all_z() {
                self.emit_warning(
                    DiagCode::UnusedSignal,
                    format!(
                        "signal '{}' never changed during simulation (t=0..{}) — \
                         constant value or unused",
                        sig_name, self.state.time
                    ),
                );
            }

            // ── WR0202: Clock Never Toggles ──
            // Signal is a clock but never toggled (only written at init time or never)
            // We check if the signal was ever written AFTER time 0 (init = time 0)
            if clock_sigs.contains(&sig_id) {
                let last_change_time = self.signal_last_change.get(&sig_id).copied();
                let has_real_change = last_change_time.is_some_and(|t| t > 0);
                if !has_real_change {
                    self.emit_warning(
                        DiagCode::ClockNeverToggles,
                        format!(
                            "clock signal '{}' never toggled during simulation (t=0..{}) — \
                             clock module unreachable, or run ended before the first edge",
                            sig_name, self.state.time
                        ),
                    );
                }
            }

            // ── WR0203: Reset Permanently Asserted ──
            // Reset signal was never de-asserted (only written at init time or never)
            if reset_sigs.contains(&sig_id) {
                let last_change_time = self.signal_last_change.get(&sig_id).copied();
                let has_real_change = last_change_time.is_some_and(|t| t > 0);
                if !has_real_change {
                    let val = self.state.read_signal(sig_id);
                    if !val.all_x() && !val.all_z() {
                        self.emit_warning(
                            DiagCode::ResetPermanentlyAsserted,
                            format!(
                                "reset signal '{}' never changed during simulation (t=0..{}) — \
                                 may be stuck asserted; verify drive and polarity (rst vs rst_n)",
                                sig_name, self.state.time
                            ),
                        );
                    }
                }
            }
        }
    }

    /// TRUE bila design memuat sinyal ultra-lebar (array memori flatten —
    /// cache 65536x32x512 ≈ 1.07e9 bit). Evaluasi PARALEL menyalin snapshot
    /// sinyal per layer → clone raksasa → OOM; dipakai penggerbang jalur
    /// sequential. Iterasi top.signals ringan (ratusan) — tanpa cache.
    pub fn has_wide_signals(&self) -> bool {
        self.design
            .top
            .signals
            .iter()
            .any(|s| s.width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD)
    }

    /// Evaluate EvalProcess events in parallel using DAG layers.
    ///
    /// Processes in the same DAG layer are independent (no signal conflicts)
    /// and are evaluated via rayon work-stealing. Writes are collected and
    /// applied after all processes in the layer complete.
    ///
    /// # Lock-Free Design
    ///
    /// Setiap process bekerja pada snapshot sinyal sendiri (clone).
    /// Tidak ada shared mutable state antar process dalam satu layer.
    fn evaluate_eval_processes_parallel(&mut self, pids: &[usize]) -> Result<(), SimError> {
        // Design dengan sinyal ultra-lebar (array memori flatten, cache
        // 65536x32x512 = 1.07e9 bit): paralel-eval menyalin snapshot SEMUA
        // sinyal per layer → clone 1e9 bit → OOM mesin kecil. Evaluasi
        // sequential membaca slot langsung (tanpa clone penuh).
        if self.has_wide_signals() {
            for &pid in pids {
                self.process_event(EventKind::EvalProcess(pid), self.current_time as usize)?;
            }
            return Ok(());
        }
        // Gunakan cached process bodies + DAG layers + signal snapshot
        // untuk menghindari clone bodies setiap cycle.
        // process_body_cache dibangun sekali di run() — tidak ada clone per cycle.
        let dag_layers: Vec<Vec<usize>> = match &self.sim_dag {
            Some(dag) => dag.layers().to_vec(),
            None => {
                for &pid in pids {
                    self.process_event(EventKind::EvalProcess(pid), self.current_time as usize)?;
                }
                return Ok(());
            }
        };
        // Evaluate each layer sequentially (processes WITHIN a layer are parallel)
        // Pass process_body_cache langsung — zero clone per cycle
        let body_cache = &self.process_body_cache;
        for layer in &dag_layers {
            let layer_pids: Vec<&usize> = layer.iter().filter(|pid| pids.contains(pid)).collect();
            if layer_pids.is_empty() {
                continue;
            }

            // Snapshot FRESH per layer: layer WAW-dependency terpisah (bit/range
            // berbeda sinyal sama) harus lihat hasil layer SEBELUMNYA — snapshot
            // sekali sebelum loop = basi → setiap proses baca X di bit lain →
            // apply berurutan menimpa → cuma bit terakhir hidup (generate
            // `b[i] = ~a[i]` DAG=128 utk 8 bit, ditemukan fuzzer).
            // Baca nilai PENDING (next_signals bila changed) — write_signal
            // antar layer menulis ke delta-pending, state.signals belum commit.
            let signal_snapshot: Vec<Arc<LogicVec>> = (0..self.state.signals.len())
                .map(|i| {
                    // Sinyal besar belum ditulis → lazy (tanpa materialisasi).
                    Arc::new(self.state.snapshot_signal(i))
                })
                .collect();

            // Evaluate all processes in this layer in parallel via rayon
            // Each worker gets its own signal clone + body reference
            let writes = crate::scheduler::sim_dag::evaluate_bodies_parallel(
                &layer_pids,
                body_cache,
                &signal_snapshot,
                &self.design.top.signals,
            )?;

            // Apply writes back to state (no borrow conflicts, all data cloned)
            for (sig_id, val) in writes {
                if sig_id < self.state.signals.len() {
                    self.state.write_signal(sig_id, val);
                }
            }
        }

        Ok(())
    }

    pub(crate) fn evaluate_clock_domain(&mut self, domain: &ClockDomain) -> Result<(), SimError> {
        // Clone process bodies upfront untuk hindari borrow conflicts
        let num_sigs = self.state.signals.len();

        // Collect sequential process bodies
        let seq_bodies: Vec<(usize, Vec<IrStmt>)> = domain
            .sequential_processes
            .iter()
            .filter_map(|&pid| {
                if pid < self.design.top.processes.len() {
                    if let Process::Sequential { body, .. } = self
                        .design
                        .top
                        .processes
                        .get(pid)
                        .expect("process pid out of bounds in clock domain eval")
                    {
                        return Some((pid, body.clone()));
                    }
                }
                None
            })
            .collect();

        // Collect follower combinational process bodies
        let follower_bodies: Vec<(usize, Vec<IrStmt>)> = domain
            .follower_processes
            .iter()
            .filter_map(|&pid| {
                if pid < self.design.top.processes.len() {
                    match self
                        .design
                        .top
                        .processes
                        .get(pid)
                        .expect("process pid out of bounds in follower bodies")
                    {
                        Process::Combinational { body, .. }
                        | Process::CombReactive { body, .. } => Some((pid, body.clone())),
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .collect();

        // Fixed-point iteration: evaluate sequential + follower until stable
        let max_iter = 10; // Safety limit
        for _iter in 0..max_iter {
            let prev_snapshot: Vec<LogicVec> = (0..num_sigs)
                .map(|i| self.state.read_signal(i).clone())
                .collect();

            // Step 1: Evaluate all sequential processes in the domain
            for (_, body) in &seq_bodies {
                self.evaluate_stmt_block(body)?;
            }

            // Commit NBA from sequential process evaluations
            self.commit_nba();

            // Step 2: Evaluate all follower combinational processes
            for (_, body) in &follower_bodies {
                self.evaluate_stmt_block(body)?;
            }

            // Check if stable: no signal changes
            let mut changed = false;
            for (i, prev) in prev_snapshot
                .iter()
                .enumerate()
                .take(num_sigs.min(prev_snapshot.len()))
            {
                let cur = self.state.read_signal(i);
                if cur != prev {
                    changed = true;
                    break;
                }
            }

            if !changed {
                break; // Converged
            }
        }

        Ok(())
    }

    fn initialize_time_zero(&mut self) -> Result<(), SimError> {
        // BUG FIX (co-sim/debugger): `run()` bisa dipanggil berulang (pause +
        // continue / step). Inisialisasi time-0 HANYA sekali — re-run setelah
        // state.time > 0 akan (a) meng-schedule ulang event time-0 yang basi,
        // dan (b) setelah kompaksi events (EVENT_COMPACT_THRESHOLD) `push_event(0)`
        // memakai `idx = 0 - events_base` → underflow usize → panic. Guard ini
        // membuat repeated run() melanjutkan dari state saat ini.
        if self.state.time != 0 {
            return Ok(());
        }
        let t = 0usize;
        let processes = self.design.top.processes.clone();

        // IEEE 1800: declaration assignments (decl-init) happen at time 0
        // BEFORE any initial/always block runs.

        // Pass 0: declaration initializers (wire a = 1; reg b = 0; etc.)
        for (pid, process) in processes.iter().enumerate() {
            if let Process::Initial { name, .. } = process {
                if name.as_str().starts_with("decl_init_") {
                    self.push_event(
                        t,
                        RegionEvent {
                            region: EventRegion::Active,
                            event: EventKind::EvalProcess(pid),
                        },
                    );
                }
            }
        }

        // Pass 1: Initial blocks (execute after declaration assignments)
        for (pid, process) in processes.iter().enumerate() {
            if matches!(process, Process::Initial { .. }) {
                if let Process::Initial { name, .. } = process {
                    if name.as_str().starts_with("decl_init_") {
                        continue;
                    }
                }
                self.push_event(
                    t,
                    RegionEvent {
                        region: EventRegion::Active,
                        event: EventKind::EvalProcess(pid),
                    },
                );
            }
        }

        // Pass 2: Combinational/Reactive processes (evaluate after initial)
        for (pid, process) in processes.iter().enumerate() {
            if matches!(
                process,
                Process::Combinational { .. } | Process::CombReactive { .. }
            ) {
                self.push_event(
                    t,
                    RegionEvent {
                        region: EventRegion::Active,
                        event: EventKind::EvalProcess(pid),
                    },
                );
            }
        }

        // Pass 3: AlwaysWithDelay (time-0 processes that schedule future events)
        for (pid, process) in processes.iter().enumerate() {
            if matches!(process, Process::AlwaysWithDelay { .. }) {
                self.push_event(
                    t,
                    RegionEvent {
                        region: EventRegion::Active,
                        event: EventKind::EvalProcess(pid),
                    },
                );
            }
        }

        // Sequential processes wait for edge events, not scheduled at time 0
        // Final processes execute only at $finish

        // Initialize coverage tracking
        // Initialize coverage tracking
        for cg in &self.design.covergroups {
            for cp in &cg.coverpoints {
                let key = format!("{}.{}", cg.name, cp.name);
                self.cover_total.insert(Symbol::intern(&key), 0);
                self.cover_hits.insert(Symbol::intern(&key), 0);
                self.cover_bins.insert(Symbol::intern(&key), HashMap::new());
            }
            for cross in &cg.crosses {
                let key = format!("{}.{}", cg.name, cross.name);
                self.cover_total.insert(Symbol::intern(&key), 0);
                self.cover_hits.insert(Symbol::intern(&key), 0);
                self.cover_bins.insert(Symbol::intern(&key), HashMap::new());
            }
        }
        Ok(())
    }

    pub fn annotate_sdf(&mut self, sdf: &SdfData) -> Result<(), SimError> {
        let mode = crate::simulator::sdf::get_timing_mode();

        // Apply cell delays to signals
        for (cell_name, cell_delay) in &sdf.cell_delays {
            // Find matching signals by cell name (partial match via hierarchy)
            // Take the first IOPATH delay as the primary cell delay
            if let Some(path_delay) = cell_delay.io_paths.values().next() {
                let rise_ns = path_delay.rise.get(mode);
                let fall_ns = path_delay.fall.get(mode);

                // Try to find signals matching this cell instance name
                for sig in &mut self.design.top.signals {
                    let sig_name = sig.name.as_str();
                    if sig_name.starts_with(cell_name) || cell_name.starts_with(sig_name) {
                        sig.delay_rise = Some((rise_ns * 1000.0) as u64); // convert ns to ps
                        sig.delay_fall = Some((fall_ns * 1000.0) as u64);
                    }
                }

                // Also store delays in the signal delay map for per-path lookup
                for (path_key, path_delay) in &cell_delay.io_paths {
                    let rise_ns = path_delay.rise.get(mode);
                    let fall_ns = path_delay.fall.get(mode);
                    let delay_key = format!("{}:{}", cell_name, path_key);
                    self.signal_delays.insert(
                        delay_key,
                        crate::simulator::state::SignalDelay {
                            rise: (rise_ns * 1000.0) as u64,
                            fall: (fall_ns * 1000.0) as u64,
                        },
                    );
                }
            }
        }

        // Apply net delays to signals
        for (net_name, net_delay) in &sdf.net_delays {
            let rise_ns = net_delay.rise.get(mode);
            let fall_ns = net_delay.fall.get(mode);
            for sig in &mut self.design.top.signals {
                if sig.name == *net_name || sig.name.ends_with(&format!(".{}", net_name)) {
                    sig.delay_rise = Some((rise_ns * 1000.0) as u64);
                    sig.delay_fall = Some((fall_ns * 1000.0) as u64);
                }
            }
        }

        // Store timing checks for later use
        self.sdf_timing_checks = sdf.timing_checks.clone();

        // Pulse control (SIM-09): resolve nama signal ke id engine. SDF memakai
        // hierarchical names — suffix match setelah titik terakhir (pola yang
        // sama dengan timing check resolve).
        for (name, pc) in &sdf.pulse_controls {
            let resolved = self.design.top.signals.iter().enumerate().find(|(_, s)| {
                s.name.as_str() == name || s.name.as_str().ends_with(&format!(".{}", name))
            });
            if let Some((id, _)) = resolved {
                self.sdf_pulse_controls.insert(id, pc.width.get(mode));
            }
        }

        // Print summary of annotation
        let cell_count = sdf.cell_delays.len();
        let net_count = sdf.net_delays.len();
        let check_count = sdf.timing_checks.len();
        eprintln!(
            "SDF annotation: {} cells, {} nets, {} timing checks (mode={})",
            cell_count,
            net_count,
            check_count,
            mode.as_str()
        );

        Ok(())
    }

    fn execute_final_blocks(&mut self) -> Result<(), SimError> {
        // `final` block harus tetap dieksekusi setelah $finish/$fatal (LRM:
        // final blocks jalan di akhir simulasi). `$fatal` sudah meng-set
        // fatal_hit yang menghentikan blok statement biasa — reset di sini
        // agar final block tidak ikut terblokir (lihat arm $fatal).
        self.fatal_hit = false;
        let bodies: Vec<Vec<IrStmt>> = self
            .design
            .top
            .processes
            .iter()
            .filter_map(|p| {
                if let Process::Final { body, .. } = p {
                    Some(body.clone())
                } else {
                    None
                }
            })
            .collect();
        for body in &bodies {
            self.evaluate_stmt_block(body)?;
        }
        Ok(())
    }

    /// Process queued foreign events (VPI/VHPI/PLI/DPI) in the appropriate
    /// IEEE 1800 region. Events are dispatched by the adapter layer and
    /// queued via `queue_foreign_event`. This method processes them all.
    /// Note: ValueChange events are fired directly from signal update path
    /// (see signal update loop) — this processes the other event types.
    fn process_foreign_events(&mut self) -> Result<(), SimError> {
        use crate::foreign::ForeignEvent;

        while let Some(event) = self.foreign_events.pop() {
            match event {
                ForeignEvent::ValueChange { object: _ } => {
                    // Value-change callbacks are fired directly from signal update
                    // path (lines 1101-1124). This event is a no-op here.
                }
                ForeignEvent::ReadWriteSync => {
                    // Read-Write Synch: VPI cbReadWriteSynch + VHPI vhpiCbReadWriteSynch
                    crate::vpi::with_vpi_engine(|_engine| {
                        crate::vpi::callback::fire_value_change_callbacks(
                            "",
                            &crate::vpi::types::t_vpi_value::default(),
                            &crate::vpi::types::t_vpi_value::default(),
                        );
                    });
                    crate::vhpi::object::with_vhpi_engine(|_engine| {
                        crate::vhpi::callback::dispatch_callback(
                            crate::vhpi::callback::vhpiCbReadWriteSynch,
                        );
                    });
                }
                ForeignEvent::ReadOnlySync => {
                    // Read-Only Synch: VPI cbReadOnlySynch + VHPI vhpiCbReadOnlySynch
                    crate::vpi::with_vpi_engine(|_engine| {
                        crate::vpi::callback::fire_value_change_callbacks(
                            "",
                            &crate::vpi::types::t_vpi_value::default(),
                            &crate::vpi::types::t_vpi_value::default(),
                        );
                    });
                    crate::vhpi::object::with_vhpi_engine(|_engine| {
                        crate::vhpi::callback::dispatch_callback(
                            crate::vhpi::callback::vhpiCbReadOnlySynch,
                        );
                    });
                }
                ForeignEvent::NextTimeStep => {
                    // Next Time Step: VHPI vhpiCbNextTimeStep
                    crate::vhpi::object::with_vhpi_engine(|_engine| {
                        crate::vhpi::callback::dispatch_callback(
                            crate::vhpi::callback::vhpiCbNextTimeStep,
                        );
                    });
                }
                ForeignEvent::Callback { callback_id: _ } => {
                    // Registered callback (after-delay). Not yet implemented.
                }
                ForeignEvent::EndOfSimulation => {
                    // End of simulation: VPI cbEndOfSimulation + VHPI vhpiCbEndOfSimulation
                    crate::vpi::with_vpi_engine(|_engine| {
                        crate::vpi::callback::fire_value_change_callbacks(
                            "",
                            &crate::vpi::types::t_vpi_value::default(),
                            &crate::vpi::types::t_vpi_value::default(),
                        );
                    });
                    crate::vhpi::object::with_vhpi_engine(|_engine| {
                        crate::vhpi::callback::dispatch_callback(
                            crate::vhpi::callback::vhpiCbEndOfSimulation,
                        );
                    });
                }
            }
        }
        Ok(())
    }
}
