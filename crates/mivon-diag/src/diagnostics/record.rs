use std::borrow::Cow;
use std::fmt;

use super::{DiagCode, DiagLevel, DiagNote, DiagSpan, FixItHint, RuntimeContext, SourceSnippet};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagCode, DiagLevel, DiagSpan};
    use crate::intern::Symbol;
    use std::borrow::Cow;

    #[test]
    fn test_diagnostic_new() {
        let d = Diagnostic::new(DiagLevel::Error, DiagCode::UndefinedSignal, "test error");
        assert_eq!(d.level, DiagLevel::Error);
        assert_eq!(d.code, DiagCode::UndefinedSignal);
        assert_eq!(d.message, "test error");
        assert_eq!(d.spans.len(), 0);
        assert!(d.notes.is_empty());
        assert!(d.hints.is_empty());
        assert!(d.explanation.is_none());
        assert!(d.suggestion.is_none());
        assert!(d.runtime_context.is_none());
        assert!(d.source_snippet.is_none());
        assert!(d.example.is_none());
        assert!(d.fix_its.is_empty());
    }

    #[test]
    fn test_diagnostic_error() {
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "signal not found");
        assert_eq!(d.level, DiagLevel::Error);
        assert_eq!(d.code, DiagCode::UndefinedSignal);
        assert_eq!(d.message, "signal not found");
        assert!(d.is_error());
    }

    #[test]
    fn test_diagnostic_warning() {
        let d = Diagnostic::warning(DiagCode::WidthMismatch, "width mismatch");
        assert_eq!(d.level, DiagLevel::Warning);
        assert_eq!(d.code, DiagCode::WidthMismatch);
        assert_eq!(d.message, "width mismatch");
        assert!(!d.is_error());
    }

    #[test]
    fn test_diagnostic_note() {
        let d = Diagnostic::note("informational message");
        assert_eq!(d.level, DiagLevel::Note);
        assert_eq!(d.code, DiagCode::SimulationError);
        assert_eq!(d.message, "informational message");
    }

    #[test]
    fn test_diagnostic_info() {
        let d = Diagnostic::info("info message");
        assert_eq!(d.level, DiagLevel::Info);
        assert_eq!(d.code, DiagCode::SimulationError);
        assert_eq!(d.message, "info message");
    }

    #[test]
    fn test_diagnostic_bug() {
        let d = Diagnostic::bug("internal error");
        assert_eq!(d.level, DiagLevel::Bug);
        assert_eq!(d.code, DiagCode::InternalError);
        assert_eq!(d.message, "internal error");
        assert!(d.is_error());
        assert!(d.is_fatal());
    }

    #[test]
    fn test_diagnostic_fatal() {
        let d = Diagnostic::fatal(DiagCode::SimulationError, "fatal error");
        assert_eq!(d.level, DiagLevel::Fatal);
        assert_eq!(d.code, DiagCode::SimulationError);
        assert_eq!(d.message, "fatal error");
        assert!(d.is_error());
        assert!(d.is_fatal());
    }

    #[test]
    fn test_diagnostic_help() {
        let d = Diagnostic::help("consider using X instead");
        assert_eq!(d.level, DiagLevel::Help);
        assert_eq!(d.code, DiagCode::SimulationError);
        assert_eq!(d.message, "consider using X instead");
    }

    #[test]
    fn test_with_span() {
        let span = DiagSpan::new(Symbol::from("test.sv"), 10, 20);
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err").with_span(span);
        assert_eq!(d.spans.len(), 1);
        assert_eq!(d.spans[0].file, Symbol::from("test.sv"));
        assert_eq!(d.spans[0].start, 10);
        assert_eq!(d.spans[0].end, 20);
    }

    #[test]
    fn test_with_note() {
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err")
            .with_note("note message");
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].message, "note message");
    }

    #[test]
    fn test_with_hint() {
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err")
            .with_hint("try this");
        assert_eq!(d.hints.len(), 1);
        assert_eq!(d.hints[0], "try this");
    }

    #[test]
    fn test_with_explanation() {
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err")
            .with_explanation("explanation text");
        assert_eq!(d.explanation, Some(Cow::Borrowed("explanation text")));
    }

    #[test]
    fn test_with_suggestion() {
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err")
            .with_suggestion("suggestion text");
        assert_eq!(d.suggestion, Some(Cow::Borrowed("suggestion text")));
    }

    #[test]
    fn test_with_runtime_context() {
        let ctx = RuntimeContext {
            time: Some("100ns".to_string()),
            delta: Some(5),
            thread: Some("sim_thread".to_string()),
            module: Some("top".to_string()),
            instance: Some("uut".to_string()),
            seed: Some(42),
            process: Some("always_ff".to_string()),
        };
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err")
            .with_runtime_context(ctx.clone());
        let rc = d.runtime_context.expect("runtime context should be set");
        assert_eq!(rc.time, Some("100ns".to_string()));
        assert_eq!(rc.delta, Some(5));
        assert_eq!(rc.thread, Some("sim_thread".to_string()));
        assert_eq!(rc.module, Some("top".to_string()));
        assert_eq!(rc.instance, Some("uut".to_string()));
        assert_eq!(rc.seed, Some(42));
        assert_eq!(rc.process, Some("always_ff".to_string()));
    }

    #[test]
    fn test_with_source_snippet() {
        let snippet = SourceSnippet::new("foo.sv", 5, 10, "line content");
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err")
            .with_source_snippet(snippet.clone());
        let s = d.source_snippet.expect("source snippet should be set");
        assert_eq!(s.file, "foo.sv");
        assert_eq!(s.line, 5);
        assert_eq!(s.col, 10);
        assert_eq!(s.source_line, "line content");
    }

    #[test]
    fn test_with_example() {
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err")
            .with_example("example code");
        assert_eq!(d.example, Some(Cow::Borrowed("example code")));
    }

    #[test]
    fn test_with_fix_it() {
        let fix_it = FixItHint::insert("foo.sv", 1, 1, "import std;".to_string(), "add import".to_string());
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "err").with_fix_it(fix_it.clone());
        assert_eq!(d.fix_its.len(), 1);
        assert_eq!(d.fix_its[0].file, fix_it.file);
        assert_eq!(d.fix_its[0].start_line, fix_it.start_line);
        assert_eq!(d.fix_its[0].start_col, fix_it.start_col);
        assert_eq!(d.fix_its[0].end_line, fix_it.end_line);
        assert_eq!(d.fix_its[0].end_col, fix_it.end_col);
        assert_eq!(d.fix_its[0].replacement, fix_it.replacement);
        assert_eq!(d.fix_its[0].description, fix_it.description);
    }

    #[test]
    fn test_is_error() {
        assert!(Diagnostic::error(DiagCode::UndefinedSignal, "err").is_error());
        assert!(Diagnostic::bug("internal error").is_error());
        assert!(!Diagnostic::warning(DiagCode::WidthMismatch, "warn").is_error());
        assert!(!Diagnostic::note("info").is_error());
        assert!(!Diagnostic::info("info").is_error());
        assert!(!Diagnostic::help("hint").is_error());
    }

    #[test]
    fn test_is_fatal() {
        assert!(Diagnostic::fatal(DiagCode::SimulationError, "fatal").is_fatal());
        assert!(Diagnostic::bug("internal error").is_fatal());
        assert!(!Diagnostic::error(DiagCode::UndefinedSignal, "err").is_fatal());
        assert!(!Diagnostic::warning(DiagCode::WidthMismatch, "warn").is_fatal());
    }

    #[test]
    fn test_diagnostic_display() {
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "undefined signal")
            .with_span(DiagSpan::new(Symbol::from("test.sv"), 5, 10));
        let formatted = format!("{}", d);
        assert!(formatted.contains("error: E2001: undefined signal"));
        assert!(formatted.contains("test.sv:5:10"));
    }
}