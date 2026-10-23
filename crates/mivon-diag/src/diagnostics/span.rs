use std::borrow::Cow;

use crate::intern::Symbol;

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
