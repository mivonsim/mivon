pub mod core;
pub mod coverage;
pub mod cycle_based;
pub mod debug;
pub mod engine_utils;
pub mod eval;
pub mod mir;
pub mod scheduler;
pub mod sequence;
pub mod uvm;
pub mod waveform;

use crate::foreign::ForeignEvent;
use crate::scheduler::clock_domain::ClockDomainAnalysis;
use crate::scheduler::SimulationDag;
use crate::simulator::arena::SimulationArena;
use crate::simulator::parallel::ParallelConfig;
use mivon_core::diagnostics::DiagSink;
use mivon_ir::*;

use crate::simulator::sdf::TimingCheck;
use crate::simulator::state::SimulationState;
use crate::simulator::types::*;
use crate::waveform::{CsvWaveWriter, FstWaveWriter, SignalStats, VcdWriter};
use mivon_core::Symbol;
use rand::rngs::StdRng;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;

/// Maks iterasi loop runtime tanpa delay sebelum dihentikan (anti-hang).
/// Loop testbench yang sah hampir selalu < 100k iterasi; loop tanpa delay
/// yang lebih besar hampir pasti tak disengaja (infinite loop).
pub(crate) const MAX_LOOP_ITER: usize = 100_000;

/// Batas jangkauan slot event dari `events_base`. Event dengan delay lebih
/// jauh dari ini (mis. `#9000000000000000000`) tidak dialokasikan — run loop
/// abort dengan diagnostic alih-alih resize miliaran slot (OOM/panic).
/// 10M slot ≈ 240MB worst-case; desain nyata menengah jauh di bawah ini.
pub(crate) const MAX_EVENT_SPAN: usize = 10_000_000;

/// Jumlah slot time-step yang dipertahankan di `events` sebelum leading
/// retired slots di-drain (lihat `SimulationEngine::retire_events`).
pub(crate) const EVENT_COMPACT_THRESHOLD: usize = 65_536;

/// VERIF-32: statistik sequence coverage per assertion sequence (keyed by
/// line:col) — berapa kali attempt dimulai, berapa yang match, dan berapa
/// yang fail (timeout tanpa match). Sequence dengan matched == 0 adalah
/// coverage hole (laporan coverage_gaps).
#[derive(Debug, Clone, Copy, Default)]
pub struct SeqCovStats {
    pub attempts: u64,
    pub matched: u64,
    pub failed: u64,
}

/// Tracks a single attempt of a concurrent assertion sequence evaluation
pub struct SequenceAttempt {
    pub sequence: Box<IrSequence>,
    pub cycles: u64,
    pub pass_stmt: Vec<IrStmt>,
    pub fail_stmt: Vec<IrStmt>,
    pub clock_event: mivon_ast::types::ClockEvent,
    /// VERIF-27: posisi source assertion (utk assertion coverage stats).
    pub line: usize,
    pub col: usize,
    /// LANG-04: track whether antecedent matched at cycles=0 for Implication.
    /// None = not yet checked (first cycle), Some(true/false) = checked.
    pub ante_matched: Option<bool>,
}

/// Batas simulasi.
///
/// - `Unlimited` (default untuk CLI): simulasi berjalan sampai `$finish` /
///   `$fatal` / assertion fatal / error internal. Paling mendekati
///   simulator industri — testbench lah yang memutuskan kapan selesai.
/// - `Finite(n)`: simulasi berhenti saat `state.time > n` (mirip `-T`).
///
/// Tidak ada konstanta "ajaib" — pengguna yang membatasi waktu memakai
/// `--max-time <n>`, sisanya unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationLimit {
    Unlimited,
    Finite(u64),
}

impl SimulationLimit {
    /// true bila waktu `t` masih dalam batas (dipanggil sebagai guard loop).
    pub fn allows(&self, t: u64) -> bool {
        match self {
            SimulationLimit::Unlimited => true,
            SimulationLimit::Finite(m) => t <= *m,
        }
    }

    /// Batas numerik (u64::MAX untuk Unlimited) — dipakai API legacy
    /// (debugger, distributed) yang masih bertipe u64.
    pub fn bound(&self) -> u64 {
        match self {
            SimulationLimit::Unlimited => u64::MAX,
            SimulationLimit::Finite(m) => *m,
        }
    }

    /// Representasi display ("unlimited" atau angka).
    pub fn display(&self) -> String {
        match self {
            SimulationLimit::Unlimited => "unlimited".to_string(),
            SimulationLimit::Finite(m) => m.to_string(),
        }
    }
}

/// Main simulation engine — event-driven SystemVerilog simulator.
pub struct SimulationEngine {
    pub design: IrDesign,
    pub state: SimulationState,
    pub sim_limit: SimulationLimit,
    /// PERF-16: cache lookup nama sinyal flatten → SignalId. Dibangun lazy
    /// SEKALI (top.signals + hier_signal_map statis setelah elaborasi) dan
    /// menggantikan scan linear O(S) per akses hierarkis di `find_signal`
    /// (hot path runtime — O(N·S) per cycle = O(N²) pada desain besar yang
    /// mengakses sinyal hierarkis, mis. `bus.arr[k]`, per cycle).
    pub signal_lookup: std::sync::OnceLock<std::collections::HashMap<Symbol, usize>>,
    /// Aktifkan laporan progres berkala (tiap 1M tick) + deteksi stall ke
    /// stderr. Di-set true oleh CLI (default false agar output library/test
    /// tetap bersih).
    pub report_progress: bool,
    pub running: bool,
    /// `$fatal` telah dipanggil — menghentikan eksekusi blok statement yang
    /// sedang berjalan SEKETIKA (beda dari `running=false` yang dipakai
    /// `$finish`, karena `final` block tetap harus dieksekusi setelah
    /// `$finish`/`$fatal`).
    pub fatal_hit: bool,
    /// Penghitung severity system task ($info/$warning/$error/$fatal, F14) —
    /// dipakai ringkasan akhir sim (`report_severity_summary`) & exit code
    /// CLI non-zero untuk `$fatal` (F15).
    pub sev_info_count: u64,
    pub sev_warning_count: u64,
    pub sev_error_count: u64,
    pub sev_fatal_count: u64,
    /// Flag pembatalan eksternal (GUI "Stop"). Diperiksa setiap time step di
    /// run loop — jika bernilai true, simulasi berhenti lebih awal.
    pub cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Event queue per time step. Di-index RELATIF terhadap `events_base`:
    /// slot `i` menyimpan event untuk waktu `events_base + i`. Slot yang sudah
    /// diproses dibuang secara periodik (lihat `retire_events`) sehingga
    /// `events` tetap bounded terhadap `max_time` (anti-leak O(max_time)).
    pub events: Vec<Vec<RegionEvent>>,
    /// Waktu absolut yang direpresentasikan oleh `events[0]`.
    pub events_base: usize,
    pub nba_pending: Vec<(IrLValue, LogicVec)>,
    /// PERF-15: Bucket NBA writes by signal ID for O(1) conflict detection.
    pub nba_signal_map: HashMap<SignalId, usize>,
    /// SIM-18: auto-checkpoint untuk crash recovery — (path, interval cycle).
    /// Checkpoint disimpan otomatis tiap `interval` cycle (dan di akhir run)
    /// sehingga run yang crash/terhenti bisa di-resume dari titik terakhir.
    pub auto_checkpoint: Option<(String, u64)>,
    pub vcd: Option<VcdWriter>,
    pub fst: Option<FstWaveWriter>,
    /// CSV waveform writer (signal values as comma-separated values)
    pub csv: Option<CsvWaveWriter>,
    /// Signal statistics collector (toggle counts, transitions)
    pub signal_stats: Option<SignalStats>,
    pub current_this: Option<ObjId>,
    pub method_locals: Vec<HashMap<Symbol, LogicVec>>,
    /// Nilai persisten variabel `static` lokal function/task — key
    /// (function_name, var_name). Inisialisasi SEKALI saat call pertama,
    /// lalu dipertahankan antar pemanggilan (IEEE 1800 §6.21).
    pub static_locals: HashMap<(Symbol, Symbol), LogicVec>,
    pub current_method: Option<Symbol>,
    pub rng: StdRng,
    /// Jumlah panggilan fungsi random (untuk $get_randcount).
    pub rand_call_count: u64,
    /// Seed RNG terakhir (untuk $get_randstate).
    pub rand_seed: u64,
    /// Scope aktif untuk $scope / $showscopes.
    pub current_scope_name: Option<String>,
    pub file_handles: HashMap<u32, File>,
    pub file_ungetc_buf: HashMap<u32, Vec<u8>>,
    pub file_read_pos: HashMap<u32, u64>,
    pub next_file_handle: u32,
    pub monitor_args: Option<Vec<IrExpr>>,
    pub monitor_last_values: Option<Vec<LogicVec>>,
    pub disable_pending: Option<Symbol>,
    pub control_flow: Option<FlowControl>,
    pub expr_recursion_depth: usize,
    pub forced_signals: HashSet<SignalId>,
    /// Simpan nilai sinyal SEBELUM force — dikembalikan saat release
    /// (LRM §10.6.2: wire kembali ke driver setelah release).
    pub pre_force_values: HashMap<SignalId, LogicVec>,
    pub signal_snapshot: Option<Vec<LogicVec>>,
    /// Snapshot Preponed (pre-NBA) di awal time step. TIDAK di-refresh per
    /// delta — dipakai oleh evaluate_sequence_attempts untuk posedge detection.
    /// Tanpa ini, signal_snapshot (di-refresh ke post-NBA) menyebabkan posedge
    /// tidak terdeteksi oleh sequence evaluator → attempt timeout palsu.
    pub preponed_snapshot: Option<Vec<LogicVec>>,
    /// Riwayat snapshot sinyal (pre-NBA) untuk evaluator sequence temporal.
    /// Index 0 = current posedge, index 1 = 1 posedge lalu, dst.
    /// Max kedalaman 8 (cukup untuk ##8). Dipopulate di setiap posedge.
    pub signal_seq_history: VecDeque<Vec<LogicVec>>,
    /// Snapshot coverage di awal time step (SEBELUM loop delta). Tidak di-refresh
    /// per delta — dipakai record_coverage_after_commit untuk diff toggle/FSM
    /// (fix SIM-30: signal_snapshot di-refresh tiap delta untuk edge detection,
    /// membuat diff coverage selalu kosong).
    pub coverage_snapshot: Option<Vec<LogicVec>>,
    pub pending_waits: Vec<(Vec<SignalId>, Vec<IrStmt>)>,
    /// Blocking event control `@(sig)` yang menunggu perubahan/edge signal.
    /// Diperiksa setiap delta saat signal berubah (setara `pending_waits`).
    pub pending_events: Vec<PendingEventControl>,
    /// Versi AST untuk class method/task UVM (`run_phase` dkk), dengan konteks.
    pub pending_ast_events: Vec<PendingAstEventControl>,
    pub pending_await_target: Option<ObjId>,
    pub pending_wait_orders: Vec<WaitOrderState>,
    pub loop_continuation: Option<Vec<IrStmt>>,
    /// F18: kontinuasi loop untuk jalur AST (task/method UVM) — set saat
    /// body loop suspend (delay / get_next_item blocking) agar loop MENGULANG
    /// saat di-resume. Terpisah dari `loop_continuation` (yang bertipe IrStmt).
    pub ast_loop_continuation: Option<Vec<mivon_ast::Stmt>>,
    /// F26: fork id yang sedang mengeksekusi branch (IR/AST) — diteruskan ke
    /// task body (execute_method_body) agar continuation resume decrement fork
    /// yang benar. Tanpa ini task di dalam fork (module initial) suspend dgn
    /// fork_id None → resume tak pernah decrement → join selesai premature.
    pub active_fork_id: Option<usize>,
    /// F26: task method baru saja suspend (evaluate_ast_block_with_delay_fork
    /// return false) — branch fork yang memanggil task TIDAK boleh decrement di
    /// titik ini; resume (ContinueAstBlock dgn fork_id) yang decrement.
    pub task_suspended: bool,
    pub post_loop_tail: Vec<IrStmt>,
    pub current_time: u64,
    pub fork_groups: Vec<ForkGroup>,
    /// Slot `fork_groups` yang sudah selesai & aman di-reuse (anti-leak:
    /// fork di dalam loop tidak menumpuk entry selamanya).
    pub fork_free: Vec<usize>,
    /// LANG-29: situs `wait fork;` yang sedang menunggu group fork milik proses
    /// ini selesai. Dipangkas di `fork_finish`; kontinuasi dieksekusi saat
    /// semua fid yang ditunggu sudah selesai.
    pub pending_wait_forks: Vec<WaitForkState>,
    pub reactive_events: Vec<EventKind>,
    pub strobe_events: Vec<Vec<IrExpr>>,
    pub fstrobe_events: Vec<(u32, Vec<IrExpr>)>,
    pub fmonitor_map: HashMap<u32, (Vec<IrExpr>, Vec<LogicVec>)>,
    pub mailbox_queues: HashMap<usize, VecDeque<LogicVec>>,
    /// LANG-24: batas kapasitas per mailbox (bounded mode). Absen/0 = unbounded.
    pub mailbox_bounds: HashMap<usize, usize>,
    pub semaphore_counts: HashMap<usize, u32>,
    /// LANG-33: mode per constraint block — `(obj_id, block_name)` → enabled.
    /// Absen = enabled (default). Dipakai solver utk me-skip block nonaktif
    /// (IEEE 1800-2017 §18.5.12 `constraint_mode()`).
    pub constraint_modes: HashMap<(ObjId, Symbol), bool>,
    /// LANG-32: mode constraint block STATIC — key (class_name, block_name),
    /// berlaku global untuk SEMUA instance class (IEEE 1800-2017 §18.5.10:
    /// `constraint_mode()` pada static constraint mempengaruhi semua instance).
    pub static_constraint_modes: HashMap<(Symbol, Symbol), bool>,
    pub assoc_data: HashMap<usize, HashMap<LogicVec, LogicVec>>,
    pub uvm_object_data: HashMap<ObjId, UvmObjectData>,
    pub uvm_component_data: HashMap<ObjId, UvmComponentData>,
    pub uvm_sequencer_data: HashMap<ObjId, UvmSequencerData>,
    pub uvm_driver_data: HashMap<ObjId, UvmDriverData>,
    pub uvm_analysis_port_data: HashMap<ObjId, UvmAnalysisPortData>,
    pub uvm_analysis_imp_data: HashMap<ObjId, UvmAnalysisImpData>,
    pub uvm_config_db_data: HashMap<(String, String), LogicVec>,
    /// VERIF-06: waiter blocking `uvm_config_db::wait_modified(inst, field)`
    /// — keyed by (inst, field); release saat `set` dipanggil utk key tsb.
    pub uvm_config_db_waiters:
        HashMap<(String, String), Vec<crate::simulator::types::UvmSyncWaiter>>,
    /// F21: uvm_event data per objek (triggered/on).
    pub uvm_event_data: HashMap<ObjId, UvmEventData>,
    /// F21: uvm_barrier data per objek (threshold/count).
    pub uvm_barrier_data: HashMap<ObjId, UvmBarrierData>,
    /// VERIF-03: uvm_cmdline_processor — singleton id (semua get() → sama).
    pub uvm_cmdline_id: Option<ObjId>,
    /// VERIF-04: uvm_root — singleton id (semua uvm_root::get() → sama).
    pub uvm_root_id: Option<ObjId>,
    /// VERIF-05: phase UVM saat ini (nama fase yang sedang/sudah dijalankan
    /// run_phase_tree) — dipakai get_name()/jump().
    pub uvm_current_phase: Option<String>,
    /// VERIF-05: target phase jump (phase.jump("report_phase")) — saat
    /// dipanggil, run_phase_tree melompat ke fase target (skip fase di
    /// antaranya).
    pub uvm_phase_jump: Option<String>,
    /// VERIF-05: objek uvm_phase handle (cache) — di-inject sebagai argumen
    /// pertama method fase (build_phase(uvm_phase phase)) agar user bisa
    /// memanggil phase.jump()/phase.get_name().
    pub uvm_phase_handle: Option<ObjId>,
    /// VERIF-18: uvm_tr_database — singleton id (semua uvm_tr_database::get_db()
    /// → sama).
    pub uvm_tr_db_id: Option<ObjId>,
    /// VERIF-18: stream per nama — stream obj id per stream name (get_stream
    /// create/reuse).
    pub uvm_tr_streams: HashMap<String, ObjId>,
    /// VERIF-19: reverse — nama stream per obj (get_tr_count/record stream).
    pub tr_stream_names: HashMap<ObjId, String>,
    /// VERIF-18: stream default db (uvm_tr_database::set_stream) — dipakai
    /// begin_tr bila transaksi tidak punya stream sendiri.
    pub tr_db_default_stream: Option<String>,
    /// VERIF-17: record transaksi UVM — begin_tr menambah, end_tr menutup
    /// (set end_time). Dipakai report + test.
    pub tr_records: Vec<crate::simulator::types::UvmTrRecord>,
    /// VERIF-17: transaksi yang masih terbuka per obj — (start idx, name)
    /// agar end_tr menemukan record yang tepat utk obj tersebut.
    pub tr_open: HashMap<ObjId, (usize, String)>,
    /// VERIF-17: stream ter-attach per obj transaksi (set_stream).
    pub tr_obj_stream: HashMap<ObjId, String>,
    /// Nilai terakhir get_arg_value (ref out dibaca get_arg_value_out).
    pub uvm_cmdline_last_value: String,
    /// Daftar nilai get_plusargs/get_arg_values (string array).
    pub uvm_cmdline_values: Vec<String>,
    /// F21: waiter blocking `wait_trigger`/`wait_on`/`wait_for` — kontinuasi
    /// AST yang di-suspend per objek event/barrier, di-resume saat trigger/
    /// barrier penuh (pola sama dengan get_next_item blocking).
    pub uvm_sync_waiters: HashMap<ObjId, Vec<crate::simulator::types::UvmSyncWaiter>>,
    // F23: data uvm_tlm_fifo (queue ObjId + capacity + export internal).
    pub uvm_tlm_fifo_data: HashMap<ObjId, crate::simulator::types::UvmTlmFifoData>,
    /// VERIF-13: uvm_comparator / uvm_in_order_comparator — antrian expected
    /// + counter match/mismatch (write dipanggil analysis_imp).
    pub uvm_comparator_data: HashMap<ObjId, crate::simulator::types::UvmComparatorData>,
    /// VERIF-15: uvm_heartbeat — object terdaftar + jumlah heartbeat wajib.
    pub uvm_heartbeat_data: HashMap<ObjId, crate::simulator::types::UvmHeartbeatData>,
    // F23: data export analysis internal fifo (__uvm_fifo_export) — parent fifo.
    pub uvm_fifo_export_data: HashMap<ObjId, ObjId>,
    /// F24: data uvm_seq_item_port (port driver↔sequencer, sequencer ter-connect).
    pub uvm_seq_item_port_data: HashMap<ObjId, crate::simulator::types::UvmSeqItemPortData>,
    /// F21: continuation AST setelah `fork join`/`join_any` di jalur AST
    /// (class method/task UVM). ForkGroup.continuation bertipe `Vec<IrStmt>`,
    /// jadi cont AST disimpan terpisah dan dieksekusi di `fork_finish`.
    pub ast_fork_cont: HashMap<usize, Vec<mivon_ast::Stmt>>,
    pub sdf_timing_checks: Vec<TimingCheck>,
    pub uvm_resource_db_data: HashMap<(String, String), LogicVec>,
    /// UVM register layer data: register objects
    pub uvm_reg_data: std::collections::HashMap<ObjId, UvmRegData>,
    /// UVM register layer data: register field objects
    pub uvm_reg_field_data: std::collections::HashMap<ObjId, UvmRegFieldData>,
    /// UVM register layer data: register block objects
    pub uvm_reg_block_data: std::collections::HashMap<ObjId, UvmRegBlockData>,
    /// UVM register layer data: register map objects
    pub uvm_reg_map_data: std::collections::HashMap<ObjId, UvmRegMapData>,
    /// UVM callback queues: (component_type, cb_type) → registered callbacks
    pub callback_queues: HashMap<(String, String), crate::simulator::types::UvmCallbackData>,
    pub factory_type_overrides: HashMap<String, String>,
    pub root_test_obj_id: Option<ObjId>,
    /// F18: fase UVM sudah dijalankan (oleh execute_phases auto-detect ATAU
    /// run_test()). Guard anti-duplikasi: run_test() yang dipanggil dari
    /// initial block berjalan SETELAH execute_phases di run() (initial hanya
    /// di-schedule ke event loop), jadi tanpa guard ini objek test dibuat dua
    /// kali dan fase dieksekusi dua kali.
    pub uvm_phases_started: bool,
    pub process_map: HashMap<ObjId, ProcessInfo>,
    pub _next_process_id: usize,
    pub current_process_id: Option<ObjId>,
    pub cover_hits: HashMap<Symbol, u64>,
    /// VERIF-27: assertion coverage metrics — per (line, col) → (pass, fail).
    /// Dipakai report assertion coverage (assertion_pass/assertion_fail).
    pub assertion_stats: HashMap<(usize, usize), (u64, u64)>,
    /// VERIF-32: sequence coverage — per (line, col) statistik attempt
    /// concurrent assertion sequence (attempts/matched/failed).
    pub sequence_coverage: HashMap<(usize, usize), SeqCovStats>,
    pub cover_total: HashMap<Symbol, u64>,
    pub cover_bins: HashMap<Symbol, HashMap<Symbol, u64>>,
    /// Nilai coverpoint terakhir per (covergroup, coverpoint) — dipakai
    /// transition bins `(a => b)` (VERIF-31). Key = "cg.cp".
    pub covergroup_prev: HashMap<Symbol, u64>,
    /// PERF-16: Pre-computed constant-bin lookup map.
    /// Key = Symbol("cg.cp"), Value = HashMap<u64, Symbol> (value → bin name).
    /// Dihitung sekali per covergroup saat sampling pertama, di-cache.
    pub covergroup_const_bins: HashMap<Symbol, HashMap<u64, (Symbol, mivon_ast::types::BinType)>>,
    /// Iterasi loop AST terkumpul selama satu eksekusi method/task (anti-hang
    /// saat loop berisi blocking event yang tidak memajukan waktu).
    pub ast_loop_iters: u64,
    pub plusargs: HashMap<String, String>,
    pub debug_mode: DebugMode,
    pub breakpoints: Vec<Breakpoint>,
    pub watchpoints: Vec<Watchpoint>,
    pub signal_history: crate::simulator::signal_history::SignalHistoryStore,
    pub signal_last_change: HashMap<usize, u64>,
    /// Arah transisi terakhir tiap signal (untuk timing check edge-aware, SIM-24).
    pub signal_last_dir: HashMap<usize, mivon_ast::types::EdgeKind>,
    /// Waktu perubahan SEBELUM signal_last_change (untuk dedupe width/period, SIM-24).
    pub signal_prev_change: HashMap<usize, u64>,
    /// Nilai commit sebelum pulse terakhir per signal — dipakai pulse control
    /// (SIM-09) untuk rollback nilai saat pulse pendek di-reject.
    pub signal_prev_value: HashMap<usize, LogicVec>,
    /// Pulse control dari SDF (SIM-09): signal_id → lebar minimum (ns). Pulse
    /// lebih pendek dari ini di-reject (di-filter, bukan violation).
    pub sdf_pulse_controls: HashMap<usize, f64>,
    /// Dedupe pelaporan timing violation per (check_name, signal_id) → last_change
    /// yang sudah dilaporkan, supaya width/period/recovery tidak spam (SIM-24).
    pub timing_reported: HashMap<(String, usize), u64>,
    pub udp_prev_args: HashMap<Symbol, Vec<LogicVec>>,
    pub parallel_config: ParallelConfig,
    pub sysfunc_prev: HashMap<Symbol, LogicVec>,
    /// Riwayat `$past` per key — di-cap ke `n+1` entry terbaru (anti-leak
    /// O(cycles) per call-site).
    pub sysfunc_history: HashMap<Symbol, VecDeque<LogicVec>>,
    pub snapshots: Vec<StateSnapshot>,
    pub paused: bool,
    pub step_mode: StepMode,
    pub event_log: Vec<DebugEvent>,
    pub snapshot_interval: u64,
    pub assert_off_all: bool,
    pub assert_kill_all: bool,
    pub assert_modules_off: HashSet<Symbol>,
    pub coverage_options: HashMap<String, String>,
    pub coverage_enabled: bool,
    /// Selectively enable specific coverage types (empty = all enabled)
    pub coverage_enabled_types: HashSet<CoverageType>,
    /// Sembunyikan laporan coverage verbose akhir-run (dipakai mivon-fuzz:
    /// setiap simulasi memanggil engine.run; report penuh per iterasi = noise
    /// stderr + I/O; coverage keys tetap diambil via `coverage_keys()`).
    pub coverage_report_silent: bool,
    /// Trace sampling opt-in (`set_trace_interval`): baris snapshot sinyal
    /// top per interval waktu — dipakai mivon-fuzz sbg fingerprint
    /// MID-SIMULATION (bug transient yang pulih sebelum akhir run terlihat;
    /// fingerprint nilai-final buta). Default None = tanpa overhead.
    pub trace_snapshots: Vec<String>,
    pub trace_interval: Option<u64>,
    pub trace_last_time: u64,
    /// Line ranges (start, end) inklusif 1-based yang di-exclude dari line
    /// coverage oleh `` `coverage_off ``/`` `coverage_on `` (SIM-29).
    pub coverage_exclusions: Vec<(usize, usize)>,
    /// SIM-29: peta baris per statement — key `format!("{}.{:?}", process_name,
    /// discriminant)` (SAMA dengan key `cover_line`), value = baris sumber
    /// statement. Di-populate elaborator (IrDesign.stmt_lines); dipakai
    /// `record_line_hit` untuk melewati statement yang barisnya berada dalam
    /// region `` `coverage_off ``/`` `coverage_on ``.
    pub stmt_lines: HashMap<Symbol, usize>,
    /// Glitch detection: max pulse width (in time units) for A->B->A detection. 0 = disabled.
    pub glitch_window: u64,
    /// Glitch detection: per-signal (time of last change, value before last change)
    pub glitch_prev: std::collections::HashMap<SignalId, (u64, mivon_ir::LogicVec)>,
    pub cover_line: HashMap<Symbol, u64>,
    pub cover_toggle: HashMap<usize, HashSet<(LogicVal, LogicVal)>>,
    pub cover_branches: HashMap<Symbol, HashMap<Symbol, u64>>,
    pub cover_fsm: HashMap<usize, HashSet<u64>>,
    pub cover_branch_counter: u64,
    pub coverage_model_handles: HashMap<usize, Symbol>,
    pub next_coverage_model_handle: usize,
    pub sequence_attempts: Vec<SequenceAttempt>,
    pub recursion_depth: HashMap<Symbol, usize>,
    pub max_recursion_depth: usize,
    /// F35: `return` AST menandai stop-blok lintas nested (set oleh handler
    /// Stmt::Return di block.rs & ast.rs; dicek di iterasi loop blok; hanya
    /// di-clear oleh wrapper method/function helper). Field TERPISAH dari
    /// control_flow IR (Break/Continue) agar tak bocor ke evaluasi IR.
    pub ast_return_pending: bool,
    pub objection_count: usize,
    pub objection_triggered: bool,
    /// VERIF-05: objection per-objek — count utk tiap objek (raise langsung
    ///     + propagasi dari descendants via hierarki parent). get_objection_count
    ///     membacanya; end-of-test tetap berbasis objection_count global (sum).
    pub uvm_objection_data: HashMap<ObjId, u64>,
    /// JIT evaluator (native code compilation for fast expression eval)
    pub jit_evaluator: Option<crate::simulator::JITEvaluator>,
    /// Use packed 4-state bitmask eval (SIMD-ready) for bitwise operations
    pub use_packed_eval: bool,
    /// Use expression-level JIT compilation (compiles entire IrExpr tree at once)
    pub use_jit_expression: bool,
    /// Zero-deallocation arena for temporary allocations during simulation
    pub sim_arena: SimulationArena,
    /// DAG-parallel process evaluator (built lazily)
    pub sim_dag: Option<SimulationDag>,
    /// Enable DAG-parallel process evaluation
    pub use_dag_parallel: bool,
    /// Layer resource guard (MIVON-SIM-34): pantau RSS, cegah OOM kill.
    pub resource_guard: crate::simulator::guard::SimResourceGuard,
    /// Clock domain analysis + fused domains for cycle-based simulation
    pub clock_analysis: Option<ClockDomainAnalysis>,
    /// Enable cycle-based simulation fusion
    pub use_cycle_fusion: bool,
    /// SIM-20: cycle-based simulation mode (`--cycle`) — clock didrive
    /// internal scheduler tanpa iterasi delta IEEE 1800 (subset desain,
    /// lihat engine/cycle_based.rs).
    pub cycle_based: bool,
    /// SIM-20: periode clock (unit waktu desain) untuk mode cycle-based.
    pub cycle_period: u64,
    /// MIR JIT compiler for compiled-code simulation path
    pub mir_jit: Option<mivon_compiler::mir::MirJitCompiler>,
    /// Enable MIR JIT for combinational process evaluation
    pub use_mir_jit: bool,
    /// Diagnostic collector for structured runtime diagnostics
    pub diag_sink: DiagSink,
    /// Current delta cycle count (for runtime context)
    pub current_delta: u64,
    /// Current process name (for runtime context)
    pub current_process_name: Option<String>,
    /// Current instance path (for runtime context), e.g. "soc.cpu0.fetch"
    pub current_instance_path: Option<String>,
    /// Posisi source (line, col) terakhir yang diketahui saat evaluasi —
    /// di-set dari ekspresi berposisi (Ident/FuncCall/ScopedIdent). Dipakai
    /// warning/error runtime yang tidak membawa lokasi eksplisit agar selalu
    /// mencantumkan file:line:col (F20). Cell agar bisa di-set dari &mut self
    /// evaluator dan dibaca dari &self helper emit.
    pub cur_src_line: std::cell::Cell<usize>,
    pub cur_src_col: std::cell::Cell<usize>,
    /// Race detection: tracks which process (ObjId) last wrote each signal in current delta
    pub signal_writers: std::collections::HashMap<SignalId, Option<ObjId>>,
    /// SIM-12: tracks write type per signal per delta (true = blocking, false = non-blocking)
    pub signal_write_types: std::collections::HashMap<SignalId, bool>,
    /// Race detection: write count per signal per time step (for oscillation detection)
    pub signal_write_count: std::collections::HashMap<SignalId, u32>,
    /// Max delta cycles per time step before abort (configurable for testing)
    pub delta_limit: u64,
    /// GUARD HANG-1 (fuZZ): `#0` zero-delay loop (`forever #0 clk = ~clk`) —
    /// tiap `#0` dijadwalkan ulang (Inactive, delta-loop) sambil meng-clone
    /// body loop; delta-limit 100k baru kena setelah >40s debug (kloning berat).
    /// Guard: jumlah `#0` SAMA-WAKTU per time step — lewat cap → error
    /// InfiniteDelta CEPAT (<10s; IDEAL <1s, task MIVON waktu eksekusi) —
    /// bukan hang.
    pub zero_delay_seen_t: u64,
    pub zero_delay_same_time: u64,
    /// GUARD HANG-2 (fuZZ): revisit time step SAMA (tanpa advance waktu) —
    /// lapis kedua untuk zero-advance churn di luar delta-loop.
    pub last_processed_t: u64,
    pub same_time_revisits: u64,
    /// Oscillation detection: hash state sinyal yang berbeda-beda per delta
    /// (urutan state BERBEDA) dalam satu time step. State berulang non-kontigu
    /// = kombinational loop (cycle) → abort cepat, bukan menunggu delta_limit.
    /// Dikosongkan setiap time step baru.
    pub osc_state_hashes: std::collections::HashSet<u64>,
    /// Hash state commit terakhir dalam time step ini (untuk mendeteksi plateau).
    pub osc_last_state_hash: Option<u64>,
    /// Cache analisis akses sinyal per process (SIM-28). Dibangun lazy saat
    /// trigger_sensitive_processes pertama. Dipakai snapshot sparse: base
    /// hanya sinyal yang diakses process yang terpicu (bukan seluruh sinyal).
    pub comb_access: Vec<crate::scheduler::sim_dag::SignalAccess>,
    /// Sudah pernah membangun comb_access?
    pub comb_access_ready: bool,

    /// Co-simulation state (shared with external simulator via TCP)
    pub cosim_state: Option<std::sync::Arc<std::sync::Mutex<crate::simulator::cosim::CosimState>>>,

    /// Co-simulation signal mapping: (signal_id, signal_name, direction)
    pub cosim_signals: Vec<(usize, String, bool)>,

    /// Foreign event queue (VPI/VHPI/PLI/DPI) — unified event queue for foreign
    /// callbacks (value-change, read-write sync, read-only sync, next time step,
    /// registered callbacks, end of simulation). Processed by scheduler in
    /// appropriate IEEE 1800 region.
    pub foreign_events: Vec<ForeignEvent>,
    /// Flag: ada event dijadwalkan di luar jendela alokasi (MAX_EVENT_SPAN).
    /// Run loop memeriksanya → abort dengan diagnostic, bukan panic/OOM.
    pub event_alloc_exceeded: bool,

    /// Per-path signal delays from SDF annotation: "cell_name:from->to" → SignalDelay
    pub signal_delays: std::collections::HashMap<String, crate::simulator::state::SignalDelay>,

    /// UPF (Unified Power Format) power intent database for power-aware simulation
    pub power_intent: Option<crate::simulator::upf::PowerIntent>,

    /// Process body cache for DAG parallel evaluation.
    /// Dibangun sekali di run() untuk menghindari clone bodies setiap cycle.
    pub process_body_cache: HashMap<usize, Vec<IrStmt>>,

    /// Fungsi system yang sudah diberi warning RT9003 sekali (dedup) — hindari
    /// spam 20 warning per penggunaan utk `$realtobits`/`$bitstoreal` dll.
    pub notimpl_warned: std::collections::HashSet<Symbol>,

    /// Hierarchical timing wheel for O(1) event scheduling (replaces Vec<Vec<RegionEvent>>).
    /// When enabled, events are stored in the timing wheel instead of `events: Vec<Vec<RegionEvent>>`.
    pub timing_wheel:
        Option<crate::simulator::engine::scheduler::timing_wheel::HierarchicalTimingWheel>,

    /// Whether to use the timing wheel for event scheduling.
    pub use_timing_wheel: bool,

    /// Performance monitoring dashboard (SIM-25): metrik simulasi runtime
    /// (delta cycles, events processed, throughput). Dipakai CLI --perf-dashboard.
    pub sim_perf: mivon_compiler::profiling::PerfDashboard,
}

// ============================================================================
// CATATAN: Fungsi-fungsi helper standalone (evaluate_string_method,
// sym_char_matches, edge_matches_abbrev) sudah dipindahkan ke
// src/simulator/engine/engine_utils.rs untuk memisahkan tanggung jawab.
// ============================================================================
