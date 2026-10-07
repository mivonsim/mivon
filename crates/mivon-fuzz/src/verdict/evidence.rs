//! Evidence — bukti konkret yang menopang verdict.

#![cfg(feature = "dev")]

/// Satu butir bukti yang menopang verdict.
#[derive(Debug, Clone)]
pub struct Evidence {
    /// Label pendek — contoh: "elaboration.parameters[\"W\"]"
    pub label: String,
    /// Nilai yang diobservasi.
    pub observed: String,
    /// Nilai yang diharapkan menurut LRM.
    pub expected: Option<String>,
    /// Apakah observed == expected.
    pub matches: bool,
}

impl Evidence {
    pub fn matching(label: impl Into<String>, value: impl Into<String>) -> Self {
        let v = value.into();
        Self {
            label: label.into(),
            observed: v.clone(),
            expected: Some(v),
            matches: true,
        }
    }

    pub fn mismatch(
        label: impl Into<String>,
        observed: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            observed: observed.into(),
            expected: Some(expected.into()),
            matches: false,
        }
    }

    pub fn observed_only(label: impl Into<String>, observed: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            observed: observed.into(),
            expected: None,
            matches: true,
        }
    }
}

/// Jenis kegagalan internal Mivon (bukan conformance issue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureKind {
    Panic,
    Hang,
    Crash,
    Abort,
}

impl FailureKind {
    pub fn label(&self) -> &'static str {
        match self {
            FailureKind::Panic => "panic",
            FailureKind::Hang => "hang",
            FailureKind::Crash => "crash",
            FailureKind::Abort => "abort",
        }
    }
}
