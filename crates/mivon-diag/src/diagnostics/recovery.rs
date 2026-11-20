//! Parser Error Recovery — never halt, always continue.
//!
//! Saat parser error, kita:
//! 1. Push diagnostic (jangan stop)
//! 2. Skip sampai sync token
//! 3. Return dummy node supaya parsing lanjut

use super::diagnostic::{DiagCode, DiagSink, DiagSpan, Diagnostic};
use crate::intern::Symbol;

/// Sync tokens — parser berhenti skip saat token ini ditemukan.
pub fn is_sync_token(token_name: &str) -> bool {
    matches!(
        token_name,
        ";" | "endmodule"
            | "end"
            | "endcase"
            | "endfunction"
            | "endtask"
            | "endclass"
            | "endgenerate"
            | "endpackage"
            | "endinterface"
            | "endprogram"
            | "}"
            | ")"
            | "]"
            | "eof"
    )
}

/// Recovery strategy for parser errors.
pub struct ParserRecovery<'a> {
    sink: &'a DiagSink,
    file: Symbol,
}

impl<'a> ParserRecovery<'a> {
    pub fn new(sink: &'a DiagSink, file: Symbol) -> Self {
        ParserRecovery { sink, file }
    }

    /// Report an error and skip to the next sync point.
    pub fn recover(&self, code: DiagCode, message: String, line: usize, col: usize) -> Diagnostic {
        let diag = Diagnostic::error(code, message).with_span(DiagSpan::new(
            self.file,
            line as u32,
            col as u32,
        ));

        self.sink.push(diag.clone());
        diag
    }

    /// Report a warning (non-fatal).
    pub fn warn(&self, code: DiagCode, message: String, line: usize, col: usize) -> Diagnostic {
        let diag = Diagnostic::warning(code, message).with_span(DiagSpan::new(
            self.file,
            line as u32,
            col as u32,
        ));

        self.sink.push(diag.clone());
        diag
    }

    /// Recover with a hint.
    pub fn recover_with_hint(
        &self,
        code: DiagCode,
        message: String,
        hint: String,
        line: usize,
        col: usize,
    ) -> Diagnostic {
        let diag = Diagnostic::error(code, message)
            .with_span(DiagSpan::new(self.file, line as u32, col as u32))
            .with_hint(hint);

        self.sink.push(diag.clone());
        diag
    }

    /// Build a "expected X" error with common sync-point hints.
    pub fn expected_token(
        &self,
        expected: &str,
        found: &str,
        line: usize,
        col: usize,
    ) -> Diagnostic {
        self.recover_with_hint(
            DiagCode::ExpectedToken,
            format!("expected '{}', found '{}'", expected, found),
            format!("try adding '{}' here", expected),
            line,
            col,
        )
    }
}

/// Check if we should stop skipping (found a sync token).
pub fn should_stop_skipping(token: &str) -> bool {
    is_sync_token(token)
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_tokens() {
        assert!(is_sync_token(";"));
        assert!(is_sync_token("endmodule"));
        assert!(is_sync_token("}"));
        assert!(!is_sync_token("foo"));
    }

    #[test]
    fn test_parser_recovery() {
        let sink = DiagSink::new();
        let recovery = ParserRecovery::new(&sink, Symbol::intern("test.sv"));

        let d = recovery.recover(DiagCode::UnexpectedToken, "bad token".to_string(), 10, 5);
        assert_eq!(d.level, crate::diagnostics::DiagLevel::Error);
        assert_eq!(d.code, DiagCode::UnexpectedToken);

        let diags = sink.diagnostics();
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn test_sync_token_edge_cases() {
        // Keywords that should NOT be sync tokens
        assert!(!is_sync_token("always"));
        assert!(!is_sync_token("initial"));
        assert!(!is_sync_token("module"));
        assert!(!is_sync_token("identifier"));
        assert!(!is_sync_token("clk"));
        // Known sync tokens should still be recognized
        assert!(is_sync_token("endmodule"));
        assert!(is_sync_token("endpackage"));
        assert!(is_sync_token("endfunction"));
        assert!(is_sync_token("endtask"));
        assert!(is_sync_token("endclass"));
        assert!(is_sync_token("endinterface"));
        assert!(is_sync_token("endprogram"));
    }

    #[test]
    fn test_recover_with_hint() {
        let sink = DiagSink::new();
        let recovery = ParserRecovery::new(&sink, Symbol::intern("test.sv"));

        let d = recovery.recover_with_hint(
            DiagCode::ExpectedToken,
            "expected semicolon".to_string(),
            "add ';'".to_string(),
            5,
            10,
        );
        assert_eq!(d.level, crate::diagnostics::DiagLevel::Error);
        assert_eq!(d.code, DiagCode::ExpectedToken);
        assert_eq!(d.hints.len(), 1);
        assert_eq!(d.hints[0], "add ';'");

        let diags = sink.diagnostics();
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn test_expected_token() {
        let sink = DiagSink::new();
        let recovery = ParserRecovery::new(&sink, Symbol::intern("test.sv"));

        let d = recovery.expected_token(";", ",", 5, 10);
        assert_eq!(d.level, crate::diagnostics::DiagLevel::Error);
        assert_eq!(d.code, DiagCode::ExpectedToken);
        assert_eq!(d.message, "expected ';', found ','");
        assert_eq!(d.hints.len(), 1);
        assert_eq!(d.hints[0], "try adding ';' here");

        let diags = sink.diagnostics();
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn test_parser_recovery_warn() {
        let sink = DiagSink::new();
        let recovery = ParserRecovery::new(&sink, Symbol::intern("test.sv"));

        let d = recovery.warn(DiagCode::WidthMismatch, "width mismatch".to_string(), 5, 10);
        assert_eq!(d.level, crate::diagnostics::DiagLevel::Warning);
        assert_eq!(d.code, DiagCode::WidthMismatch);
        assert_eq!(d.message, "width mismatch");
        assert!(!d.is_error());

        let diags = sink.diagnostics();
        assert_eq!(diags.len(), 1);
    }
}
