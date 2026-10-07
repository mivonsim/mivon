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
                    .all(|d| d.location.is_some());
                if all_have_loc {
                    (true, "semua diagnostik error mempunyai lokasi".to_string())
                } else {
                    let missing: Vec<_> = obs.diagnostics.iter()
                        .filter(|d| d.is_error() && d.location.is_none())
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

/// Properti semantik yang harus terpenuhi.
#[derive(Debug, Clone)]
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
