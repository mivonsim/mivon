//! Verdict — hasil keputusan LRM Judge.
//!
//! Hierarki:
//! - `Verdict`      — keputusan akhir per testcase
//! - `JudgeConfidence` — seberapa kuat bukti yang mendasari verdict
//! - `Evidence`     — bukti konkret yang menopang verdict
//! - `Provenance`   — ketertelusuran penuh ke klausul LRM

#![cfg(feature = "dev")]

pub mod confidence;
pub mod evidence;
pub mod provenance;

pub use confidence::JudgeConfidence;
pub use evidence::{Evidence, FailureKind};
pub use provenance::{ApplicabilityProof, Provenance};

use crate::lrm_model::{ClauseRef, RuleId};

/// Keputusan akhir hakim atas satu testcase.
///
/// Perbedaan Mivon vs expected TIDAK otomatis = Violation.
/// `RuleClassification` di LrmRule yang menentukan apakah perbedaan
/// adalah bug, undefined behavior, atau implementation-defined.
#[derive(Debug, Clone)]
pub enum Verdict {
    /// Mivon sesuai LRM untuk testcase ini.
    Pass,

    /// Mivon melanggar aturan LRM yang wajib (RequiredBehavior).
    Violation {
        rule: RuleId,
        clause: ClauseRef,
        expected: String,
        observed: String,
    },

    /// Testcase tidak valid — tidak dapat menguji aturan yang dimaksud.
    InvalidTest { reason: String },

    /// LRM tidak mendefinisikan perilaku untuk kasus ini.
    /// Mivon boleh berperilaku apa saja — bukan bug.
    UndefinedByLrm {
        rule: RuleId,
        clause: ClauseRef,
    },

    /// LRM memberi kebebasan ke implementasi.
    /// Mivon tidak salah meski berbeda dari simulator lain.
    ImplementationDependent {
        rule: RuleId,
        clause: ClauseRef,
    },

    /// Klausul LRM ambigu. Membutuhkan interpretasi manusia.
    LrmAmbiguous {
        clause: ClauseRef,
        interpretations: Vec<String>,
    },

    /// Judge tidak cukup informasi untuk memberi keputusan.
    JudgeInsufficient {
        reason: String,
        missing: Vec<String>,
    },

    /// Mivon crash/panic/hang — bukan masalah conformance tapi tetap bug.
    MivonInternalFailure {
        kind: FailureKind,
        detail: String,
    },
}

impl Verdict {
    /// Apakah verdict ini merupakan bug yang harus disimpan.
    ///
    /// `Violation` + `Insufficient` TIDAK langsung disimpan — hanya bila
    /// confidence >= `Strong`. Logika confidence ada di `JudgeReport`.
    pub fn is_bug(&self) -> bool {
        matches!(
            self,
            Verdict::Violation { .. } | Verdict::MivonInternalFailure { .. }
        )
    }

    /// Label singkat untuk display dan dedup.
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Violation { .. } => "violation",
            Verdict::InvalidTest { .. } => "invalid_test",
            Verdict::UndefinedByLrm { .. } => "lrm_undefined",
            Verdict::ImplementationDependent { .. } => "impl_dep",
            Verdict::LrmAmbiguous { .. } => "lrm_ambiguous",
            Verdict::JudgeInsufficient { .. } => "judge_insufficient",
            Verdict::MivonInternalFailure { .. } => "mivon_failure",
        }
    }
}

/// Laporan lengkap dari satu sesi judge.
#[derive(Debug, Clone)]
pub struct JudgeReport {
    pub testcase_id: String,
    pub verdict: Verdict,
    pub confidence: JudgeConfidence,
    pub rule_results: Vec<RuleResult>,
    pub evidence: Vec<Evidence>,
    pub provenance: Option<Provenance>,
}

impl JudgeReport {
    /// Apakah laporan ini harus disimpan sebagai bug.
    /// Violation + Insufficient TIDAK disimpan.
    pub fn should_save(&self) -> bool {
        self.verdict.is_bug()
            && !matches!(self.confidence, JudgeConfidence::Insufficient)
    }
}

/// Hasil evaluasi satu aturan LRM.
#[derive(Debug, Clone)]
pub struct RuleResult {
    pub rule: RuleId,
    pub verdict: RuleVerdict,
    pub evidence: Vec<Evidence>,
    pub explanation: String,
}

/// Verdict per aturan (sebelum diagregasi ke verdict testcase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleVerdict {
    Satisfied,
    Violated { reason: String },
    NotApplicable,
    Inconclusive { reason: String },
}
