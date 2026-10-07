//! LrmState — state design yang dievaluasi hakim LRM.
//!
//! Hakim tidak melihat stdout mentah. Hakim melihat state terstruktur
//! yang dibangun dari IrDesign hasil elaborasi mivon.

#![cfg(feature = "dev")]

use std::collections::HashMap;

/// State design lengkap yang dievaluasi hakim.
#[derive(Debug, Clone, Default)]
pub struct LrmState {
    pub scopes: ScopeGraph,
    pub symbols: SymbolTable,
    pub types: TypeSystem,
    pub elaborated_design: DesignModel,
    pub processes: ProcessModel,
    pub scheduler: SchedulerModel,
}

// ── Scope ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ScopeGraph {
    pub root: ScopeId,
    pub nodes: HashMap<ScopeId, ScopeNode>,
    pub parent: HashMap<ScopeId, ScopeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ScopeId(pub String);

#[derive(Debug, Clone)]
pub struct ScopeNode {
    pub id: ScopeId,
    pub kind: ScopeKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Module,
    Interface,
    Package,
    Function,
    Task,
    Block,
    Generate,
    Class,
}

// ── Symbol ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    pub entries: HashMap<SymbolId, SymbolEntry>,
    pub by_name: HashMap<(ScopeId, String), SymbolId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SymbolId(pub String);

#[derive(Debug, Clone)]
pub struct SymbolEntry {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub scope: ScopeId,
    pub width: u32,
    pub is_signed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Net,
    Variable,
    Parameter,
    Localparam,
    Port,
    EnumMember,
    TypeDef,
}

// ── Type ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct TypeSystem {
    pub types: HashMap<TypeId, SvType>,
    pub bindings: HashMap<SymbolId, TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TypeId(pub String);

#[derive(Debug, Clone)]
pub struct SvType {
    pub id: TypeId,
    pub kind: SvTypeKind,
    pub width: u32,
    pub is_signed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SvTypeKind {
    Logic,
    Bit,
    Integer,
    Int,
    LongInt,
    ShortInt,
    Byte,
    Real,
    String,
    PackedArray { elem_width: u32, size: u32 },
    UnpackedArray { elem_type: TypeId, size: usize },
    Struct { fields: Vec<(String, TypeId)> },
    Enum { base: TypeId, members: Vec<String> },
    UserDefined { name: String },
}

// ── Design ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DesignModel {
    pub hierarchy: Vec<HierarchyNode>,
    /// key: "instance_path.param_name"
    pub parameters: HashMap<String, ParameterBinding>,
    pub generates: Vec<GenerateBlock>,
    pub port_bindings: Vec<PortBinding>,
}

#[derive(Debug, Clone)]
pub struct HierarchyNode {
    pub instance_path: String,
    pub module_name: String,
    pub children: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ParameterBinding {
    pub instance_path: String,
    pub param_name: String,
    pub default_value: String,
    pub override_value: Option<String>,
    pub is_overridden: bool,
}

#[derive(Debug, Clone)]
pub struct GenerateBlock {
    pub instance_path: String,
    pub kind: GenerateKind,
    pub iterations: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateKind {
    For,
    If,
    Case,
}

#[derive(Debug, Clone)]
pub struct PortBinding {
    pub instance_path: String,
    pub port_name: String,
    pub connected_signal: String,
    pub direction: PortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    Input,
    Output,
    Inout,
}

// ── Process ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ProcessModel {
    pub always_blocks: Vec<AlwaysProcess>,
    pub initial_blocks: Vec<InitialProcess>,
    pub final_blocks: Vec<FinalProcess>,
}

#[derive(Debug, Clone)]
pub struct AlwaysProcess {
    pub id: String,
    pub kind: AlwaysKind,
    pub sensitivity: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlwaysKind {
    Plain,
    Comb,
    Ff,
    Latch,
}

#[derive(Debug, Clone)]
pub struct InitialProcess {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct FinalProcess {
    pub id: String,
}

// ── Scheduler ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct SchedulerModel {
    pub regions: Vec<SchedulerRegionRecord>,
    pub time_steps: Vec<TimeStep>,
}

/// Region scheduler LRM §4.4: Active, NBA, Observed, Reactive, Postponed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerRegion {
    Active,
    Nba,
    Observed,
    Reactive,
    Postponed,
}

impl SchedulerRegion {
    pub fn label(self) -> &'static str {
        match self {
            SchedulerRegion::Active => "Active",
            SchedulerRegion::Nba => "NBA",
            SchedulerRegion::Observed => "Observed",
            SchedulerRegion::Reactive => "Reactive",
            SchedulerRegion::Postponed => "Postponed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedulerRegionRecord {
    pub region: SchedulerRegion,
    pub event_count: usize,
}

#[derive(Debug, Clone)]
pub struct TimeStep {
    pub time: u64,
    pub regions_fired: Vec<SchedulerRegion>,
}
