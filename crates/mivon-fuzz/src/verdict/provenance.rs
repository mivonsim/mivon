//! Provenance — ketertelusuran penuh verdict ke klausul LRM.
//!
//! Setiap verdict harus dapat ditelusuri:
//!
//! ```text
//! VERDICT
//!    │
//!    ├── Rule (RuleId)
//!    │    └── IEEE 1800 §clause
//!    │
//!    ├── Applicability proof
//!    │    └── applies_when(&state) = true
//!    │
//!    ├── Expected behavior
//!    │    └── LRM mewajibkan X
//!    │
//!    ├── Mivon observation
//!    │    └── Mivon menghasilkan Y
//!    │
//!    └── Comparison
//!         └── X ≠ Y → VIOLATION
//! ```

#![cfg(feature = "dev")]

use crate::lrm_model::{ClauseRef, RuleId};

/// Ketertelusuran penuh satu verdict.
#[derive(Debug, Clone)]
pub struct Provenance {
    pub rule: RuleId,
    pub clause: ClauseRef,
    /// Bukti bahwa aturan berlaku untuk testcase ini.
    pub applicability: ApplicabilityProof,
    /// Deskripsi perilaku yang diharapkan LRM.
    pub expected: String,
    /// Deskripsi perilaku yang diobservasi dari Mivon.
    pub observed: String,
}

impl Provenance {
    /// Format provenance menjadi string untuk output bug report.
    pub fn format(&self) -> String {
        let cond_lines: String = self
            .applicability
            .conditions
            .iter()
            .map(|(desc, val)| format!("      - {} → {}\n", desc, val))
            .collect();

        format!(
            "Provenance:\n\
                 Rule:       {}\n\
                 Clause:     IEEE 1800-{} §{}\n\
                 Applicable: {}\n\
             {}\
                 Expected:   {}\n\
                 Observed:   {}\n",
            self.rule.0,
            self.clause.revision,
            self.clause.section,
            if self.applicability.applies { "YES" } else { "NO" },
            cond_lines,
            self.expected,
            self.observed,
        )
    }
}

/// Bukti bahwa aturan berlaku untuk testcase ini.
#[derive(Debug, Clone)]
pub struct ApplicabilityProof {
    /// Setiap kondisi applicability beserta hasil evaluasinya.
    pub conditions: Vec<(String, bool)>,
    /// true jika semua kondisi terpenuhi.
    pub applies: bool,
}

impl ApplicabilityProof {
    pub fn new(conditions: Vec<(&'static str, bool)>) -> Self {
        let applies = conditions.iter().all(|(_, v)| *v);
        Self {
            conditions: conditions
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            applies,
        }
    }

    pub fn always() -> Self {
        Self {
            conditions: vec![("unconditional".to_string(), true)],
            applies: true,
        }
    }
}
