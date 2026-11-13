//! Payload artefak pipeline per kategori (dibaca/ditulis tool: `melab`,
//! `minspect`, `mcov`, `mlint`, `msim`).
//!
//! Payload ini adalah artefak turunan, bukan penentu cache hit. Validitas
//! kompilasi tetap milik node content-addressed; kategori di sini menyimpan
//! ringkasan agar tool bisa membaca hasil build sebelumnya.

use serde::{Deserialize, Serialize};

use crate::entries::{CheckResult, VerifyCheckKind};

/// Hasil verifikasi per kategori.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyPayload {
    pub parse_ok: bool,
    pub elab_ok: bool,
    pub err_count: usize,
    pub warn_count: usize,
    pub info_count: usize,
    pub checks: Vec<(VerifyCheckKind, CheckResult)>,
}

/// Satu instance hasil elaborasi.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElabInstance {
    pub module: String,
    pub instance: String,
    pub port_bindings: usize,
    pub param_overrides: Vec<(String, i64)>,
    pub line: usize,
    pub col: usize,
}

/// Jumlah proses per module.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessCounts {
    pub combinational: usize,
    pub comb_reactive: usize,
    pub sequential: usize,
    pub initial: usize,
    pub final_: usize,
    pub always_with_delay: usize,
}

/// Jumlah signal per jenis net.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetCounts {
    pub wire: usize,
    pub wand: usize,
    pub wor: usize,
    pub tri: usize,
    pub triand: usize,
    pub trior: usize,
    pub supply0: usize,
    pub supply1: usize,
}

/// Ringkasan hasil elaborasi satu module.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ElaboratePayload {
    pub instance_count: usize,
    pub instances: Vec<ElabInstance>,
    pub processes: ProcessCounts,
    pub net_counts: NetCounts,
}

/// Ringkasan blok generate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeneratePayload {
    pub if_blocks: usize,
    pub for_blocks: usize,
    pub case_blocks: usize,
    pub expanded_instances: usize,
}

/// Satu temuan lint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LintFinding {
    pub module: String,
    pub check: String,
    pub severity: String,
    pub message: String,
}

/// Ringkasan temuan lint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LintPayload {
    pub findings: Vec<LintFinding>,
}

/// Ringkasan coverage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoveragePayload {
    pub line_items: u64,
    pub line_hits: u64,
    pub branch_total: u64,
    pub branch_covered: u64,
    pub toggle_signals: u64,
    pub toggle_transitions: u64,
    pub fsm_signals: u64,
    pub fsm_states: u64,
}

/// Ringkasan simulasi.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SimulationPayload {
    pub end_time: u64,
    pub events_processed: u64,
    pub signal_count: usize,
    pub init_signals: usize,
    pub processes: ProcessCounts,
}

/// Satu signal waveform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveSignal {
    pub name: String,
    pub width: usize,
    pub kind: String,
    pub net: String,
    pub is_signed: bool,
}

/// Ringkasan waveform.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WaveformPayload {
    pub signals: Vec<WaveSignal>,
}

/// Statistik optimasi elaborator.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OptimizePayload {
    pub const_folds: usize,
    pub loop_unrolls: usize,
    pub unrolled_stmts: usize,
}

/// Statistik evaluasi ekspresi.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExpressionPayload {
    pub expr_evals: usize,
    pub samples: Vec<(String, i64)>,
}
