//! mivon-diag — single owner untuk diagnostics + string interning (Symbol/Span).
//!
//! Dipakai oleh mivon-core (re-export untuk kompatibilitas) dan crate lain.
//! Tidak bergantung pada mivon-core untuk menghindari siklus dependensi.

pub mod diagnostics;
pub mod envdiag;
pub mod intern;

pub use intern::{Span, Symbol, init_string_table};
pub use envdiag::{DiagnosticsContext, EmitterHandle};
pub use diagnostics::{
    all_codes, lookup_code, DiagCode, DiagLevel, FixItHint, Diagnostic, SourceSnippet,
    RuntimeContext, DiagSink, DiagNote, DiagSpan, format_diagnostic, TerminalEmitter,
    diag_global, GlobalDiagnosticEngine, ParserRecovery, format_suggestion, levenshtein,
    suggest_name, cumulative_to_file_line, resolve_source_location,
};
