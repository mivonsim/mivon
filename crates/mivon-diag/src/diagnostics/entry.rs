//! Diagnostic entry kaya (Kritik-12) — baris diagnostic terserialisasi per
//! file (dipakai MICD `diagnostics.mdb`): primary span, secondary/related
//! span, fix-it, dan code action — bukan hanya "Error berpesan".
//!
//! Satu sumber tipe (konsolidasi diag 2026-10): pindah dari micd agar
//! compiler juga consumer murni; wire format identik (urutan field tetap)
//! sehingga tanpa bump VERSION MICD.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::span::TextSpan;
use super::{
    lookup_code, CodeAction, DiagCode, DiagLevel, Diagnostic, FixIt, RelatedInfo, SourceSnippet,
};

/// Satu diagnostic per baris.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagEntry {
    /// Baris utama (singkatan — sama dengan `span.start_line` bila ada).
    pub line: usize,
    pub col: usize,
    pub severity: DiagLevel,
    pub message: String,
    /// Kode diagnostic (kosong bila tidak ada).
    pub code: String,
    /// Primary span (rentang penuh, Kritik 12). `None` = point-only.
    pub span: Option<TextSpan>,
    /// Secondary/related span (Kritik 12).
    pub related: Vec<RelatedInfo>,
    /// Fix-it (Kritik 12).
    pub fixit: Option<FixIt>,
    /// Code actions (Kritik 12).
    pub actions: Vec<CodeAction>,
}

impl DiagEntry {
    /// Point diagnostic sederhana (tanpa span/fixit/actions).
    pub fn new(
        line: usize,
        col: usize,
        severity: DiagLevel,
        message: String,
        code: String,
    ) -> Self {
        DiagEntry {
            line,
            col,
            severity,
            message,
            code,
            span: None,
            related: Vec::new(),
            fixit: None,
            actions: Vec::new(),
        }
    }

    /// Proyeksi ke `Diagnostic` pusat agar dirender lewat renderer tunggal
    /// `format_diagnostic`.
    ///
    /// - Level: `DiagLevel` langsung — tidak ada pemetaan enum kedua.
    /// - Kode: string → enum via `lookup_code` bila terdaftar; bila tidak,
    ///   kode mentah tetap terlihat lewat awalan `[RAW]` di pesan (enum
    ///   fallback `SimulationError`).
    /// - Posisi: `file:line:col` dibawa lewat `SourceSnippet` (file dari
    ///   `FileDiags::path`) — bukan ditempel ke teks pesan.
    pub fn to_diagnostic(&self, file: &std::path::Path) -> Diagnostic {
        let raw = self.code.as_str();
        let (code, message) = match lookup_code(raw) {
            Some(c) => (c, self.message.clone()),
            None if !raw.is_empty() => (
                DiagCode::SimulationError,
                format!("[{}] {}", raw, self.message),
            ),
            None => (DiagCode::SimulationError, self.message.clone()),
        };
        let mut d = Diagnostic::new(self.severity, code, message);
        if !file.as_os_str().is_empty() && self.line > 0 {
            d = d.with_source_snippet(SourceSnippet::new(
                file.display().to_string(),
                self.line,
                self.col,
                String::new(),
            ));
        }
        d
    }
}

/// Diagnostic satu file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileDiags {
    pub path: PathBuf,
    pub entries: Vec<DiagEntry>,
    /// Hash konten file saat diagnostic diambil.
    pub content_hash: u64,
}

impl FileDiags {
    pub fn error_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.severity == DiagLevel::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.severity == DiagLevel::Warning)
            .count()
    }

    /// Banyak entry yang membawa fix-it.
    pub fn fixable_count(&self) -> usize {
        self.entries.iter().filter(|e| e.fixit.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::format_diagnostic;
    use std::path::Path;

    #[test]
    fn test_diag_entry_to_diagnostic_error() {
        let e = DiagEntry::new(
            10,
            4,
            DiagLevel::Error,
            "bad port".to_string(),
            "E3005".to_string(),
        );
        let d = e.to_diagnostic(Path::new("a.sv"));
        let s = format_diagnostic(&d);
        assert!(s.contains("bad port"));
        // E3005 terdaftar di registry → header kode asli, bukan ditempel ke pesan.
        assert!(s.contains("E3005"));
        assert!(s.contains("error"));
        // Posisi dari snippet (file:line:col), bukan prefix pesan.
        assert!(s.contains("10:4"));
    }

    #[test]
    fn test_diag_entry_to_diagnostic_warning_no_code() {
        let w = DiagEntry::new(
            2,
            1,
            DiagLevel::Warning,
            "unused".to_string(),
            String::new(),
        );
        let d = w.to_diagnostic(Path::new("a.sv"));
        let s = format_diagnostic(&d);
        assert!(s.contains("unused"));
        assert!(s.contains("warning"));
        assert!(!s.contains("[]"));
    }

    /// Kode di luar registry tetap terlihat (awalan [RAW]) — tidak hilang.
    #[test]
    fn test_diag_entry_raw_code_preserved() {
        let e = DiagEntry::new(1, 1, DiagLevel::Error, "weird".into(), "WD001".into());
        let d = e.to_diagnostic(Path::new("a.sv"));
        let s = format_diagnostic(&d);
        assert!(s.contains("WD001"), "kode mentah harus tetap terlihat: {s}");
        assert!(s.contains("weird"));
    }

    #[test]
    fn test_diag_counts() {
        let d = FileDiags {
            path: PathBuf::from("a.sv"),
            entries: vec![
                DiagEntry::new(1, 0, DiagLevel::Error, "boom".into(), "ER001".into()),
                DiagEntry::new(2, 0, DiagLevel::Warning, "warn".into(), "WR010".into()),
            ],
            content_hash: 1,
        };
        assert_eq!(d.error_count(), 1);
        assert_eq!(d.warning_count(), 1);
        assert_eq!(d.fixable_count(), 0);
    }

    #[test]
    fn test_rich_entry_roundtrip() {
        let mut e = DiagEntry::new(
            4,
            10,
            DiagLevel::Error,
            "width mismatch".into(),
            "WD001".into(),
        );
        e.span = Some(TextSpan {
            start_line: 4,
            start_col: 10,
            end_line: 4,
            end_col: 22,
        });
        e.related.push(RelatedInfo {
            span: TextSpan::point(1, 5),
            message: "dideklarasikan di sini".into(),
        });
        e.fixit = Some(FixIt {
            range: TextSpan::point(4, 10),
            replacement: "logic [7:0]".into(),
            description: "lebar kan ke 8 bit".into(),
        });
        e.actions.push(CodeAction {
            title: "Sembuhkan otomatis".into(),
            edit: e.fixit.clone(),
        });
        let d = FileDiags {
            path: PathBuf::from("a.sv"),
            entries: vec![e],
            content_hash: 42,
        };
        assert_eq!(d.fixable_count(), 1);
        let bytes = bincode::serialize(&d).unwrap();
        let d2: FileDiags = bincode::deserialize(&bytes).unwrap();
        assert_eq!(d, d2);
        assert_eq!(d2.entries[0].severity, DiagLevel::Error);
        assert_eq!(d2.entries[0].actions.len(), 1);
        assert_eq!(d2.entries[0].related[0].message, "dideklarasikan di sini");
    }
}
