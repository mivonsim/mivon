use std::fmt;

/// Derive serde: `DiagLevel` dipakai langsung di struktur serialisasi MICD
/// (`FileDiags`/`DiagEntry`) — satu enum level untuk seluruh sistem, tanpa
/// enum duplikat `DiagSeverity`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum DiagLevel {
    /// Fatal — simulasi harus dihentikan segera.
    Fatal,
    /// Internal compiler error — bug di compiler itu sendiri.
    Bug,
    /// Pasti salah — harus diperbaiki user.
    Error,
    /// Mencurigakan tapi valid — user harus perhatikan.
    Warning,
    /// Informasi tambahan.
    Note,
    /// Informasi umum.
    Info,
    /// Saran perbaikan.
    Help,
    /// Informasi trace untuk debugging.
    Trace,
    /// Informasi debug yang sangat detail.
    Debug,
}

impl DiagLevel {
    /// Kembalikan severity label pendek.
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagLevel::Fatal => "fatal",
            DiagLevel::Bug => "bug",
            DiagLevel::Error => "error",
            DiagLevel::Warning => "warning",
            DiagLevel::Note => "note",
            DiagLevel::Info => "info",
            DiagLevel::Help => "help",
            DiagLevel::Trace => "trace",
            DiagLevel::Debug => "debug",
        }
    }

    /// Apakah level ini menghentikan eksekusi?
    pub fn is_fatal(&self) -> bool {
        matches!(self, DiagLevel::Fatal | DiagLevel::Bug)
    }

    /// Apakah level ini dianggap error?
    pub fn is_error(&self) -> bool {
        matches!(self, DiagLevel::Fatal | DiagLevel::Bug | DiagLevel::Error)
    }
}

impl fmt::Display for DiagLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
