//! Predicate dan Requirement — kondisi dan keharusan aturan LRM.

#![cfg(feature = "dev")]

/// Requirement — apa yang harus dipenuhi implementasi bila aturan berlaku.
#[derive(Debug, Clone)]
pub enum Requirement {
    /// Sinyal harus mempunyai nilai tertentu di akhir simulasi.
    SignalValueEquals { signal: String, expected: String },

    /// Diagnostik dengan kode tertentu wajib diterbitkan.
    DiagnosticRequired {
        code: String,
        severity: DiagSeverity,
    },

    /// Diagnostik dengan kode tertentu tidak boleh diterbitkan.
    DiagnosticForbidden { code: String },

    /// Elaborasi harus berhasil (tidak boleh gagal).
    ElaborationSucceeds,

    /// Elaborasi harus gagal dengan kode tertentu.
    ElaborationFails { code: String },

    /// Properti semantik harus terpenuhi.
    SemanticProperty(SemanticProp),

    /// Jumlah iterasi generate harus sesuai nilai parameter.
    GenerateIterationsMatch {
        instance: String,
        expected_count: usize,
    },

    /// Diagnostik harus menyertakan lokasi file:line:col.
    DiagnosticHasLocation,
}

/// Hasil evaluasi satu `Requirement`.
///
/// Tiga-nilai (bukan `bool`): aturan yang BELUM diimplementasi harus jujur
/// `Inconclusive`, bukan `Satisfied` palsu. Tanpa ini hakim melaporkan
/// `Pass` untuk aturan yang tak pernah dievaluasi (temuan review: 25/27
/// aturan tak pernah bisa gagal karena catch-all `=> true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Requirement terpenuhi menurut observasi.
    Satisfied(String),
    /// Requirement dilanggar — kandidat bug kepatuhan.
    Violated(String),
    /// Tidak dapat dievaluasi (belum diimplementasi / data observasi kurang).
    Inconclusive(String),
}

impl CheckOutcome {
    pub fn is_satisfied(&self) -> bool {
        matches!(self, CheckOutcome::Satisfied(_))
    }

    pub fn is_violated(&self) -> bool {
        matches!(self, CheckOutcome::Violated(_))
    }

    pub fn reason(&self) -> &str {
        match self {
            CheckOutcome::Satisfied(r)
            | CheckOutcome::Violated(r)
            | CheckOutcome::Inconclusive(r) => r,
        }
    }
}

impl Requirement {
    /// Evaluasi requirement terhadap observasi mivon.
    pub fn check(&self, obs: &crate::observer::MivonObservation) -> CheckOutcome {
        match self {
            Requirement::DiagnosticHasLocation => {
                let all_have_loc = obs.diagnostics.iter()
                    .filter(|d| d.is_error())
                    .filter(|d| !is_summary_error(&d.message))
                    .all(|d| d.location.is_some());
                if all_have_loc {
                    CheckOutcome::Satisfied("semua diagnostik error mempunyai lokasi".to_string())
                } else {
                    let missing: Vec<_> = obs.diagnostics.iter()
                        .filter(|d| d.is_error() && d.location.is_none())
                        .filter(|d| !is_summary_error(&d.message))
                        .map(|d| d.message.clone())
                        .take(3)
                        .collect();
                    CheckOutcome::Violated(format!("diagnostik tanpa lokasi: {}", missing.join("; ")))
                }
            }
            Requirement::ElaborationSucceeds => {
                let elaboration_failed = obs.diagnostics.iter()
                    .any(|d| d.is_error() && d.code.starts_with('E'));
                if !elaboration_failed {
                    CheckOutcome::Satisfied("elaborasi berhasil".to_string())
                } else {
                    let codes: Vec<_> = obs.diagnostics.iter()
                        .filter(|d| d.is_error())
                        .map(|d| d.code.clone())
                        .take(3)
                        .collect();
                    CheckOutcome::Violated(format!("elaborasi gagal: {}", codes.join(", ")))
                }
            }
            Requirement::ElaborationFails { code } => {
                let found = obs.diagnostics.iter().any(|d| d.is_error() && d.code == *code);
                if found {
                    CheckOutcome::Satisfied(format!("elaborasi gagal dengan kode {}", code))
                } else {
                    CheckOutcome::Violated(format!(
                        "elaborasi diharapkan gagal dengan kode {} — tidak ditemukan",
                        code
                    ))
                }
            }
            Requirement::DiagnosticRequired { code, .. } => {
                let found = obs.diagnostics.iter().any(|d| d.code == *code);
                if found {
                    CheckOutcome::Satisfied(format!("diagnostik {} diterbitkan", code))
                } else {
                    CheckOutcome::Violated(format!("diagnostik {} tidak diterbitkan", code))
                }
            }
            Requirement::DiagnosticForbidden { code } => {
                let found = obs.diagnostics.iter().any(|d| d.code == *code);
                if !found {
                    CheckOutcome::Satisfied(format!("diagnostik {} tidak muncul (benar)", code))
                } else {
                    CheckOutcome::Violated(format!("diagnostik {} muncul (dilarang)", code))
                }
            }
            Requirement::SignalValueEquals { signal, expected } => {
                let actual = obs.values.iter()
                    .find(|v| v.signal == *signal)
                    .map(|v| v.value.display())
                    .unwrap_or_else(|| "<tidak ditemukan>".to_string());
                if actual == *expected {
                    CheckOutcome::Satisfied(format!("{} = {} (sesuai LRM)", signal, expected))
                } else {
                    CheckOutcome::Violated(format!("{}: expected={} observed={}", signal, expected, actual))
                }
            }
            // Properti semantik (NBA region, fill literal, concat width, dll.)
            // butuh trace/IR-level yang belum tersedia di observasi black-box.
            // Jujur Inconclusive — JANGAN dianggap satisfied (dulu catch-all
            // `=> true` membuat hakim selalu Pass untuk aturan ini).
            Requirement::SemanticProperty(prop) => CheckOutcome::Inconclusive(format!(
                "properti semantik {:?} belum dapat dievaluasi dari observasi black-box",
                prop
            )),
            Requirement::GenerateIterationsMatch { instance, expected_count } => {
                CheckOutcome::Inconclusive(format!(
                    "generate iterations utk '{}' (expected {}) belum dievaluasi — snapshot elaborasi belum diisi",
                    instance, expected_count
                ))
            }
        }
    }
}

/// Ringkasan agregat pasca-sim: bukan diagnostik berlokasi (lokasi ada di
/// tiap penyusunnya). Daftar sejajar `is_global_error` di oracle.
/// Dipakai `DiagnosticHasLocation` + evaluator syntax DIAG-001.
pub(crate) fn is_summary_error(msg: &str) -> bool {
    msg.contains("simulasi dihentikan") || msg.contains("assertion(s) gagal")
}

#[cfg(test)]
mod tests {
    use super::{is_summary_error, CheckOutcome, Requirement, SemanticProp};
    use crate::observer::MivonObservation;

    /// Summary $fatal/assert agregat bukan pelanggaran lokasi (bug_0227/0248).
    #[test]
    fn summary_errors_exempt_from_location() {
        assert!(is_summary_error("$fatal: simulasi dihentikan (1 fatal)"));
        assert!(is_summary_error("$fatal: simulasi dihentikan (1)"));
        assert!(is_summary_error("3 assertion(s) gagal"));
        assert!(!is_summary_error("assertion failed"));
        assert!(!is_summary_error("signal 'p' not found"));
    }

    /// Properti semantik yang belum dapat dievaluasi → Inconclusive, BUKAN
    /// Satisfied palsu (temuan review: catch-all `=> true` membuat hakim
    /// selalu Pass untuk ~25 aturan yang tak pernah dievaluasi).
    #[test]
    fn semantic_property_is_inconclusive_not_satisfied() {
        let obs = MivonObservation::default();
        for prop in [
            SemanticProp::NbaInNbaRegion,
            SemanticProp::FillLiteralExpandsToTargetWidth,
            SemanticProp::ConcatWidthIsSum,
            SemanticProp::SignedExtensionPreservesSign,
        ] {
            let out = Requirement::SemanticProperty(prop).check(&obs);
            assert!(
                matches!(out, CheckOutcome::Inconclusive(_)),
                "harus Inconclusive, dapat {out:?}"
            );
            assert!(!out.is_satisfied());
        }
    }

    /// Requirement yang terimplementasi tetap menghasilkan Satisfied/Violated.
    #[test]
    fn implemented_requirements_still_decide() {
        let obs = MivonObservation::default();
        assert!(Requirement::ElaborationSucceeds.check(&obs).is_satisfied());
        assert!(Requirement::DiagnosticHasLocation
            .check(&obs)
            .is_satisfied());
    }
}
#[derive(Debug, Clone)]
/// Properti semantik yang harus terpenuhi.
pub enum SemanticProp {
    /// Iterasi generate harus sesuai nilai parameter override.
    GenerateIterationsMatchParam,
    /// NBA assignment harus terjadi di region NBA, bukan Active.
    NbaInNbaRegion,
    /// Signed extension harus mempertahankan nilai negatif.
    SignedExtensionPreservesSign,
    /// Lebar ekspresi concat harus sama dengan jumlah lebar operand.
    ConcatWidthIsSum,
    /// Fill literal harus diexpand sesuai lebar target.
    FillLiteralExpandsToTargetWidth,
}

/// Severity diagnostik.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagSeverity {
    Error,
    Warning,
    Note,
}
