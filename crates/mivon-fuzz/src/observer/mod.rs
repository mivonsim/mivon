//! observer — instrumentasi mivon → MivonObservation terstruktur.
//!
//! Observer mengubah output mivon yang tidak terstruktur menjadi
//! `MivonObservation` yang dapat dievaluasi oleh lrm_judge.
//!
//! Pipeline:
//! ```text
//! parse → AST → semantic → elaboration → process → scheduling → final state
//!                              ↓               ↓           ↓          ↓
//!                   ElaborationSnapshot  SchedulingObs  ValueObs  RuntimeTrace
//! ```

#![cfg(feature = "dev")]

pub mod collector;
pub mod state;

pub use collector::collect_from_runner;
pub use state::build_lrm_state;

use std::collections::HashMap;
use crate::lrm_model::LrmValue;

/// Observasi lengkap dari satu eksekusi mivon.
///
/// Hakim tidak melihat stdout/stderr mentah — ia melihat struct ini.
#[derive(Debug, Clone)]
pub struct MivonObservation {
    pub exit_status: ExitStatus,
    pub diagnostics: Vec<ObsDiagnostic>,
    pub elaboration: Option<ElaborationSnapshot>,
    pub runtime: Option<RuntimeTrace>,
    pub values: Vec<ValueObservation>,
    pub scheduling: Vec<SchedulingObservation>,
    /// stdout/stderr mentah — untuk secondary evidence dan debug.
    pub raw_stdout: String,
    pub raw_stderr: String,
}

impl Default for MivonObservation {
    fn default() -> Self {
        Self {
            exit_status: ExitStatus::Ok,
            diagnostics: Vec::new(),
            elaboration: None,
            runtime: None,
            values: Vec::new(),
            scheduling: Vec::new(),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
        }
    }
}

impl MivonObservation {
    /// Apakah mivon mengalami kegagalan internal (panic/hang/crash).
    pub fn is_internal_failure(&self) -> bool {
        matches!(
            self.exit_status,
            ExitStatus::Panic | ExitStatus::Hang | ExitStatus::Crash { .. } | ExitStatus::Abort
        )
    }

    /// Apakah ada error diagnostik.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.is_error())
    }

    /// Semua kode error.
    pub fn error_codes(&self) -> Vec<&str> {
        self.diagnostics.iter()
            .filter(|d| d.is_error())
            .map(|d| d.code.as_str())
            .collect()
    }
}

// ── Exit Status ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatus {
    Ok,
    CleanError,
    Panic,
    Hang,
    Crash { code: i32 },
    Abort,
}

impl ExitStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ExitStatus::Ok => "ok",
            ExitStatus::CleanError => "clean_error",
            ExitStatus::Panic => "panic",
            ExitStatus::Hang => "hang",
            ExitStatus::Crash { .. } => "crash",
            ExitStatus::Abort => "abort",
        }
    }
}

// ── Diagnostik ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ObsDiagnostic {
    pub severity: ObsSeverity,
    /// Kode diagnostik mivon — contoh: "E1002", "WR0102", "EL3001"
    pub code: String,
    pub message: String,
    pub location: Option<ObsLocation>,
}

impl ObsDiagnostic {
    pub fn is_error(&self) -> bool {
        matches!(self.severity, ObsSeverity::Error | ObsSeverity::Fatal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsSeverity {
    Error,
    Warning,
    Note,
    Fatal,
}

#[derive(Debug, Clone)]
pub struct ObsLocation {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

impl ObsLocation {
    /// Format "file:line:col"
    pub fn display(&self) -> String {
        format!("{}:{}:{}", self.file, self.line, self.col)
    }
}

// ── Elaboration Snapshot ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ElaborationSnapshot {
    pub hierarchy: Vec<ObsHierarchyNode>,
    pub parameters: Vec<ObsParameterBinding>,
    pub signals: Vec<ObsSignalDecl>,
    pub generate_expansions: Vec<ObsGenerateExpansion>,
    pub port_bindings: Vec<ObsPortBinding>,
}

#[derive(Debug, Clone)]
pub struct ObsHierarchyNode {
    pub instance_path: String,
    pub module_name: String,
}

#[derive(Debug, Clone)]
pub struct ObsParameterBinding {
    pub instance_path: String,
    pub param_name: String,
    pub value: String,
    pub is_overridden: bool,
}

#[derive(Debug, Clone)]
pub struct ObsSignalDecl {
    pub name: String,
    pub width: u32,
    pub is_signed: bool,
}

#[derive(Debug, Clone)]
pub struct ObsGenerateExpansion {
    pub instance_path: String,
    pub kind: String,
    pub iterations: usize,
}

#[derive(Debug, Clone)]
pub struct ObsPortBinding {
    pub instance_path: String,
    pub port_name: String,
    pub connected_signal: String,
}

// ── Runtime Trace ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RuntimeTrace {
    pub time_steps: Vec<ObsTimeStep>,
    pub final_values: HashMap<String, LrmValue>,
}

#[derive(Debug, Clone)]
pub struct ObsTimeStep {
    pub time: u64,
    pub region: String,
    pub assignments: Vec<ObsAssignment>,
}

#[derive(Debug, Clone)]
pub struct ObsAssignment {
    pub signal: String,
    pub old_value: LrmValue,
    pub new_value: LrmValue,
    pub is_nba: bool,
}

// ── Value Observation ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ValueObservation {
    pub signal: String,
    pub time: u64,
    pub value: LrmValue,
}

// ── Scheduling Observation ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SchedulingObservation {
    pub time: u64,
    pub region: String,
    pub event_kind: String,
    pub signal: Option<String>,
}
