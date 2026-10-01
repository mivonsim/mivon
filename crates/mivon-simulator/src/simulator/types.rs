use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::HashMap;
use std::fmt;

// ── Debug types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DebugMode {
    Normal,
    Debug,
    DeepDebug,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepMode {
    Running,
    Paused,
    StepCycle,
}

#[derive(Debug, Clone)]
pub enum Breakpoint {
    Cycle(u64),
    SignalEq(String, LogicVec),
    SignalNeq(String, LogicVec),
    SignalChange(String),
    Module(String),
}

impl fmt::Display for Breakpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Breakpoint::Cycle(c) => write!(f, "break cycle {}", c),
            Breakpoint::SignalEq(n, v) => write!(f, "break signal {} == {}", n, v),
            Breakpoint::SignalNeq(n, v) => write!(f, "break signal {} != {}", n, v),
            Breakpoint::SignalChange(n) => write!(f, "break change {}", n),
            Breakpoint::Module(n) => write!(f, "break module {}", n),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Watchpoint {
    Signal(String),
    MemAddr(u64),
}

impl fmt::Display for Watchpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Watchpoint::Signal(n) => write!(f, "watch {}", n),
            Watchpoint::MemAddr(a) => write!(f, "watch mem[{:#x}]", a),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DebugEvent {
    pub kind: DebugEventKind,
    pub time: u64,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum DebugEventKind {
    BreakpointHit,
    WatchpointHit,
    StepComplete,
    SignalChanged,
}

#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub time: u64,
    pub signals: Vec<LogicVec>,
    pub next_signals: Vec<LogicVec>,
    pub changed: Vec<bool>,
}

#[derive(Debug, Clone)]
pub enum EventKind {
    EvalProcess(usize),
    ContinueBlock(Continuation),
    ContinueAstBlock(
        Vec<mivon_ast::Stmt>,
        Option<usize>,
        Option<ObjId>,
        Option<Symbol>,
    ),
    /// WAV-13: commit tertunda dari write signal ber-annotasi SDF delay.
    /// `annotate_sdf` mengisi `IrSignal.delay_rise/delay_fall` (ps) tapi
    /// sebelumnya tidak pernah dibaca — write langsung commit di t, padahal
    /// harus muncul di t + delay. `write_lvalue` menjadwalkan event ini;
    /// handler (`process_event`) commit dengan resolusi multi-driver +
    /// record_signal_change seperti write normal.
    SdfDelayedWrite {
        sig_id: usize,
        value: LogicVec,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventRegion {
    Preponed = 1,
    PreActive = 2,
    Active = 3,
    Inactive = 4,
    PreNba = 5,
    Nba = 6,
    PostNba = 7,
    PreObserved = 8,
    Observed = 9,
    PostObserved = 10,
    Reactive = 11,
    PostReactive = 12,
    Postponed = 13,
}

pub(super) const IEEE_REGIONS: [EventRegion; 13] = [
    EventRegion::Preponed,
    EventRegion::PreActive,
    EventRegion::Active,
    EventRegion::Inactive,
    EventRegion::PreNba,
    EventRegion::Nba,
    EventRegion::PostNba,
    EventRegion::PreObserved,
    EventRegion::Observed,
    EventRegion::PostObserved,
    EventRegion::Reactive,
    EventRegion::PostReactive,
    EventRegion::Postponed,
];

#[derive(Debug, Clone)]
pub struct RegionEvent {
    pub region: EventRegion,
    pub event: EventKind,
}

/// Blocking event control `@(sig)` / `@(posedge sig)` yang sedang menunggu
/// perubahan/edge pada satu atau lebih signal. SATU entry mewakili SATU `@(...)`:
/// fire sekali saat sinyal mana pun berubah, cegah double-fire / event stale.
/// Deteksi fire memakai `signal_snapshot` (nilai awal delta) sebagai baseline
/// "sebelum perubahan" — benar untuk level DAN edge (lihat process_pending_events).
#[derive(Debug, Clone)]
pub struct PendingEventControl {
    /// Semua (signal, edge) dari satu `@(a or b)` — None edge = level.
    pub sigs: Vec<(SignalId, Option<ClockEdge>)>,
    /// Statement yang dilanjutkan setelah event terpenuhi (body + sisa + loop_cont).
    pub continuation: Vec<IrStmt>,
    /// LANG-27: guard `iff (cond)` — continuation hanya lanjut bila cond true.
    pub iff: Option<IrExpr>,
    /// Fork group branch tempat `@(...)` ini berada (None = bukan branch fork).
    /// Tanpa ini, branch fork yang suspend di `@(ev)` lalu resume kehilangan
    /// akuntansi ForkGroup → `join` tidak pernah selesai (probe v03:
    /// `fork @(ev) ... #5 -> ev ... join`).
    pub fork_id: Option<usize>,
}

/// Blocking event control `@(...)` di jalur AST (class method/task UVM).
/// Menyimpan konteks method (this/locals/method) agar continuation bisa
/// di-resume dengan benar (resume AST task kehilangan konteks tanpa ini).
#[derive(Debug, Clone)]
pub struct PendingAstEventControl {
    /// Semua (signal, edge) dari satu `@(a or b)` — SATU entry, cegah double-fire.
    pub sigs: Vec<(SignalId, Option<ClockEdge>)>,
    pub continuation: Vec<mivon_ast::Stmt>,
    pub this: Option<ObjId>,
    pub method: Option<Symbol>,
    /// Snapshot lengkap method_locals saat suspensi.
    pub locals: Vec<HashMap<Symbol, LogicVec>>,
    /// Jumlah frame locals saat suspensi — dipakai truncate saat continuation selesai.
    pub base_len: usize,
    /// LANG-27: guard `iff (cond)` di jalur AST — diperiksa saat event fire.
    pub iff: Option<mivon_ast::Expr>,
}

#[derive(Debug, Clone)]
pub struct ForkGroup {
    pub(super) remaining: usize,
    pub(super) continuation: Vec<IrStmt>,
    /// Continuation sudah dijadwalkan/dieksekusi — cegah evaluasi ganda.
    pub(super) fired: bool,
    /// Group masih dipakai (belum di-retire ke `fork_free`).
    pub(super) active: bool,
    /// true = Join/JoinNone (slot aman di-reuse saat semua branch selesai).
    /// false = JoinAny (branch yang dibuang masih mereferensi fid → jangan reuse).
    pub(super) reclaimable: bool,
    /// LANG-29: nama proses top-level yang membuat group ini
    /// (`current_process_name` saat `fork` dieksekusi). Dipakai `wait fork`
    /// untuk memilih group milik proses yang sedang berjalan. None = dibuat
    /// di luar konteks proses (fallback: ikut semua group aktif).
    pub(super) spawner: Option<String>,
    /// LANG-30: `disable fork` — child processes group ini di-terminate.
    /// Branch yang masih tertunda (ContinueBlock/ContinueAstBlock dengan
    /// fork_id ini) di-skip tanpa eksekusi dan langsung decrement.
    pub(super) disabled: bool,
}

/// LANG-29: satu situs `wait fork;` yang sedang menunggu. Dipangkas di
/// `fork_finish` (saat group selesai); bila `fids` kosong, kontinuasi
/// dieksekusi. Mendukung jalur IR (module/initial) dan AST (task/method).
#[derive(Debug, Clone)]
pub struct WaitForkState {
    /// Fork group yang masih ditunggu (fid) — dipangkas saat group selesai.
    pub fids: Vec<usize>,
    /// Kontinuasi jalur IR (statement setelah `wait fork`).
    pub continuation: Vec<IrStmt>,
    /// Kontinuasi jalur AST (task/method) — kosong bila jalur IR.
    pub ast_continuation: Vec<mivon_ast::Stmt>,
    /// Konteks task/method saat suspend (jalur AST).
    pub this: Option<ObjId>,
    pub method: Option<Symbol>,
    pub locals: Vec<HashMap<Symbol, LogicVec>>,
    pub base_len: usize,
    /// Nama proses saat `wait fork` dieksekusi — di-restore sebelum kontinuasi
    /// dijalankan (check_wait_forks berjalan di luar konteks EvalProcess,
    /// jadi current_process_name bisa menunjuk proses lain).
    pub process_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Continuation {
    pub stmts_to_exec: Vec<IrStmt>,
    pub stmts_remaining: Vec<IrStmt>,
    pub fork_id: Option<usize>,
    pub process_id: Option<ObjId>,
    /// LANG-29: nama proses saat suspend — ContinueBlock me-restore
    /// `current_process_name` agar `wait fork` (dan fitur berbasis nama proses)
    /// tetap melihat proses yang benar setelah resume. None = tidak diketahui.
    pub process_name: Option<String>,
    /// Label `begin : <label>` yang terbuka saat kontinuasi dijadwalkan
    /// (BUG FIX disable-label lintas branch: `disable <label>` dari branch
    /// lain harus bisa membunuh branch yang sedang SUSPEND di dalam blok itu
    /// — tanpa label ini `disable_pending` tak punya konsumen sehingga bocor
    /// dan menelan seluruh statement sesudahnya, termasuk continuation join).
    pub named_labels: Vec<Symbol>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlowControl {
    Break,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcessStatus {
    Finished = 0,
    Running = 1,
    Waiting = 2,
    Suspended = 3,
    Killed = 4,
}

#[derive(Debug, Clone)]
pub struct UvmObjectData {
    pub name: String,
}

/// VERIF-17: satu record transaksi UVM — dibuat oleh `begin_tr(name)` pada
/// uvm_transaction/uvm_sequence_item, ditutup oleh `end_tr()`. Waktu memakai
/// time simulation (state.time); `stream` = nama tr_stream yang dilampirkan
/// (via `set_stream`/`db.get_stream`), None bila tidak ada.
#[derive(Debug, Clone)]
pub struct UvmTrRecord {
    /// Nama transaksi (argumen begin_tr).
    pub name: String,
    /// Obj id transaksi (uvm_transaction/uvm_sequence_item instance).
    pub obj_id: ObjId,
    /// Nama stream tempat record dilampirkan (VERIF-18/19), None = default.
    pub stream: Option<String>,
    /// Waktu mulai (state.time saat begin_tr).
    pub start_time: u64,
    /// Waktu selesai (state.time saat end_tr) — None selama masih terbuka.
    pub end_time: Option<u64>,
}

/// UVM callback data: registered callback objects for a callback type.
#[derive(Debug, Clone)]
pub struct UvmCallbackData {
    /// Callback type name (e.g., "my_callbacks")
    pub cb_type_name: String,
    /// Registered callback objects (ObjId → callback instance name)
    pub callbacks: Vec<(ObjId, String)>,
    /// Whether the callback is enabled
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct UvmComponentData {
    pub parent: Option<ObjId>,
    pub children: Vec<ObjId>,
    pub report_verbosity: u32,
}

/// UVM event data: sinkronisasi antar komponen (`uvm_event`).
/// `triggered`/`on` di-set oleh trigger(); waiter (`wait_trigger`/`wait_on`)
/// di-block sampai flag naik — pola blocking ContinueAstBlock di block.rs.
#[derive(Debug, Clone)]
pub struct UvmEventData {
    /// Nama event (dari `new(name)`).
    pub name: String,
    /// Flag `triggered` — dibaca `triggered()` / `wait_trigger()`.
    pub triggered: bool,
    /// Flag `on` — dibaca `is_on()` / `wait_on()`.
    pub on: bool,
}

impl UvmEventData {
    /// F21 review: `on` default FALSE (semantics UVM — event baru off;
    /// `wait_on()` harus block sampai `trigger()`/`on_off(1)` menyalakannya).
    /// Sebelumnya `on: true` membuat `wait_on()` langsung return (tak pernah
    /// block). `reset()` juga mematikan `on` (UVM reset: on=0, triggered=0).
    pub fn new(name: String) -> Self {
        UvmEventData {
            name,
            triggered: false,
            on: false,
        }
    }
}

/// UVM barrier data: sinkronisasi N-proses (`uvm_barrier`).
/// Setiap `wait_for()` menambah `count`; saat `count >= threshold` semua
/// waiter di-release dan count di-reset ke 0 (auto-reset, seperti UVM asli).
#[derive(Debug, Clone)]
pub struct UvmBarrierData {
    /// Nama barrier (dari `new(name, threshold)`).
    pub name: String,
    /// Jumlah proses yang harus menunggu sebelum semua dilepas.
    pub threshold: u32,
    /// Jumlah proses yang sudah sampai (dari wait_for/wait_for_count).
    pub count: u32,
}

impl UvmBarrierData {
    pub fn new(name: String, threshold: u32) -> Self {
        UvmBarrierData {
            name,
            threshold: threshold.max(1),
            count: 0,
        }
    }
}

/// F23: uvm_tlm_fifo — buffer FIFO TLM dgn blocking put/get/peek.
/// Queue berisi ObjId item (handle object). `put` saat penuh & `get` saat
/// kosong di-block (suspend + waiter, pola sama dengan uvm_event/barrier);
/// `analysis_export.write(item)` (export internal) memetakan ke put non-block.
#[derive(Debug, Clone)]
pub struct UvmTlmFifoData {
    pub name: String,
    /// Antrean item (ObjId).
    pub queue: std::collections::VecDeque<ObjId>,
    /// Kapasitas (default 1, dari `new(name, parent, size)`).
    pub capacity: usize,
    /// Export internal (`analysis_export`) — id objek `__uvm_fifo_export`.
    pub export_id: Option<ObjId>,
}

impl UvmTlmFifoData {
    pub fn new(name: String, capacity: usize) -> Self {
        UvmTlmFifoData {
            name,
            queue: std::collections::VecDeque::new(),
            capacity: capacity.max(1),
            export_id: None,
        }
    }
}

/// F21: waiter blocking `wait_trigger`/`wait_on`/`wait_for`/`wait_for_count`
/// yang di-suspend — kontinuasi AST + konteks method, di-resume saat
/// trigger()/barrier penuh (pola sama dengan get_next_item blocking).
#[derive(Debug, Clone)]
pub struct UvmSyncWaiter {
    /// Statement sisa setelah titik suspend (statement wait TIDAK diulang
    /// karena `wait_for` punya side effect count += 1).
    pub continuation: Vec<mivon_ast::Stmt>,
    pub fork_id: Option<usize>,
    pub this: Option<ObjId>,
    /// Konteks method class yang sedang berjalan (dipakai resume ContinueAstBlock).
    pub method: Option<Symbol>,
    /// F23: label wait utk release selektif — "get"/"peek"/"put" (fifo)
    /// atau nama method wait lain (event/barrier). Bukan konteks method.
    pub wait_label: String,
}

#[derive(Debug, Clone)]
pub struct UvmSequencerData {
    pub item_queue: Vec<ObjId>,
    pub current_item: Option<ObjId>,
}

#[derive(Debug, Clone)]
pub struct UvmDriverData {
    pub sequencer_id: Option<ObjId>,
    pub current_item: Option<ObjId>,
}

/// F24: uvm_seq_item_port — port koneksi driver↔sequencer (UVM asli
/// `seq_item_port.get_next_item(req)` / `.item_done()`). Menyimpan sequencer
/// yang di-connect; method get_next_item/item_done/try_next_item mendelegasi
/// ke sequencer tsb. Blocking get_next_item di-intercept block.rs (waiter
/// keyed by sequencer id, label "get_next_item").
#[derive(Debug, Clone)]
pub struct UvmSeqItemPortData {
    pub sequencer_id: Option<ObjId>,
}

#[derive(Debug, Clone)]
pub struct UvmAnalysisPortData {
    pub connections: Vec<ObjId>,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct UvmAnalysisImpData {
    pub parent: Option<ObjId>,
    pub name: String,
}

/// Data internal `uvm_comparator` / `uvm_in_order_comparator` (VERIF-13):
/// antrian expected (in-order) + counter match/mismatch. `write(actual)`
/// dipanggil analysis_imp → pop expected head → compare → increment counter.
/// `get_match_count()` / `get_mismatch_count()` membaca counter.
#[derive(Debug, Clone)]
pub struct UvmComparatorData {
    /// Antrian expected (obj id) — in-order: head dibandingkan dengan actual.
    pub expected: std::collections::VecDeque<ObjId>,
    pub matches: u64,
    pub mismatches: u64,
}

/// Data internal `uvm_heartbeat` (VERIF-15): object yang di-monitor wajib
/// memanggil `heartbeat(obj)` minimal `required` kali sebelum `check()`
/// (biasanya di check_phase/report_phase). `check()` mengembalikan 0 dan
/// emit error bila ada object yang heartbeat-nya kurang.
#[derive(Debug, Clone, Default)]
pub struct UvmHeartbeatData {
    /// Object terdaftar → jumlah heartbeat yang wajib dipenuhi.
    pub required: std::collections::HashMap<ObjId, u64>,
    /// Object → jumlah heartbeat yang sudah diterima.
    pub received: std::collections::HashMap<ObjId, u64>,
}

/// UVM register field data: bit position within a register, width, access policy.
#[derive(Debug, Clone)]
pub struct UvmRegFieldData {
    /// Parent register object ID
    pub parent_reg: Option<ObjId>,
    /// Bit position (LSB) within the register
    pub bit_pos: usize,
    /// Width in bits
    pub width: usize,
    /// Current value (mirrored)
    pub value: LogicVec,
    /// Desired value (next value to be written)
    pub desired: LogicVec,
    /// Access policy: "RO", "WO", "RW", "RC", "RS", "WRC", "WRS", "WC", "WS", "W1C", "W1S", etc.
    pub access: String,
    /// Whether the field has been modified since last update
    pub modified: bool,
    /// Whether the field is volatile
    pub volatile: bool,
}

impl UvmRegFieldData {
    pub fn new() -> Self {
        UvmRegFieldData {
            parent_reg: None,
            bit_pos: 0,
            width: 1,
            value: LogicVec::new(1),
            desired: LogicVec::new(1),
            access: "RW".to_string(),
            modified: false,
            volatile: false,
        }
    }
}

impl Default for UvmRegFieldData {
    fn default() -> Self {
        Self::new()
    }
}

/// UVM register data: a named register with address, width, and fields.
#[derive(Debug, Clone)]
pub struct UvmRegData {
    /// Register address in the address map
    pub address: u64,
    /// Total width in bits (sum of field widths)
    pub width: usize,
    /// Current mirrored value
    pub value: LogicVec,
    /// Desired value (pending write)
    pub desired: LogicVec,
    /// List of field object IDs belonging to this register
    pub fields: Vec<ObjId>,
    /// Whether register has been modified since last update
    pub modified: bool,
    /// Parent block data
    pub parent_block: Option<ObjId>,
}

impl UvmRegData {
    pub fn new() -> Self {
        UvmRegData {
            address: 0,
            width: 32,
            value: LogicVec::new(32),
            desired: LogicVec::new(32),
            fields: Vec::new(),
            modified: false,
            parent_block: None,
        }
    }
}

impl Default for UvmRegData {
    fn default() -> Self {
        Self::new()
    }
}

/// UVM register block data: a block containing registers with an address map.
#[derive(Debug, Clone)]
pub struct UvmRegBlockData {
    /// Address map: offset -> register object ID
    pub regs_by_offset: std::collections::HashMap<u64, ObjId>,
    /// Default register map object ID (uvm_reg_map instance)
    pub default_map: Option<ObjId>,
    /// Base address of the block
    pub base_address: u64,
}

impl UvmRegBlockData {
    pub fn new() -> Self {
        UvmRegBlockData {
            regs_by_offset: std::collections::HashMap::new(),
            default_map: None,
            base_address: 0,
        }
    }
}

impl Default for UvmRegBlockData {
    fn default() -> Self {
        Self::new()
    }
}

/// UVM register map data: address decoding for a set of registers.
#[derive(Debug, Clone)]
pub struct UvmRegMapData {
    /// Address map: offset -> register object ID
    pub regs_by_offset: std::collections::HashMap<u64, ObjId>,
    /// Base address
    pub base_address: u64,
    /// Address width in bits
    pub n_bits: usize,
}

impl UvmRegMapData {
    pub fn new() -> Self {
        UvmRegMapData {
            regs_by_offset: std::collections::HashMap::new(),
            base_address: 0,
            n_bits: 32,
        }
    }
}

impl Default for UvmRegMapData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct WaitOrderState {
    pub events: Vec<SignalId>,
    pub expected_idx: usize,
    pub continuation: Vec<IrStmt>,
    pub failure_stmts: Vec<IrStmt>,
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub status: ProcessStatus,
    pub await_continuations: Vec<Vec<IrStmt>>,
}

/// Coverage types that can be selectively enabled
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoverageType {
    Line,
    Toggle,
    Branch,
    Fsm,
    Covergroup,
}

/// X-propagation mode for controlling how X (unknown) values propagate
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum XPropagationMode {
    /// X is masked by deterministic values: 0 & X = 0, 1 | X = 1, X == 5 = X
    Optimistic,
    /// Any X input produces X output: 0 & X = X, 1 | X = X
    Pessimistic,
    /// X treated as potentially any value — strictest checking
    XAnywhere,
}

impl XPropagationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            XPropagationMode::Optimistic => "optimistic",
            XPropagationMode::Pessimistic => "pessimistic",
            XPropagationMode::XAnywhere => "x-anywhere",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "optimistic" | "opt" => Some(XPropagationMode::Optimistic),
            "pessimistic" | "pess" => Some(XPropagationMode::Pessimistic),
            "x-anywhere" | "anywhere" | "xany" => Some(XPropagationMode::XAnywhere),
            _ => None,
        }
    }
}

/// Konfigurasi format waktu untuk `%t` (set oleh `$timeformat`).
/// units = eksponen unit (mis. -9 = ns, -12 = ps), sesuai IEEE 1800.
#[derive(Debug, Clone)]
pub struct TimeFormat {
    pub units: i64,
    pub precision: i64,
    pub suffix: String,
    pub min_field_width: usize,
    /// Eksponen unit basis sim-time (mis. -9 = ns). Diderivasi dari
    /// `` `timescale `` desain saat engine init; dipakai untuk skala %t.
    pub base_units: i64,
}

impl TimeFormat {
    /// Parse eksponen unit dari string seperti "1ns", "10ps", "100us", "1ms".
    /// Mengembalikan None jika tidak dikenal (caller fallback ke -9 = ns).
    pub fn unit_exponent(unit: &str) -> Option<i64> {
        let unit = unit.trim();
        if unit.ends_with("fs") {
            Some(-15)
        } else if unit.ends_with("ps") {
            Some(-12)
        } else if unit.ends_with("ns") {
            Some(-9)
        } else if unit.ends_with("us") {
            Some(-6)
        } else if unit.ends_with("ms") {
            Some(-3)
        } else if unit.ends_with('s') {
            Some(0)
        } else {
            None
        }
    }
}

impl Default for TimeFormat {
    fn default() -> Self {
        TimeFormat {
            units: -9,
            precision: 0,
            suffix: String::new(),
            min_field_width: 0,
            base_units: -9,
        }
    }
}
