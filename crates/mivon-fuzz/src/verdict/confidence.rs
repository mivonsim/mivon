//! JudgeConfidence — seberapa kuat bukti yang mendasari verdict.

#![cfg(feature = "dev")]

/// Tingkat kepercayaan verdict.
///
/// `Violation` + `Insufficient` tidak disimpan sebagai bug —
/// hanya sebagai finding untuk review manual.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum JudgeConfidence {
    /// Terbukti secara formal dari aturan LRM + observasi lengkap.
    /// Semua kondisi applicability terpenuhi, expected vs observed jelas.
    Proven,

    /// Bukti kuat — tidak ada ambiguitas signifikan.
    Strong,

    /// Bergantung pada asumsi yang masuk akal tapi tidak terbukti penuh.
    Conditional { assumptions: Vec<String> },

    /// Informasi tidak cukup untuk keputusan kuat.
    /// Verdict Violation + Insufficient = jangan simpan sebagai bug.
    Insufficient,
}

impl JudgeConfidence {
    pub fn label(&self) -> &'static str {
        match self {
            JudgeConfidence::Proven => "proven",
            JudgeConfidence::Strong => "strong",
            JudgeConfidence::Conditional { .. } => "conditional",
            JudgeConfidence::Insufficient => "insufficient",
        }
    }

    /// Apakah confidence cukup untuk menyimpan sebagai bug.
    pub fn is_actionable(&self) -> bool {
        matches!(self, JudgeConfidence::Proven | JudgeConfidence::Strong)
    }
}
