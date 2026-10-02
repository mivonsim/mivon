use mivon_core::intern::Symbol;
use mivon_core::{LogicVal, LogicVec};
use std::collections::HashMap;

pub type SignalId = usize;
pub type ClassId = usize;
pub type ObjId = usize;

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct IrDesign {
    pub top: IrModule,
    pub modules: HashMap<Symbol, IrModule>,
    pub classes: HashMap<Symbol, IrClassDef>,
    pub covergroups: Vec<IrCovergroup>,
    pub dpi_imports: Vec<IrDpiImport>,
    pub hier_signal_map: HashMap<Symbol, SignalId>,
    pub udp_defs: Vec<mivon_ast::types::UdpDef>,
    pub specify_items: Vec<mivon_ast::types::SpecifyItem>,
    pub timescale: Option<(String, String)>,
    /// Module-level recursive function declarations — kept for runtime evaluation (not inlined)
    pub module_functions: HashMap<Symbol, mivon_ast::types::FunctionDecl>,
    /// Preprocessed source lines for rich diagnostics (used by SimulationEngine)
    pub source_lines: Option<Vec<String>>,
    /// First source file path (for source snippets)
    pub source_file: Option<String>,
    /// Konstanta package global (qualified `pkg::name` → nilai i64), termasuk
    /// enum member dan parameter package. Dipakai evaluasi `pkg::item` saat
    /// runtime (ScopedIdent) di method class / constraint.
    pub pkg_scoped_consts: HashMap<Symbol, i64>,
    /// Line ranges (start, end) inklusif 1-based — dari `` `coverage_off ``/
    /// `` `coverage_on `` (koordinat output preprocessed). Dipakai engine untuk
    /// mengecualikan baris dari line coverage (SIM-29).
    pub coverage_exclusions: Vec<(usize, usize)>,
    /// SIM-29: peta baris statement untuk filtering line coverage per-statement.
    /// Key = `format!("{}.{:?}", process_name, std::mem::discriminant(stmt))` —
    /// SAMA dengan key `record_line_hit` (engine/coverage.rs), value = baris
    /// sumber statement (1-based, koordinat output preprocessed). Di-populate
    /// elaborator (stmt.rs) dari AST via `expr_location`; engine
    /// `record_line_hit` melewati statement yang barisnya berada dalam
    /// `` `coverage_off ``/`` `coverage_on `` (is_line_excluded).
    pub stmt_lines: HashMap<Symbol, usize>,
    /// LANG-08: net alias (IEEE 1800-2017 §10.9) — peta member → canonical
    /// SignalId. `alias a = b;` membuat a dan b satu jaringan: engine
    /// mer-direct read/write member ke canonical (state.alias_redirect),
    /// sehingga menulis ke salah satu terlihat di semua (short).
    pub net_aliases: HashMap<SignalId, SignalId>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrDpiImport {
    pub name: Symbol,
    pub return_width: usize,
    pub arg_widths: Vec<usize>,
    pub is_task: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrCovergroup {
    pub name: Symbol,
    pub coverpoints: Vec<IrCoverpoint>,
    pub crosses: Vec<IrCross>,
    /// VERIF-28: `type_option.weight = N` — bobot utk functional coverage
    /// keseluruhan (default 1).
    pub weight: u64,
    /// VERIF-28: `type_option.per_instance = 1` — coverage per-instance
    /// (default false = merge semua instance ke satu akumulator).
    pub per_instance: bool,
    /// IMPLICIT SAMPLING (IEEE 1800 §19.8): signal clocking event
    /// `@(posedge clk)` — engine sample otomatis tiap edge (None = hanya
    /// `sample()` eksplisit). Dulu event tak turun ke IR → covergroup
    /// `@event` tanpa sample() = 0 samples senyap.
    pub event_signal: Option<SignalId>,
    /// true = posedge, false = negedge.
    pub event_posedge: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrCoverpoint {
    pub name: Symbol,
    pub expr: IrExpr,
    /// Bin eksplisit (bins/illegal_bins/ignore_bins) — VERIF-30. Sebelumnya
    /// di-parse parser tapi di-drop elaborator & diabaikan sampler (hanya
    /// auto-binning default).
    pub bins: Vec<IrBin>,
}

/// Satu bin eksplisit coverpoint: nama + daftar range nilai.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrBin {
    pub name: Symbol,
    pub ranges: Vec<IrBinRange>,
    pub bin_type: mivon_ast::types::BinType,
    /// Transition bins `(a => b => ...)` — VERIF-31: tiap Vec<IrExpr> = satu
    /// sekuens nilai (panjang 2 = kasus umum `prev => curr`).
    pub transitions: Vec<Vec<IrExpr>>,
}

/// Satu range nilai bin: `[lo:hi]` (high Some) atau nilai tunggal (high None).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrBinRange {
    pub low: IrExpr,
    pub high: Option<IrExpr>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrCross {
    pub name: Symbol,
    pub coverpoints: Vec<Symbol>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrTypeParam {
    pub name: Symbol,
    pub default_type: Option<mivon_ast::types::DataType>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrClassDef {
    pub name: Symbol,
    pub extends: Option<Symbol>,
    pub type_params: Vec<IrTypeParam>,
    pub fields: Vec<IrClassField>,
    pub methods: Vec<IrClassMethod>,
    /// (block_name, is_static, items) — LANG-32: `is_static=true` berarti
    /// block constraint dibagi antar semua instance (constraint_mode()-nya
    /// berlaku global per-class, IEEE 1800-2017 §18.5.10).
    pub constraints: Vec<(Symbol, bool, Vec<mivon_ast::types::ConstraintItem>)>,
    pub rand_fields: Vec<Symbol>,
    /// LANG-40: `let` declaration di dalam class (IEEE 1800-2017 §11.12.2).
    pub lets: Vec<mivon_ast::types::LetDecl>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ObjectData {
    pub class_name: Symbol,
    pub fields: HashMap<Symbol, LogicVec>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrClassField {
    pub name: Symbol,
    pub width: usize,
    pub array_depth: usize,
    pub elem_width: usize,
    /// F18: tipe deklarasi field (`my_env env;` → UserDefined "my_env").
    /// Dipakai resolve_new_class_hint untuk `field = new(...)` di build_phase
    /// — tanpa ini objek class UVM dibuat dengan class_name kosong dan
    /// constructor/parent-link tidak pernah dijalankan.
    pub dtype: Option<mivon_ast::DataType>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrClassMethod {
    pub name: Symbol,
    pub is_task: bool,
    pub virtual_flag: bool,
    pub is_static: bool,
    pub ports: Vec<mivon_ast::FunctionPort>,
    pub decls: Vec<mivon_ast::Decl>,
    pub stmts: Vec<mivon_ast::Stmt>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct IrModule {
    pub name: Symbol,
    pub signals: Vec<SignalInfo>,
    pub inputs: Vec<SignalId>,
    pub outputs: Vec<SignalId>,
    pub inouts: Vec<SignalId>,
    pub processes: Vec<Process>,
    pub sub_instances: Vec<IrInstance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum NetType {
    #[default]
    Wire,
    Wand,
    Wor,
    Tri,
    Tri0,
    Tri1,
    TriAnd,
    TriOr,
    Supply0,
    Supply1,
}

impl NetType {
    pub fn resolve_bit(&self, current: LogicVal, incoming: LogicVal) -> LogicVal {
        match self {
            NetType::Wand | NetType::TriAnd | NetType::Supply0 => {
                // Wired-AND: Z = transparent, otherwise AND
                match (current, incoming) {
                    (LogicVal::X, _) | (_, LogicVal::X) => LogicVal::X,
                    (LogicVal::Z, v) => v,
                    (v, LogicVal::Z) => v,
                    (LogicVal::Zero, _) | (_, LogicVal::Zero) => LogicVal::Zero,
                    _ => LogicVal::One,
                }
            }
            NetType::Wor | NetType::TriOr | NetType::Supply1 => {
                // Wired-OR: Z = transparent, otherwise OR
                match (current, incoming) {
                    (LogicVal::X, _) | (_, LogicVal::X) => LogicVal::X,
                    (LogicVal::Z, v) => v,
                    (v, LogicVal::Z) => v,
                    (LogicVal::One, _) | (_, LogicVal::One) => LogicVal::One,
                    _ => LogicVal::Zero,
                }
            }
            NetType::Tri | NetType::Tri0 | NetType::Tri1 | NetType::Wire => {
                // Tri-state: exactly one non-Z driver wins; conflict = X
                match (current, incoming) {
                    (LogicVal::Z, v) => v,
                    (v, LogicVal::Z) => v,
                    (LogicVal::X, _) | (_, LogicVal::X) => LogicVal::X,
                    (LogicVal::Zero, LogicVal::One) | (LogicVal::One, LogicVal::Zero) => {
                        LogicVal::X
                    }
                    _ => current, // same value
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StructFieldInfo {
    pub name: Symbol,
    pub offset: usize,
    pub width: usize,
    /// Nama tipe field bila field adalah struct/typedef lain (nested struct
    /// access `a.b.c`). Diisi saat store_typedef_fields / deklarasi struct.
    /// Dipakai elaborator untuk resolve lvalue nested (`hw2reg.val.d = x`).
    pub type_name: Option<Symbol>,
    /// Field bertipe SIGNED (`int y;`, `byte b;`, `signed logic [7:0]`).
    /// Baca field harus signed (LRM 1800 §6.24.1) — tanpa ini
    /// `$display("%0d", s.y)` untuk `s.y = -5` tercetak 4294967291.
    pub is_signed: bool,
    /// Fields dari anonymous struct/union inline (`struct packed {...} data;`
    /// tanpa typedef). type_name tidak ada di typedef_field_map, jadi sub_fields
    /// menyimpan fields langsung agar chain `a.b.c` tetap bisa di-resolve
    /// (pola register file OpenTitan: `reg2hw.masked_out.data.qe = x`).
    pub sub_fields: Vec<StructFieldInfo>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SignalInfo {
    pub name: Symbol,
    pub width: usize,
    pub kind: SignalKind,
    pub net_type: NetType,
    pub multi_driver: bool,
    pub init_val: LogicVec,
    pub array_depth: usize,
    pub elem_width: usize,
    pub array_dims: Vec<usize>,
    pub class_name: Option<Symbol>,
    pub is_string: bool,
    pub is_real: bool,
    pub is_mailbox: bool,
    pub is_semaphore: bool,
    pub is_2state: bool,
    pub is_dynamic: bool,
    pub is_queue: bool,
    pub is_associative: bool,
    pub is_signed: bool,
    pub is_const: bool,
    pub msb: usize,
    pub lsb: usize,
    pub struct_fields: Vec<StructFieldInfo>,
    pub packed_dims: Vec<usize>,
    pub delay_rise: Option<u64>,
    pub delay_fall: Option<u64>,
    pub iface_type: Option<Symbol>,
    pub iface_modport: Option<Symbol>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum SignalKind {
    #[default]
    Wire,
    Reg,
    Logic,
    Input,
    Output,
    Inout,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrInstance {
    pub module_name: Symbol,
    pub instance_name: Symbol,
    pub port_map: std::sync::Arc<std::collections::HashMap<Symbol, SignalId>>,
    pub param_map: std::sync::Arc<std::collections::HashMap<Symbol, i64>>,
    pub type_param_map: std::sync::Arc<std::collections::HashMap<Symbol, usize>>,
    /// Posisi instance di source (untuk diagnostic).
    pub line: usize,
    pub col: usize,
    /// Atribut `(* ... *)` sebelum instance (EMULATOR.md §10 — anotasi Mivon
    /// `mivon_region`/`mivon_irq` diteruskan ke MHIR). Field TERAKHIR —
    /// cache bincode MICD lama gagal decode (EOF) → rebuild, bukan salah baca.
    pub attrs: Vec<mivon_ast::types::AttrEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SignalSensitivity {
    pub sig_id: SignalId,
    /// None = seluruh signal memicu; Some(msb,lsb) = hanya bila RANGE tsb berubah.
    pub msb: Option<usize>,
    pub lsb: Option<usize>,
}

impl SignalSensitivity {
    pub fn whole(sig_id: SignalId) -> Self {
        Self {
            sig_id,
            msb: None,
            lsb: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Process {
    Combinational {
        name: Symbol,
        sensitivity: Vec<SignalSensitivity>,
        body: Vec<IrStmt>,
    },
    CombReactive {
        name: Symbol,
        sensitivity: Vec<SignalSensitivity>,
        body: Vec<IrStmt>,
    },
    Sequential {
        name: Symbol,
        clock: ClockEdge,
        reset: Option<ResetInfo>,
        body: Vec<IrStmt>,
        /// LANG-27: guard `@(posedge clk iff (en))` — proses hanya dijalankan
        /// saat edge clock terjadi DAN kondisi iff bernilai true.
        iff: Option<IrExpr>,
    },
    Initial {
        name: Symbol,
        body: Vec<IrStmt>,
    },
    Final {
        name: Symbol,
        body: Vec<IrStmt>,
    },
    AlwaysWithDelay {
        name: Symbol,
        delay: u64,
        body: Vec<IrStmt>,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClockEdge {
    PosEdge(SignalId),
    NegEdge(SignalId),
    /// F27: clock hierarkis lewat port interface (`posedge b.clk`). Symbol
    /// adalah path `inst.field` yang di-resolve engine via `hier_signal_map`
    /// setelah flatten (port interface dibuat handle 64-bit, bukan signal
    /// clock nyata, jadi edge tidak bisa di-track via SignalId saat elaborasi).
    PosEdgeHier(Symbol),
    NegEdgeHier(Symbol),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResetInfo {
    pub signal: SignalId,
    pub polarity: bool,
    pub r#async: bool,
    pub value: LogicVec,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CaseType {
    Normal,
    CaseX,
    CaseZ,
    /// `case (x) inside` — label bisa berupa nilai tunggal (equality) atau
    /// rentang `[lo:hi]` (direpresentasikan sebagai IrExpr::InsideRange).
    Inside,
    /// LANG-17: `unique case` — warning bila 0 item cocok (tanpa default)
    /// ATAU 2+ item cocok.
    Unique,
    /// LANG-16: `unique0 case` — warning hanya bila 2+ item cocok.
    Unique0,
    /// LANG-17: `priority case` — warning bila 2+ item cocok (hanya item
    /// pertama yang dieksekusi, tanpa warning no-match).
    Priority,
    /// Kombinasi qualifier + kind (LRM 1800 §12.5, qualifier & kind
    /// ortogonal): pencocokan memakai semantik X/Z, qualifier untuk warning.
    /// Tanpa ini `priority casez` jatuh ke exact-match (bug vs iverilog).
    UniqueX,
    UniqueZ,
    Unique0X,
    Unique0Z,
    PriorityX,
    PriorityZ,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IrStmt {
    Block {
        stmts: Vec<IrStmt>,
    },
    NamedBlock {
        name: Symbol,
        stmts: Vec<IrStmt>,
        decls: Vec<mivon_ast::Decl>,
    },
    BlockingAssign {
        lhs: IrLValue,
        rhs: IrExpr,
        delay: Option<u64>,
    },
    NonBlockingAssign {
        lhs: IrLValue,
        rhs: IrExpr,
        delay: Option<u64>,
    },
    If {
        cond: IrExpr,
        true_branch: Vec<IrStmt>,
        false_branch: Vec<IrStmt>,
    },
    Case {
        case_type: CaseType,
        expr: IrExpr,
        items: Vec<IrCaseItem>,
        default: Vec<IrStmt>,
    },
    LoopFor {
        init: Option<Box<IrStmt>>,
        cond: IrExpr,
        step: Option<Box<IrStmt>>,
        body: Vec<IrStmt>,
    },
    LoopWhile {
        cond: IrExpr,
        body: Vec<IrStmt>,
    },
    LoopDoWhile {
        cond: IrExpr,
        body: Vec<IrStmt>,
    },
    Repeat {
        count: IrExpr,
        body: Vec<IrStmt>,
    },
    Foreach {
        array_var: IrExpr,
        index_var: Symbol,
        body: Vec<IrStmt>,
    },
    Delay {
        delay: u64,
        body: Vec<IrStmt>,
    },
    Force {
        lvalue: IrLValue,
        rhs: IrExpr,
    },
    Wait {
        cond: IrExpr,
        body: Vec<IrStmt>,
    },
    SysCall {
        name: Symbol,
        args: Vec<IrExpr>,
        /// Posisi source (`$name`) untuk diagnostic file:line:col (F20).
        line: usize,
        col: usize,
    },
    SysFinish,
    Null,
    EventControl {
        /// Daftar (signal, edge) sensitivity — `@(a or posedge b)`.
        /// edge None = level (tunggu perubahan nilai signal).
        sigs: Vec<(SignalId, Option<ClockEdge>)>,
        body: Vec<IrStmt>,
        /// LANG-27: guard `iff (cond)` pada event control — continuation
        /// hanya dilanjutkan bila kondisi benar saat event terpenuhi.
        iff: Option<IrExpr>,
    },
    EventTrigger {
        sig_id: SignalId,
    },
    MethodCallStmt {
        obj: IrExpr,
        method: Symbol,
        args: Vec<IrExpr>,
        with_clause: Option<Box<IrExpr>>,
    },
    Break,
    Continue,
    Disable {
        name: Symbol,
    },
    Release {
        lvalue: IrLValue,
    },
    Deassign {
        lvalue: IrLValue,
    },
    Fork {
        processes: Vec<Vec<IrStmt>>,
        join_type: IrJoinType,
    },
    Assert {
        cond: IrExpr,
        pass_stmt: Vec<IrStmt>,
        fail_stmt: Vec<IrStmt>,
        clock_event: Option<mivon_ast::types::ClockEvent>,
        disable_iff: Option<Box<IrExpr>>,
        sequence: Option<Box<IrSequence>>,
        /// Posisi source assertion utk diagnostic file:line:col (F20).
        line: usize,
        col: usize,
    },
    Assume {
        cond: IrExpr,
        pass_stmt: Vec<IrStmt>,
        fail_stmt: Vec<IrStmt>,
        clock_event: Option<mivon_ast::types::ClockEvent>,
        disable_iff: Option<Box<IrExpr>>,
        sequence: Option<Box<IrSequence>>,
        /// Posisi source assumption utk diagnostic file:line:col (F20).
        line: usize,
        col: usize,
    },
    Cover {
        cond: IrExpr,
        pass_stmt: Vec<IrStmt>,
        clock_event: Option<mivon_ast::types::ClockEvent>,
        disable_iff: Option<Box<IrExpr>>,
        sequence: Option<Box<IrSequence>>,
    },
    /// LANG-14: `expect (property_expr) else stmt` — assertion dalam
    /// procedural code (IEEE 1800-2017 §17.16.2). Subset: kondisi dievaluasi
    /// seketika saat statement dijangkau; false → fail_stmt + report
    /// "expect failed" (assertion immediate); true → pass_stmt. Tidak
    /// dipengaruhi $assertoff/$assertkill (berbeda dari assert immediate).
    Expect {
        cond: IrExpr,
        pass_stmt: Vec<IrStmt>,
        fail_stmt: Vec<IrStmt>,
        /// Posisi source expect utk diagnostic file:line:col (F20).
        line: usize,
        col: usize,
    },
    WaitOrder {
        events: Vec<SignalId>,
        failure_stmts: Vec<IrStmt>,
    },
    /// LANG-29: `wait fork;` — blokir sampai semua fork process milik proses
    /// ini selesai (IR dari `Stmt::WaitFork`).
    WaitFork,
    RandCase {
        items: Vec<(IrExpr, Vec<IrStmt>)>,
    },
    RandSequence {
        productions: Vec<(Symbol, Vec<(IrExpr, Vec<IrStmt>)>)>,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IrJoinType {
    Join,
    JoinAny,
    JoinNone,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrCaseItem {
    pub labels: Vec<IrExpr>,
    pub body: Vec<IrStmt>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IrLValue {
    Signal(SignalId, usize),
    RangeSelect(SignalId, usize, usize),
    BitSelect(SignalId, usize),
    ArrayIndex {
        sig_id: SignalId,
        index: Box<IrExpr>,
        elem_width: usize,
    },
    ArrayRangeSelect {
        sig_id: SignalId,
        index: Box<IrExpr>,
        elem_width: usize,
        msb: usize,
        lsb: usize,
    },
    ArrayBitSelect {
        sig_id: SignalId,
        index: Box<IrExpr>,
        elem_width: usize,
        /// Offset bit dalam elemen — bisa dinamis (`arr[i][j]` dengan j runtime,
        /// mis. `seeds_q[seed_idx][rd_idx]` di flash_ctrl_lcmgr) maupun
        /// konstanta (hasil static select).
        bit: Box<IrExpr>,
    },
    /// Dynamic indexed part-select lvalue: `sig[base +: width]` dengan base
    /// runtime (mis. `packed_data_d[word_sel*BusWidth +: BusWidth] = ...` di
    /// RTL OpenTitan). width di-resolve saat elaborasi (biasanya param/konstan).
    ExprPartSelect {
        sig_id: SignalId,
        base: Box<IrExpr>,
        width: usize,
    },
    /// Field class object: `obj.field` (obj = signal berisi handle object).
    ObjectField {
        sig_id: SignalId,
        field: Symbol,
    },
    /// Lvalue hierarkis yang belum ter-resolve saat elaborasi — nama signal
    /// di flatten list (mis. signal interface instance `sif.csb` atau path
    /// instance `u_dut.u_padring.cio_*`). Statement module di-elaborate
    /// SEBELUM flatten_instances, jadi nama hierarkis belum ada di
    /// signal_map/signals saat itu. Engine me-resolve nama ke flattened
    /// signal list saat write (mekanisme sama dengan `IrExpr::HierRef`).
    HierRef(Symbol),
    /// Seleksi bit/index pada lvalue hierarkis: `sif.sd_out[i]`. Index
    /// dievaluasi runtime; lebar elemen (bit vs word array) di-resolve engine
    /// dari SignalInfo flattened signal.
    HierRefIndex {
        name: Symbol,
        index: Box<IrExpr>,
    },
    Concat(Vec<IrLValue>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IrExpr {
    Const(LogicVec),
    FillLit(LogicVal),
    Signal(SignalId, usize),
    RangeSelect(SignalId, usize, usize),
    BitSelect(SignalId, usize),
    ExprRangeSelect(Box<IrExpr>, usize, usize),
    ExprBitSelect(Box<IrExpr>, usize),
    ExprPartSelect(Box<IrExpr>, Box<IrExpr>, Box<IrExpr>),
    ArrayIndex {
        sig_id: SignalId,
        index: Box<IrExpr>,
        elem_width: usize,
    },
    Concat(Vec<IrExpr>),
    Replicate(usize, Box<IrExpr>),
    UnaryOp(UnaryIrOp, Box<IrExpr>),
    BinaryOp(BinaryIrOp, Box<IrExpr>, Box<IrExpr>),
    Cond(Box<IrExpr>, Box<IrExpr>, Box<IrExpr>),
    Signed(Box<IrExpr>),
    String(String),
    SysFunc {
        name: Symbol,
        args: Vec<IrExpr>,
        /// Posisi source (`$name`) untuk diagnostic file:line:col (F20).
        line: usize,
        col: usize,
    },
    NewCall {
        class_name: Symbol,
        args: Vec<IrExpr>,
    },
    This,
    MethodCall {
        obj: Box<IrExpr>,
        method: Symbol,
        args: Vec<IrExpr>,
        with_clause: Option<Box<IrExpr>>,
    },
    MemberAccess {
        obj: Box<IrExpr>,
        field: Symbol,
    },
    DpiCall {
        name: Symbol,
        args: Vec<IrExpr>,
        return_width: usize,
    },
    HierRef(Symbol),
    Inside {
        expr: Box<IrExpr>,
        list: Vec<IrExpr>,
    },
    /// Range inside `{[lo:hi]}` — nilai dalam rentang inklusif. Dipakai
    /// runtime (engine) saat `inside {[a:b]}` dengan operan non-konstan.
    InsideRange {
        expr: Box<IrExpr>,
        lo: Box<IrExpr>,
        hi: Box<IrExpr>,
    },
    Cast {
        width: usize,
        expr: Box<IrExpr>,
    },
    StreamingConcat {
        op: String,
        slice_size: Option<usize>,
        slices: Vec<IrExpr>,
    },
    Dist {
        expr: Box<IrExpr>,
        items: Vec<IrDistItem>,
    },
    UdpLookup {
        udp_name: Symbol,
        args: Vec<IrExpr>,
    },
    /// Runtime function call (used for recursive functions that can't be inlined)
    FuncCall {
        func_name: Symbol,
        args: Vec<IrExpr>,
    },
    /// Virtual interface binding handle (instance name → binding value)
    VifBinding {
        instance_name: Symbol,
    },
    /// Virtual interface member access (resolved at runtime via bound instance)
    VirtualIfaceAccess {
        vif_name: Symbol,
        field: Symbol,
        field_width: usize,
    },
    /// Increment/decrement dengan side-effect dalam ekspresi (LRM 1800
    /// §11.4.1): `i++`, `++i`, `i--`, `--i`.
    /// `read` = nilai saat ini (rvalue operand), `lv` = target write-back
    /// nilai ±1. `pre` menentukan hasil (baru untuk prefix, lama untuk
    /// postfix). Evaluator menulis balik setelah membaca.
    IncDec {
        read: Box<IrExpr>,
        lv: Box<IrLValue>,
        /// true = `++`, false = `--`
        inc: bool,
        /// true = prefix (hasil nilai baru), false = postfix (hasil lama)
        pre: bool,
    },
    /// Konstanta real — bit-pattern f64 64-bit dengan PENANDA tipe real.
    /// Tanpa varian ini, literal real `1.5` dan integer 64-bit tak dapat
    /// dibedakan di IR (keduanya `Const` 64-bit) sehingga
    /// `$display("%f", 1.5)` salah baca (bit-pattern dianggap integer).
    /// Nilai bit identik dengan `Const` — pembedanya hanya metadata tipe.
    RealConst(LogicVec),
}

/// Temporal sequence expression for property evaluation
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IrSequence {
    /// Immediate Boolean expression (evaluated each cycle)
    Expr(IrExpr),
    /// ##N — wait N clock cycles
    Delay(u64),
    /// ##[min:max] — wait between min and max clock cycles
    DelayRange(u64, u64),
    /// seq1 ##1 seq2 — concatenation (first then second)
    Concat(Box<IrSequence>, Box<IrSequence>),
    /// seq1 or seq2 — either matches
    Or(Box<IrSequence>, Box<IrSequence>),
    /// seq1 and seq2 — both must match
    And(Box<IrSequence>, Box<IrSequence>),
    /// seq[*N] — repeat seq N times consecutively
    Repeat(Box<IrSequence>, u64),
    /// ante |-> cons — overlap implication: when ante matches on a cycle,
    /// cons is checked starting from the SAME cycle.
    Implication(Box<IrSequence>, Box<IrSequence>),
}

impl IrSequence {
    /// Estimate the minimum number of clock cycles this sequence needs to match
    pub fn min_cycles(&self) -> u64 {
        match self {
            IrSequence::Expr(_) => 0,
            IrSequence::Delay(n) => *n,
            IrSequence::DelayRange(min, _) => *min,
            IrSequence::Concat(a, b) => a.min_cycles() + b.min_cycles() + 1,
            IrSequence::Or(a, b) => a.min_cycles().min(b.min_cycles()),
            IrSequence::And(a, b) => a.min_cycles().max(b.min_cycles()),
            IrSequence::Repeat(seq, n) => seq.min_cycles() * n,
            IrSequence::Implication(ante, cons) => ante.min_cycles() + cons.min_cycles(),
        }
    }
    /// Estimate the maximum number of clock cycles before sequence is determined
    pub fn max_cycles(&self) -> Option<u64> {
        match self {
            IrSequence::Expr(_) => Some(0),
            IrSequence::Delay(n) => Some(*n),
            IrSequence::DelayRange(_, max) => Some(*max),
            IrSequence::Concat(a, b) => a
                .max_cycles()
                .and_then(|am| b.max_cycles().map(|bm| am + bm + 1)),
            IrSequence::Or(a, b) => a
                .max_cycles()
                .and_then(|am| b.max_cycles().map(|bm| am.max(bm))),
            IrSequence::And(a, b) => a
                .max_cycles()
                .and_then(|am| b.max_cycles().map(|bm| am.max(bm))),
            IrSequence::Repeat(seq, n) => seq.max_cycles().map(|m| m * n),
            IrSequence::Implication(ante, cons) => ante
                .max_cycles()
                .and_then(|am| cons.max_cycles().map(|cm| am + cm)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IrDistItem {
    pub range_lo: Option<i64>,
    pub range_hi: Option<i64>,
    pub weight_type: DistWeightType,
    pub weight: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DistWeightType {
    Item,
    Range,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UnaryIrOp {
    Plus,
    Minus,
    Not,
    BitNot,
    RedAnd,
    RedNand,
    RedOr,
    RedNor,
    RedXor,
    RedXnor,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BinaryIrOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Power,
    Eq,
    Neq,
    CaseEq,
    CaseNeq,
    EqWild,
    NeqWild,
    Lt,
    Le,
    Gt,
    Ge,
    BitAnd,
    BitOr,
    BitXor,
    BitXnor,
    Shl,
    Shr,
    Sshl,
    Sshr,
    LogicalAnd,
    LogicalOr,
}
