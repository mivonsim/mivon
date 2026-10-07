//! ClauseRef — referensi ke klausul IEEE 1800 LRM.
//!
//! Menyimpan referensi, bukan teks LRM verbatim.

#![cfg(feature = "dev")]

/// Referensi ke klausul standar IEEE.
/// Contoh: IEEE 1800-2017 §23.10.1
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClauseRef {
    pub standard: Standard,
    /// Nomor klausul — contoh: "6.24.1", "11.4.14", "23.10.1"
    pub section: String,
    /// Revisi standar — "2012" atau "2017"
    pub revision: &'static str,
}

impl ClauseRef {
    pub fn sv2017(section: impl Into<String>) -> Self {
        Self {
            standard: Standard::Ieee1800_2017,
            section: section.into(),
            revision: "2017",
        }
    }

    pub fn sv2012(section: impl Into<String>) -> Self {
        Self {
            standard: Standard::Ieee1800_2012,
            section: section.into(),
            revision: "2012",
        }
    }

    /// Format untuk display — contoh: "IEEE 1800-2017 §23.10.1"
    pub fn display(&self) -> String {
        format!("IEEE 1800-{} §{}", self.revision, self.section)
    }
}

/// Standar IEEE yang direferensikan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standard {
    Ieee1800_2012,
    Ieee1800_2017,
    Ieee1364_2005,
}
