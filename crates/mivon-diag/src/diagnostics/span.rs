use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::intern::Symbol;

/// Rentang teks 1-based (inclusive) dalam satu file — tipe span "rich entry"
/// (Kritik-12) untuk struktur diagnostic terserialisasi (MICD `FileDiags`).
/// BEDA dari `intern::Span` (rentang byte leksikal string-pool) dan
/// `DiagSpan` (span runtime `Diagnostic`, byte-offset + file-symbol).
/// Pindah dari micd 2026-10 (konsolidasi diag — satu sumber tipe); MICD
/// re-export alias `Span` sehingga API & wire format (urutan field) tetap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl TextSpan {
    pub fn point(line: usize, col: usize) -> Self {
        TextSpan {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        }
    }
}

/// Informasi terkait (secondary span) — konteks tambahan di posisi lain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedInfo {
    pub span: TextSpan,
    pub message: String,
}

/// Source location untuk diagnostic.
#[derive(Debug, Clone)]
pub struct DiagSpan {
    pub file: Symbol,
    pub start: u32,
    pub end: u32,
    pub label: Option<Cow<'static, str>>,
}

impl DiagSpan {
    pub fn new(file: Symbol, start: u32, end: u32) -> Self {
        DiagSpan {
            file,
            start,
            end,
            label: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// Additional note yang dilampirkan ke diagnostic.
#[derive(Debug, Clone)]
pub struct DiagNote {
    pub message: Cow<'static, str>,
    pub span: Option<DiagSpan>,
}

impl DiagNote {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        DiagNote {
            message: message.into(),
            span: None,
        }
    }

    pub fn with_span(mut self, span: DiagSpan) -> Self {
        self.span = Some(span);
        self
    }
}
