//! mivon-diag — single owner untuk diagnostics + string interning (Symbol/Span).
//!
//! Dipakai oleh mivon-core (re-export untuk kompatibilitas) dan crate lain.
//! Tidak bergantung pada mivon-core untuk menghindari siklus dependensi.

pub mod diagnostics;
pub mod envdiag;
pub mod intern;

pub use diagnostics::{
    all_codes, cumulative_to_file_line, diag_global, format_diagnostic, format_suggestion,
    levenshtein, lookup_code, resolve_source_location, suggest_name, DiagCode, DiagLevel, DiagNote,
    DiagSink, DiagSpan, Diagnostic, FixItHint, GlobalDiagnosticEngine, ParserRecovery,
    RuntimeContext, SourceSnippet, TerminalEmitter,
};
pub use envdiag::{DiagnosticsContext, EmitterHandle};
pub use intern::{init_string_table, Span, Symbol};
