//! AI-assisted debugging — stage 1 of #70.
//! Memberikan saran perbaikan berdasarkan kode diagnostik dan pola error
//! yang dikenal, dengan confidence score.
//! 1 file = 1 tanggung jawab: hanya AI debug suggestion engine.

use crate::diagnostics::DiagCode;

/// Hasil saran perbaikan dari AI debug engine.
#[derive(Debug, Clone)]
pub struct AiFixSuggestion {
    /// Kode diagnostik yang ditangani.
    pub diag_code: DiagCode,
    /// Deskripsi saran perbaikan.
    pub suggestion: String,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f64,
    /// Apakah saran ini dapat diaplikasikan otomatis.
    pub auto_applicable: bool,
}

/// Engine saran perbaikan berbasis aturan (AI-assisted debugging foundation).
pub struct AiDebugEngine {
    /// Daftar saran yang dikenal (kode diagnostik → saran, confidence, auto).
    suggestions: Vec<(DiagCode, String, f64, bool)>,
}

impl Default for AiDebugEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AiDebugEngine {
    pub fn new() -> Self {
        Self {
            suggestions: vec![
                // Parse errors
                (
                    DiagCode::ExpectedSemi,
                    "Tambahkan titik koma (;) di akhir statement".to_string(),
                    0.90,
                    true,
                ),
                (
                    DiagCode::UnclosedBlock,
                    "Periksa kurung kurawal yang tidak ditutup pada module atau block".to_string(),
                    0.85,
                    false,
                ),
                (
                    DiagCode::UnexpectedToken,
                    "Periksa token yang tidak terduga —可能 ada karakter yang salah atau hilang".to_string(),
                    0.80,
                    false,
                ),
                // Semantic errors
                (
                    DiagCode::WidthMismatch,
                    "Periksa lebar sinyal (width mismatch) pada assignment".to_string(),
                    0.88,
                    false,
                ),
                (
                    DiagCode::TypeMismatch,
                    "Periksa tipe data pada operand atau assignment".to_string(),
                    0.82,
                    false,
                ),
                (
                    DiagCode::UndefinedSignal,
                    "Verifikasi sinyal telah dideklarasikan sebelum digunakan".to_string(),
                    0.85,
                    false,
                ),
                // Simulation errors
                (
                    DiagCode::NbaWriteConflict,
                    "Periksa race condition pada sinyal yang ditulis oleh multiple always block".to_string(),
                    0.75,
                    false,
                ),
                (
                    DiagCode::SignalUninitialized,
                    "Periksa propagasi X pada sinyal yang tidak diinisialisasi".to_string(),
                    0.82,
                    false,
                ),
                (
                    DiagCode::SchedulerDeadlock,
                    "Periksa fork/join atau wait yang mungkin menyebabkan deadlock".to_string(),
                    0.78,
                    false,
                ),
                // Runtime
                (
                    DiagCode::NotImplemented,
        "Fitur belum diimplementasikan — periksa dokumentasi atau gunakan alternatif".to_string(),
                    0.95,
                    false,
                ),
            ],
        }
    }

    /// Cari saran perbaikan untuk kode diagnostik.
    pub fn suggest(&self, code: DiagCode) -> Option<AiFixSuggestion> {
        self.suggestions
            .iter()
            .find(|(c, _, _, _)| *c == code)
            .map(|(c, suggestion, confidence, auto)| AiFixSuggestion {
                diag_code: *c,
                suggestion: suggestion.clone(),
                confidence: *confidence,
                auto_applicable: *auto,
            })
    }

    /// Cari semua saran dengan confidence di atas threshold.
    pub fn suggest_above_threshold(&self, threshold: f64) -> Vec<AiFixSuggestion> {
        self.suggestions
            .iter()
            .filter(|(_, _, conf, _)| *conf >= threshold)
            .map(|(c, suggestion, confidence, auto)| AiFixSuggestion {
                diag_code: *c,
                suggestion: suggestion.clone(),
                confidence: *confidence,
                auto_applicable: *auto,
            })
            .collect()
    }

    /// Jumlah total saran yang tersedia.
    pub fn suggestion_count(&self) -> usize {
        self.suggestions.len()
    }
}

/// Dapatkan saran perbaikan cepat untuk kode diagnostik (fungsi helper).
pub fn quick_suggest(code: DiagCode) -> Option<String> {
    let engine = AiDebugEngine::default();
    engine.suggest(code).map(|s| s.suggestion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ai_debug_engine_new() {
        let engine = AiDebugEngine::new();
        assert!(engine.suggestion_count() > 0, "harus ada saran default");
    }

    #[test]
    fn test_ai_debug_suggest_semicolon() {
        let engine = AiDebugEngine::new();
        let suggestion = engine.suggest(DiagCode::ExpectedSemi);
        assert!(suggestion.is_some(), "harus ada saran untuk ExpectedSemi");
        let s = suggestion.unwrap();
        assert!(s.confidence > 0.8);
        assert!(s.auto_applicable, "titik koma harus auto-applicable");
    }

    #[test]
    fn test_ai_debug_suggest_unclosed_block() {
        let engine = AiDebugEngine::new();
        let suggestion = engine.suggest(DiagCode::UnclosedBlock);
        assert!(suggestion.is_some());
        let s = suggestion.unwrap();
        assert!(s.suggestion.to_lowercase().contains("kurung") || s.suggestion.to_lowercase().contains("block"));
        assert!(!s.auto_applicable);
    }

    #[test]
    fn test_ai_debug_suggest_width_mismatch() {
        let engine = AiDebugEngine::new();
        let suggestion = engine.suggest(DiagCode::WidthMismatch);
        assert!(suggestion.is_some());
        let s = suggestion.unwrap();
        assert!(s.suggestion.to_lowercase().contains("lebar") || s.suggestion.to_lowercase().contains("width"));
    }

    #[test]
    fn test_ai_debug_suggest_race_condition() {
        let engine = AiDebugEngine::new();
        let suggestion = engine.suggest(DiagCode::NbaWriteConflict);
        assert!(suggestion.is_some());
        let s = suggestion.unwrap();
        assert!(s.suggestion.to_lowercase().contains("race"), "harus menyebutkan race condition");
    }

    #[test]
    fn test_ai_debug_suggest_x_propagation() {
        let engine = AiDebugEngine::new();
        let suggestion = engine.suggest(DiagCode::SignalUninitialized);
        assert!(suggestion.is_some());
        let s = suggestion.unwrap();
        assert!(
            s.suggestion.to_lowercase().contains("x") || s.suggestion.to_lowercase().contains("inisialisasi"),
            "harus menyebutkan X atau inisialisasi"
        );
    }

    #[test]
    fn test_ai_debug_suggest_unknown() {
        let engine = AiDebugEngine::new();
        // Use a variant that's not in our suggestions list
        let suggestion = engine.suggest(DiagCode::InternalError);
        assert!(suggestion.is_none(), "kode tidak dikenal harus kembali None");
    }

    #[test]
    fn test_ai_debug_threshold() {
        let engine = AiDebugEngine::new();
        let high_conf = engine.suggest_above_threshold(0.85);
        assert!(
            high_conf.iter().all(|s| s.confidence >= 0.85),
            "semua harus di atas threshold"
        );
        let all = engine.suggest_above_threshold(0.0);
        assert_eq!(
            all.len(),
            engine.suggestion_count(),
            "threshold 0 harus mengembalikan semua"
        );
    }

    #[test]
    fn test_quick_suggest() {
        let suggestion = quick_suggest(DiagCode::ExpectedSemi);
        assert!(suggestion.is_some());
        assert_eq!(
            suggestion.unwrap(),
            "Tambahkan titik koma (;) di akhir statement"
        );

        let none = quick_suggest(DiagCode::InternalError);
        assert!(none.is_none());
    }

    #[test]
    fn test_ai_fix_suggestion_fields() {
        let engine = AiDebugEngine::new();
        let s = engine.suggest(DiagCode::WidthMismatch).unwrap();
        assert_eq!(s.diag_code, DiagCode::WidthMismatch);
        assert!(!s.auto_applicable);
    }
}
