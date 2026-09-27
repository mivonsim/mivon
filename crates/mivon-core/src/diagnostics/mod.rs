//! Diagnostics — error recovery, diagnostic sink, formatted output.
//!
//! Phase 4 implementation. Thread-safe diagnostic collection via MPSC channel.

pub mod codes;
pub mod diagnostic;
pub mod emitter;
pub mod global;
pub mod recovery;
pub mod suggest;

pub use codes::{all_codes, lookup_code};
pub use diagnostic::{
    DiagCode, DiagLevel, DiagNote, DiagSink, DiagSpan, Diagnostic, FixItHint, RuntimeContext,
    SourceSnippet,
};
pub use emitter::{format_diagnostic, TerminalEmitter};
pub use global::{diag_global, GlobalDiagnosticEngine};
pub use recovery::ParserRecovery;
pub use suggest::{format_suggestion, levenshtein, suggest_name};

/// Konversi baris cumulative (merged source) → baris relatif-file berdasar
/// directive `` `line ``.
///
/// Satu-satunya definisi formula value-aware (dipakai parser, simulator, dan
/// core resolver — jangan duplikasi rumus ini di tempat lain):
///   directive `` `line VAL "file" `` di baris fisik `bp` (1-based) menyatakan
///   baris berikutnya (`bp+1`) adalah baris `VAL` file. Maka baris fisik `C`
///   adalah baris file `VAL + (C - bp) - 1`.
///
/// `bp` = baris fisik (1-based) directive terdekat yang berada DI ATAS `C`.
pub fn cumulative_to_file_line(bp: usize, val: usize, cumulative_line: usize) -> usize {
    if cumulative_line <= bp {
        return val;
    }
    val + (cumulative_line - bp) - 1
}

/// Resolve nama file + baris relatif-file untuk posisi di merged source.
///
/// Source gabungan berisi directive `` `line N "file" `` di awal tiap file.
/// Directive di index `i` (0-based) mendeklarasikan: baris berikutnya (merged)
/// adalah baris `N` dari `file`. Untuk baris error `line` (1-based merged),
/// baris relatif-file = N + line - (i+1) - 1. Fallback ke `default_file`.
pub fn resolve_source_location(
    source_lines: &[String],
    default_file: &str,
    line: usize,
) -> (String, usize) {
    if line == 0 || line > source_lines.len() {
        return (default_file.to_string(), line);
    }
    // Scan mundur dari baris error untuk directive `line terakhir
    let end = line.saturating_sub(1); // last index to check (0-based)
    for i in (0..=end).rev() {
        let src = &source_lines[i];
        if let Some(rest) = src.strip_prefix('`') {
            let rest = rest.trim_start();
            if rest.starts_with("line ") {
                // Parse angka baris deklarasi: `line N "filename"
                let mut declared: usize = 1;
                let digits = rest.strip_prefix("line ").unwrap_or(rest).trim_start();
                let digits_end = digits
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(digits.len());
                if let Ok(n) = digits[..digits_end].parse::<usize>() {
                    declared = n.max(1);
                }
                if let Some(start) = rest.find('"') {
                    if let Some(end_q) = rest[start + 1..].find('"') {
                        let file = rest[start + 1..start + 1 + end_q].to_string();
                        let relative = cumulative_to_file_line(i + 1, declared, line);
                        return (file, relative.max(1));
                    }
                }
            }
        }
    }
    (default_file.to_string(), line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumulative_to_file_line_basic() {
        // Directive `line 100 "a.sv"` di baris fisik 5 → baris fisik 6 = 100.
        assert_eq!(cumulative_to_file_line(5, 100, 6), 100);
        assert_eq!(cumulative_to_file_line(5, 100, 7), 101);
        assert_eq!(cumulative_to_file_line(5, 100, 10), 104);
    }

    #[test]
    fn cumulative_to_file_line_at_or_before_directive() {
        // Baris fisik sama dengan baris directive → jatuh ke nilai directive.
        assert_eq!(cumulative_to_file_line(5, 100, 5), 100);
    }

    #[test]
    fn resolve_source_location_basic() {
        let lines: Vec<String> = vec!["`line 1 \"a.sv\"".to_string(), "module top;".to_string()];
        let (file, line) = resolve_source_location(&lines, "default.sv", 2);
        assert_eq!(file, "a.sv");
        assert_eq!(line, 1);
    }
}
