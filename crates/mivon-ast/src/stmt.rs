use super::expr::Expr;
use mivon_core::intern::Symbol;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AlwaysBlock {
    pub kind: AlwaysKind,
    pub sensitivity: Option<SensitivityList>,
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InitialBlock {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AlwaysKind {
    Always,
    AlwaysComb,
    AlwaysFF,
    AlwaysLatch,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SensitivityList {
    pub events: Vec<SensitivityEvent>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SensitivityEvent {
    PosEdge(Expr),
    NegEdge(Expr),
    Level(Expr),
    Wildcard,
    /// `@(posedge clk iff (en))` — event hanya dianggap terjadi bila cond benar.
    /// Menyimpan event asli + kondisi guard. (LANG-27)
    Iff {
        event: Box<SensitivityEvent>,
        cond: Expr,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Stmt {
    Block {
        stmts: Vec<Stmt>,
    },
    NamedBlock {
        name: Symbol,
        stmts: Vec<Stmt>,
        decls: Vec<super::types::Decl>,
    },
    IfElse {
        cond: Expr,
        true_branch: Box<Stmt>,
        false_branch: Option<Box<Stmt>>,
    },
    Case {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
    },
    CaseX {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
    },
    CaseZ {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
    },
    StmtCase {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
    },
    LoopForever {
        stmts: Vec<Stmt>,
    },
    LoopWhile {
        cond: Expr,
        stmts: Vec<Stmt>,
    },
    DoWhile {
        cond: Expr,
        stmts: Vec<Stmt>,
    },
    LoopFor {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Box<Stmt>>,
        stmts: Vec<Stmt>,
    },
    Repeat {
        count: Expr,
        stmts: Vec<Stmt>,
    },
    BlockingAssign {
        lhs: Expr,
        rhs: Expr,
        delay: Option<super::types::Delay>,
    },
    NonBlockingAssign {
        lhs: Expr,
        rhs: Expr,
        delay: Option<super::types::Delay>,
    },
    StmtAssign {
        lhs: Expr,
        rhs: Expr,
    },
    Expr {
        expr: Expr,
    },
    SysCall {
        name: Symbol,
        args: Vec<Expr>,
        /// Posisi source (`$name`) untuk diagnostic file:line:col (F20).
        line: usize,
        col: usize,
    },
    SysFinish,
    Delay {
        delay: Expr,
        stmt: Box<Stmt>,
    },
    Wait {
        cond: Expr,
        stmt: Option<Box<Stmt>>,
    },
    /// LANG-29: `wait fork;` — blokir sampai SEMUA fork process milik proses
    /// ini selesai (IEEE 1800-2017 9.7.2). Fieldless: parser memproduksi
    /// varian ini untuk `wait fork` (berbeda dari `wait (expr)` yang
    /// menggunakan `Wait { cond, stmt }`).
    WaitFork,
    Disable {
        name: Symbol,
    },
    Force {
        lhs: Expr,
        rhs: Expr,
    },
    Release {
        expr: Expr,
    },
    Deassign {
        expr: Expr,
    },
    Break,
    Continue,
    Return(Option<Box<Expr>>),
    Null,
    EventControl {
        events: Vec<SensitivityEvent>,
        stmt: Option<Box<Stmt>>,
    },
    EventTrigger {
        name: Symbol,
    },
    ForeachLoop {
        array_var: Symbol,
        index_vars: Vec<Symbol>,
        stmts: Vec<Stmt>,
    },
    // Unique/Priority case qualifiers. `kind` = jenis case yang
    // di-qualify (`case`/`casex`/`casez`) — qualifier & kind ortogonal
    // (LRM 1800 §12.5): qualifier mengatur overlap-check, kind mengatur
    // pencocokan wildcard. Tanpa `kind`, `priority casez` kehilangan
    // semantik wildcard-nya (bug: hanya exact-match).
    UniqueCase {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
        kind: CaseKind,
    },
    PriorityCase {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
        kind: CaseKind,
    },
    /// LANG-16/17: `unique0 case` — warning hanya bila ada 2+ item cocok
    /// (tanpa warning no-match, beda dengan `unique case`).
    Unique0Case {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
        kind: CaseKind,
    },
    CaseInside {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Option<Box<Stmt>>,
    },
    // Immediate/concurrent assertions
    Assert {
        cond: Expr,
        pass_stmt: Option<Box<Stmt>>,
        fail_stmt: Option<Box<Stmt>>,
        clock_event: Option<super::types::ClockEvent>,
        disable_iff: Option<Box<Expr>>,
    },
    Assume {
        cond: Expr,
        pass_stmt: Option<Box<Stmt>>,
        fail_stmt: Option<Box<Stmt>>,
        clock_event: Option<super::types::ClockEvent>,
        disable_iff: Option<Box<Expr>>,
    },
    Cover {
        cond: Expr,
        pass_stmt: Option<Box<Stmt>>,
        clock_event: Option<super::types::ClockEvent>,
        disable_iff: Option<Box<Expr>>,
    },
    Expect {
        cond: Expr,
        pass_stmt: Option<Box<Stmt>>,
        fail_stmt: Option<Box<Stmt>>,
    },
    /// SVA concurrent assertion dengan sequence temporal (LANG-06):
    /// `assert property (@(posedge clk) a ##1 b);` — properti diwakili
    /// sequence (`##N`, concat, or/and, repeat), bukan ekspresi boolean
    /// tunggal. Engine memulai SequenceAttempt tiap clock edge dan
    /// menyelesaikannya saat sequence cocok / melebihi max cycles.
    PropertySeq {
        sequence: super::types::Sequence,
        pass_stmt: Option<Box<Stmt>>,
        fail_stmt: Option<Box<Stmt>>,
        clock_event: Option<super::types::ClockEvent>,
        disable_iff: Option<Box<Expr>>,
    },
    WaitOrder {
        events: Vec<Symbol>,
        fail_stmt: Option<Box<Stmt>>,
    },
    /// Unique/priority if
    UniqueIf {
        cond: Expr,
        true_branch: Box<Stmt>,
        false_branch: Option<Box<Stmt>>,
    },
    PriorityIf {
        cond: Expr,
        true_branch: Box<Stmt>,
        false_branch: Option<Box<Stmt>>,
    },
    Fork {
        processes: Vec<Stmt>,
        join_type: JoinType,
    },
    RandCase {
        items: Vec<RandCaseItem>,
    },
    RandSequence {
        productions: Vec<RandSeqProduction>,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RandSeqProduction {
    pub name: Symbol,
    pub items: Vec<RandSeqItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RandSeqItem {
    pub value: Box<Stmt>,
    pub weight: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RandCaseItem {
    pub weight: u64,
    pub stmt: Box<Stmt>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum JoinType {
    Join,
    JoinAny,
    JoinNone,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CaseItem {
    pub labels: Vec<Expr>,
    pub stmt: Box<Stmt>,
}

/// Jenis case yang di-qualify `unique`/`unique0`/`priority`
/// (LRM 1800 §12.5) — qualifier & kind ortogonal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CaseKind {
    /// `unique case` / `priority case` — exact match
    Plain,
    /// `unique casex` / `priority casex` — X/Z/? wildcard
    X,
    /// `unique casez` / `priority casez` — Z/? wildcard
    Z,
}
