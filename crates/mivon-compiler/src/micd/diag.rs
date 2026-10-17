//! diagnostics.mdb — database diagnostic per file.
//!
//! IDE/LSP/GUI cukup query file → daftar diagnostic tanpa compile ulang.
//! Format append-friendly: satu objek per file berisi snapshot diagnostic
//! terbaru file tersebut. Sejak Kritik 12 db.md, setiap entry kaya:
//! primary span, secondary/related span, fix-it, dan code action — bukan
//! hanya `Error` berpesan.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use mivon_core::diagnostics::{DiagCode, DiagLevel, Diagnostic};

/// Tingkat keparahan diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagSeverity {
    Error,
    Warning,
    Hint,
    Note,
    Info,
}

/// Rentang teks (1-based, inclusive end) dalam satu file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl Span {
    pub fn point(line: usize, col: usize) -> Self {
        Span {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        }
    }
}

/// Informasi terkait (secondary span) — konteks tambahan di file/posisi lain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedInfo {
    pub span: Span,
    pub message: String,
}

/// Fix-it: saran perbaikan otomatis (rentang + teks pengganti).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixIt {
    pub range: Span,
    pub replacement: String,
    pub description: String,
}

/// Code action: aksi bernama yang ditawarkan IDE (bisa memuat edit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeAction {
    pub title: String,
    pub edit: Option<FixIt>,
}

/// Satu diagnostic per baris.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagEntry {
    /// Baris utama (singkatan — sama dengan `span.start_line` bila ada).
    pub line: usize,
    pub col: usize,
    pub severity: DiagSeverity,
    pub message: String,
    /// Kode diagnostic (kosong bila tidak ada).
    pub code: String,
    /// Primary span (rentang penuh, Kritik 12). `None` = point-only.
    pub span: Option<Span>,
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
        severity: DiagSeverity,
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

    /// Ubah ke Diagnostic core agar bisa dirender lewat renderer tunggal
    /// format_diagnostic. Severity dipetakan ke DiagLevel; kode (string)
    /// dan posisi dibawa ke pesan agar tidak hilang (ruang kode micd tidak
    /// dipetakan ke enum DiagCode core).
    pub fn to_diagnostic(&self) -> Diagnostic {
        let level = match self.severity {
            DiagSeverity::Error => DiagLevel::Error,
            DiagSeverity::Warning => DiagLevel::Warning,
            DiagSeverity::Hint => DiagLevel::Help,
            DiagSeverity::Note => DiagLevel::Note,
            DiagSeverity::Info => DiagLevel::Info,
        };
        let code = if self.code.is_empty() {
            String::new()
        } else {
            format!("[{}] ", self.code)
        };
        let pos = if self.line > 0 {
            format!("{}:{}: ", self.line, self.col)
        } else {
            String::new()
        };
        let message = format!("{}{}{}", pos, code, self.message);
        Diagnostic::new(level, DiagCode::InvalidSyntax, message)
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
            .filter(|e| e.severity == DiagSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.severity == DiagSeverity::Warning)
            .count()
    }

    /// Banyak entry yang membawa fix-it.
    pub fn fixable_count(&self) -> usize {
        self.entries.iter().filter(|e| e.fixit.is_some()).count()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::diagnostics::format_diagnostic;

    #[test]
    fn test_diag_entry_to_diagnostic_error() {
        let e = DiagEntry::new(
            10,
            4,
            DiagSeverity::Error,
            "bad port".to_string(),
            "E3005".to_string(),
        );
        let d = e.to_diagnostic();
        let s = format_diagnostic(&d);
        assert!(s.contains("bad port"));
        assert!(s.contains("E3005"));
        assert!(s.contains("error"));
        assert!(s.contains("10:4"));
    }

    #[test]
    fn test_diag_entry_to_diagnostic_warning_no_code() {
        let w = DiagEntry::new(
            2,
            1,
            DiagSeverity::Warning,
            "unused".to_string(),
            String::new(),
        );
        let d = w.to_diagnostic();
        let s = format_diagnostic(&d);
        assert!(s.contains("unused"));
        assert!(s.contains("warning"));
        assert!(!s.contains("[]"));
    }

    #[test]
    fn test_diag_counts() {
        let d = FileDiags {
            path: PathBuf::from("a.sv"),
            entries: vec![
                DiagEntry::new(1, 0, DiagSeverity::Error, "boom".into(), "ER001".into()),
                DiagEntry::new(2, 0, DiagSeverity::Warning, "warn".into(), "WR010".into()),
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
            DiagSeverity::Error,
            "width mismatch".into(),
            "WD001".into(),
        );
        e.span = Some(Span {
            start_line: 4,
            start_col: 10,
            end_line: 4,
            end_col: 22,
        });
        e.related.push(RelatedInfo {
            span: Span::point(1, 5),
            message: "dideklarasikan di sini".into(),
        });
        e.fixit = Some(FixIt {
            range: Span::point(4, 10),
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
        assert_eq!(d2.entries[0].actions.len(), 1);
        assert_eq!(d2.entries[0].related[0].message, "dideklarasikan di sini");
    }
}
