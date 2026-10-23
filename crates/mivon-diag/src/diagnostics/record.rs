use std::borrow::Cow;
use std::fmt;

use super::{
    DiagCode, DiagLevel, DiagNote, DiagSpan, FixItHint, RuntimeContext, SourceSnippet,
};

/// Structured diagnostic ala Rust/Cargo (error/warning/note/help).
///
/// Setiap diagnostic memiliki:
/// - Severity (fatal/error/warning/note/info/help/trace/debug)
/// - Error code (RT0001, WR0102, dll)
/// - Title/message utama
/// - Source location + snippet
/// - Runtime context (khusus simulasi HDL)
/// - Explanation
/// - Help/suggestion
/// - Notes tambahan
/// - Example code
/// - Fix-it hints (concrete code edits)
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub code: DiagCode,
    pub message: Cow<'static, str>,
    pub spans: Vec<DiagSpan>,
    pub notes: Vec<DiagNote>,
    pub hints: Vec<Cow<'static, str>>,
    /// Penjelasan panjang tentang penyebab error.
    pub explanation: Option<Cow<'static, str>>,
    /// Saran perbaikan spesifik.
    pub suggestion: Option<Cow<'static, str>>,
    /// Konteks simulasi (waktu, delta, thread, dll).
    pub runtime_context: Option<RuntimeContext>,
    /// Source snippet dengan pointer.
    pub source_snippet: Option<SourceSnippet>,
    /// Contoh kode perbaikan (ditampilkan setelah Help).
    pub example: Option<Cow<'static, str>>,
    /// Fix-it hints — edit kode konkret yang bisa diterapkan otomatis.
    pub fix_its: Vec<FixItHint>,
}

impl Diagnostic {
    pub fn new(level: DiagLevel, code: DiagCode, message: impl Into<Cow<'static, str>>) -> Self {
        Diagnostic {
            level,
            code,
            message: message.into(),
            spans: Vec::new(),
            notes: Vec::new(),
            hints: Vec::new(),
            explanation: None,
            suggestion: None,
            runtime_context: None,
            source_snippet: None,
            example: None,
            fix_its: Vec::new(),
        }
    }

    /// Buat fatal diagnostic — simulasi harus berhenti.
    pub fn fatal(code: DiagCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(DiagLevel::Fatal, code, message)
    }

    pub fn error(code: DiagCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(DiagLevel::Error, code, message)
    }

    pub fn warning(code: DiagCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(DiagLevel::Warning, code, message)
    }

    pub fn note(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(DiagLevel::Note, DiagCode::SimulationError, message)
    }

    pub fn info(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(DiagLevel::Info, DiagCode::SimulationError, message)
    }

    pub fn bug(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(DiagLevel::Bug, DiagCode::InternalError, message)
    }

    pub fn help(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(DiagLevel::Help, DiagCode::SimulationError, message)
    }

    pub fn with_span(mut self, span: DiagSpan) -> Self {
        self.spans.push(span);
        self
    }

    pub fn with_note(mut self, note: impl Into<Cow<'static, str>>) -> Self {
        self.notes.push(DiagNote::new(note));
        self
    }

    pub fn with_hint(mut self, hint: impl Into<Cow<'static, str>>) -> Self {
        self.hints.push(hint.into());
        self
    }

    pub fn with_explanation(mut self, explanation: impl Into<Cow<'static, str>>) -> Self {
        self.explanation = Some(explanation.into());
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<Cow<'static, str>>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn with_runtime_context(mut self, ctx: RuntimeContext) -> Self {
        self.runtime_context = Some(ctx);
        self
    }

    pub fn with_source_snippet(mut self, snippet: SourceSnippet) -> Self {
        self.source_snippet = Some(snippet);
        self
    }

    /// Buat diagnostic dengan explanation + help dari DiagCode yang sesuai.
    pub fn with_code_context(mut self) -> Self {
        if self.explanation.is_none() {
            let expl = self.code.explanation();
            if !expl.is_empty() {
                self.explanation = Some(Cow::Borrowed(expl));
            }
        }
        if self.suggestion.is_none() {
            let hlp = self.code.help();
            if !hlp.is_empty() {
                self.suggestion = Some(Cow::Borrowed(hlp));
            }
        }
        self
    }

    /// Tambahkan contoh kode perbaikan (ditampilkan sebagai "Example" setelah Help).
    pub fn with_example(mut self, example: impl Into<Cow<'static, str>>) -> Self {
        self.example = Some(example.into());
        self
    }

    /// Tambahkan fix-it hint (edit kode konkret).
    pub fn with_fix_it(mut self, fix_it: FixItHint) -> Self {
        self.fix_its.push(fix_it);
        self
    }

    pub fn is_error(&self) -> bool {
        self.level.is_error()
    }

    pub fn is_fatal(&self) -> bool {
        self.level.is_fatal()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.level, self.code, self.message)?;

        for span in &self.spans {
            if self.source_snippet.is_some() {
                break; // snippet sudah menampilkan lokasi file:line:col
            }
            write!(f, "\n  --> {}:{}:{}", span.file, span.start, span.end)?;
            if let Some(label) = &span.label {
                write!(f, " — {}", label)?;
            }
        }

        if let Some(snippet) = &self.source_snippet {
            write!(f, "\n{}", snippet.format(false))?;
        }

        if let Some(ctx) = &self.runtime_context {
            let ctx_str = ctx.format();
            if !ctx_str.is_empty() {
                write!(f, "\n{}", ctx_str)?;
            }
        }

        if let Some(explanation) = &self.explanation {
            write!(f, "\nExplanation\n\n{}", explanation)?;
        }

        for note in &self.notes {
            write!(f, "\n  = note: {}", note.message)?;
        }

        if let Some(suggestion) = &self.suggestion {
            write!(f, "\n  = help: {}", suggestion)?;
        }

        if let Some(example) = &self.example {
            write!(f, "\n\nExample\n\n{}", example)?;
        }

        for hint in &self.hints {
            write!(f, "\n  = help: {}", hint)?;
        }

        if !self.fix_its.is_empty() {
            write!(f, "\n\nFix-it suggestions:")?;
            for fix_it in &self.fix_its {
                write!(
                    f,
                    "\n  = fix-it: {} ({}:{}:{}-{}:{})",
                    fix_it.description,
                    fix_it.file,
                    fix_it.start_line,
                    fix_it.start_col,
                    fix_it.end_line,
                    fix_it.end_col
                )?;
                if !fix_it.replacement.is_empty() {
                    write!(f, " -> '{}'", fix_it.replacement.escape_debug())?;
                } else {
                    write!(f, " (delete)")?;
                }
            }
        }

        Ok(())
    }
}
