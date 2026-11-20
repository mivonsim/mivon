use std::fmt;

/// Derive serde: `DiagLevel` dipakai langsung di struktur serialisasi MICD
/// (`FileDiags`/`DiagEntry`) — satu enum level untuk seluruh sistem, tanpa
/// enum duplikat `DiagSeverity`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_as_str() {
        assert_eq!(DiagLevel::Fatal.as_str(), "fatal");
        assert_eq!(DiagLevel::Bug.as_str(), "bug");
        assert_eq!(DiagLevel::Error.as_str(), "error");
        assert_eq!(DiagLevel::Warning.as_str(), "warning");
        assert_eq!(DiagLevel::Note.as_str(), "note");
        assert_eq!(DiagLevel::Info.as_str(), "info");
        assert_eq!(DiagLevel::Help.as_str(), "help");
        assert_eq!(DiagLevel::Trace.as_str(), "trace");
        assert_eq!(DiagLevel::Debug.as_str(), "debug");
    }

    #[test]
    fn test_level_is_fatal() {
        assert!(DiagLevel::Fatal.is_fatal());
        assert!(DiagLevel::Bug.is_fatal());
        assert!(!DiagLevel::Error.is_fatal());
        assert!(!DiagLevel::Warning.is_fatal());
        assert!(!DiagLevel::Note.is_fatal());
        assert!(!DiagLevel::Info.is_fatal());
        assert!(!DiagLevel::Help.is_fatal());
        assert!(!DiagLevel::Trace.is_fatal());
        assert!(!DiagLevel::Debug.is_fatal());
    }

    #[test]
    fn test_level_is_error() {
        assert!(DiagLevel::Fatal.is_error());
        assert!(DiagLevel::Bug.is_error());
        assert!(DiagLevel::Error.is_error());
        assert!(!DiagLevel::Warning.is_error());
        assert!(!DiagLevel::Note.is_error());
        assert!(!DiagLevel::Info.is_error());
        assert!(!DiagLevel::Help.is_error());
        assert!(!DiagLevel::Trace.is_error());
        assert!(!DiagLevel::Debug.is_error());
    }

    #[test]
    fn test_level_ordering() {
        use std::cmp::Ordering;

        // derive(Ord) orders by declaration order: Fatal (0) is smallest,
        // Debug (8) is greatest.
        assert!(DiagLevel::Fatal < DiagLevel::Bug);
        assert!(DiagLevel::Bug < DiagLevel::Error);
        assert!(DiagLevel::Error < DiagLevel::Warning);
        assert!(DiagLevel::Warning < DiagLevel::Note);
        assert!(DiagLevel::Note < DiagLevel::Info);
        assert!(DiagLevel::Info < DiagLevel::Help);
        assert!(DiagLevel::Help < DiagLevel::Trace);
        assert!(DiagLevel::Trace < DiagLevel::Debug);

        assert_eq!(DiagLevel::Fatal.cmp(&DiagLevel::Fatal), Ordering::Equal);
        assert_eq!(DiagLevel::Debug.cmp(&DiagLevel::Debug), Ordering::Equal);
        assert_eq!(DiagLevel::Warning.cmp(&DiagLevel::Warning), Ordering::Equal);
    }

    #[test]
    fn test_level_display_matches_as_str() {
        assert_eq!(format!("{}", DiagLevel::Fatal), DiagLevel::Fatal.as_str());
        assert_eq!(format!("{}", DiagLevel::Warning), DiagLevel::Warning.as_str());
        assert_eq!(format!("{}", DiagLevel::Info), DiagLevel::Info.as_str());
        assert_eq!(format!("{}", DiagLevel::Debug), DiagLevel::Debug.as_str());
    }

    #[test]
    fn test_level_clone_copy_eq() {
        let a = DiagLevel::Error;
        let b = DiagLevel::Error;
        let c = DiagLevel::Warning;

        assert_eq!(a, b);
        assert_ne!(a, c);

        let _cloned = a.clone();
        let _copied: DiagLevel = a;
        let _ = a; // a still usable because Copy

        assert_eq!(a, b);
    }
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
