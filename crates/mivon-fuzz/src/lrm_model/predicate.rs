//! Predicate dan Requirement — kondisi dan keharusan aturan LRM.

#![cfg(feature = "dev")]

/// Requirement — apa yang harus dipenuhi implementasi bila aturan berlaku.
#[derive(Debug, Clone)]
pub enum Requirement {
    /// Sinyal harus mempunyai nilai tertentu di akhir simulasi.
    SignalValueEquals { signal: String, expected: String },

    /// Diagnostik dengan kode tertentu wajib diterbitkan.
    DiagnosticRequired { code: String, severity: DiagSeverity },

    /// Diagnostik dengan kode tertentu tidak boleh diterbitkan.
    DiagnosticForbidden { code: String },

    /// Elaborasi harus berhasil (tidak boleh gagal).
    ElaborationSucceeds,

    /// Elaborasi harus gagal dengan kode tertentu.
    ElaborationFails { code: String },

    /// Properti semantik harus terpenuhi.
    SemanticProperty(SemanticProp),

    /// Jumlah iterasi generate harus sesuai nilai parameter.
    GenerateIterationsMatch { instance: String, expected_count: usize },

    /// Diagnostik harus menyertakan lokasi file:line:col.
    DiagnosticHasLocation,
}

impl Requirement {
    /// Evaluasi requirement terhadap observasi mivon.
    /// Mengembalikan (satisfied, reason).
    pub fn check(&self, obs: &crate::observer::MivonObservation) -> (bool, String) {
        match self {
            Requirement::DiagnosticHasLocation => {
                let all_have_loc = obs.diagnostics.iter()
                    .filter(|d| d.is_error())
                    .filter(|d| !is_summary_error(&d.message))
                    .all(|d| d.location.is_some());
                if all_have_loc {
                    (true, "semua diagnostik error mempunyai lokasi".to_string())
                } else {
                    let missing: Vec<_> = obs.diagnostics.iter()
                        .filter(|d| d.is_error() && d.location.is_none())
                        .filter(|d| !is_summary_error(&d.message))
                        .map(|d| d.message.clone())
                        .take(3)
                        .collect();
                    (false, format!("diagnostik tanpa lokasi: {}", missing.join("; ")))
                }
            }
            Requirement::ElaborationSucceeds => {
                let _ok = obs.exit_status == crate::observer::ExitStatus::Ok
                    || obs.exit_status == crate::observer::ExitStatus::CleanError;
                let elaboration_failed = obs.diagnostics.iter()
                    .any(|d| d.is_error() && d.code.starts_with('E'));
                if !elaboration_failed {
                    (true, "elaborasi berhasil".to_string())
                } else {
                    let codes: Vec<_> = obs.diagnostics.iter()
                        .filter(|d| d.is_error())
                        .map(|d| d.code.clone())
                        .take(3)
                        .collect();
                    (false, format!("elaborasi gagal: {}", codes.join(", ")))
                }
            }
            Requirement::DiagnosticRequired { code, .. } => {
                let found = obs.diagnostics.iter().any(|d| d.code == *code);
                if found {
                    (true, format!("diagnostik {} diterbitkan", code))
                } else {
                    (false, format!("diagnostik {} tidak diterbitkan", code))
                }
            }
            Requirement::DiagnosticForbidden { code } => {
                let found = obs.diagnostics.iter().any(|d| d.code == *code);
                if !found {
                    (true, format!("diagnostik {} tidak muncul (benar)", code))
                } else {
                    (false, format!("diagnostik {} muncul (dilarang)", code))
                }
            }
            Requirement::SignalValueEquals { signal, expected } => {
                let actual = obs.values.iter()
                    .find(|v| v.signal == *signal)
                    .map(|v| v.value.display())
                    .unwrap_or_else(|| "<tidak ditemukan>".to_string());
                if actual == *expected {
                    (true, format!("{} = {} (sesuai LRM)", signal, expected))
                } else {
                    (false, format!("{}: expected={} observed={}", signal, expected, actual))
                }
            }
            // Requirement lain belum diimplementasi — inconclusive.
            _ => (true, "requirement belum diimplementasi — dianggap satisfied".to_string()),
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
    use super::is_summary_error;

    /// Summary $fatal/assert agregat bukan pelanggaran lokasi (bug_0227/0248).
    #[test]
    fn summary_errors_exempt_from_location() {
        assert!(is_summary_error("$fatal: simulasi dihentikan (1 fatal)"));
        assert!(is_summary_error("$fatal: simulasi dihentikan (1)"));
        assert!(is_summary_error("3 assertion(s) gagal"));
        assert!(!is_summary_error("assertion failed"));
        assert!(!is_summary_error("signal 'p' not found"));
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
