//! Mid-Level IR — simulation-optimized intermediate representation.
//!
//! MIR adalah lower dari HIR, dioptimasi untuk simulasi event-driven.
//! Signal dialokasikan dalam SoA layout, instructions di-flatten.

use std::collections::HashMap;

use mivon_core::intern::Symbol;

// ─── MIR Types ───

/// MIR signal — flattened, width-resolved.
#[derive(Debug, Clone)]
pub struct MirSignal {
    pub name: Symbol,
    pub width: usize,
    pub kind: MirSignalKind,
    pub initial_value: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirSignalKind {
    Wire,
    Reg,
    Logic,
    Input,
    Output,
    Inout,
    Parameter,
    Localparam,
    /// Clock sinyal (digunakan oleh AlwaysClk/AlwaysFF sensitivity).
    Clock,
}

/// MIR instruction — flat, linear for fast interpretation.
#[derive(Debug, Clone)]
pub enum MirInstr {
    /// Load constant
    Const {
        dest: usize,
        value: u64,
        width: usize,
    },
    /// Load signal value
    Load { dest: usize, signal: usize },
    /// Store to signal
    Store { signal: usize, src: usize },
    /// Binary operation
    Binary {
        op: MirBinOp,
        dest: usize,
        lhs: usize,
        rhs: usize,
        width: usize,
    },
    /// Unary operation
    Unary {
        op: MirUnOp,
        dest: usize,
        operand: usize,
        width: usize,
    },
    /// Conditional branch
    Branch {
        cond: usize,
        then_label: usize,
        else_label: usize,
    },
    /// Unconditional jump
    Jump { label: usize },
    /// Label target
    Label(usize),
    /// Non-blocking assignment (scheduled)
    NonBlocking {
        signal: usize,
        src: usize,
        delay: Option<u64>,
    },
    /// Display/debug
    Display { args: Vec<MirDisplayArg> },
    /// Finish simulation
    Finish,
    /// Nop
    Nop,
}

#[derive(Debug, Clone)]
pub enum MirDisplayArg {
    Signal(usize),
    Str(String),
    Format(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirUnOp {
    Not,
    Neg,
}

/// MIR process — collection of instructions for a block.
#[derive(Debug, Clone)]
pub struct MirProcess {
    pub name: Symbol,
    pub sensitivity: MirSensitivity,
    pub instrs: Vec<MirInstr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirSensitivity {
    AlwaysComb,
    AlwaysClk { signal: usize, edge: MirEdge },
    AlwaysFF { signal: usize, edge: MirEdge },
    Initial,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirEdge {
    Posedge,
    Negedge,
}

// ─── MIR Module ───

/// MIR module — flattened, simulation-ready.
#[derive(Debug, Clone)]
pub struct MirModule {
    pub name: Symbol,
    pub signals: Vec<MirSignal>,
    pub processes: Vec<MirProcess>,
    pub signal_map: HashMap<Symbol, usize>,
}

impl MirModule {
    pub fn new(name: Symbol) -> Self {
        MirModule {
            name,
            signals: Vec::new(),
            processes: Vec::new(),
            signal_map: HashMap::new(),
        }
    }

    /// Add a signal and return its index.
    pub fn add_signal(&mut self, signal: MirSignal) -> usize {
        let idx = self.signals.len();
        self.signal_map.insert(signal.name, idx);
        self.signals.push(signal);
        idx
    }

    /// Look up signal index by name.
    pub fn signal_index(&self, name: Symbol) -> Option<usize> {
        self.signal_map.get(&name).copied()
    }

    /// Total signal width (memory needed).
    pub fn total_width(&self) -> usize {
        self.signals.iter().map(|s| s.width).sum()
    }
}

// ─── MIR Design ───

/// Complete MIR design for simulation.
#[derive(Debug, Clone)]
pub struct MirDesign {
    pub modules: HashMap<Symbol, MirModule>,
    pub top: MirModule,
}
