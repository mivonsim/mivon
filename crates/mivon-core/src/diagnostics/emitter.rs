//! Diagnostic Emitter — formatted output ala Cargo/Rust untuk terminal dan LSP.
//!
//! Format keluaran:
//!
//! ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//!
//! error[RT0001]: Null handle access
//!
//! ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//!
//!    ┌─ uart.sv:52:14
//!    │
//! 52 │ fifo.push(data);
//!    │      ^^^^ ── Handle "fifo" is null
//!    │
//!    = module : uart_top
//!    = process: write_task
//!    = time   : 125 ns
//!
//! Explanation
//!
//! The FIFO handle was never initialized before simulation started.
//!
//! Help
//!
//! Initialize fifo before use: fifo = new();

use std::io::{self, Write};

use super::diagnostic::{DiagLevel, DiagSink, Diagnostic};

/// Box drawing characters
#[allow(dead_code)] // BOX_V dipakai SourceSnippet::format (diagnostic.rs); tidak lagi di emitter
const BOX_V: &str = "│";
const BOX_TR: &str = "┌─";
#[allow(dead_code)]
const BOX_EMPTY: &str = "   ";

/// ANSI color codes
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";
#[allow(dead_code)]
const WHITE: &str = "\x1b[37m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Terminal emitter — format diagnostics untuk console output ala Rust/Cargo.
pub struct TerminalEmitter {
    writer: Box<dyn Write + Send>,
    use_color: bool,
    /// Gunakan format sederhana (tanpa box drawing).
    simple_mode: bool,
}

impl TerminalEmitter {
    pub fn new() -> Self {
        TerminalEmitter {
            writer: Box::new(io::stderr()),
            use_color: atty_is_terminal(),
            simple_mode: false,
        }
    }

    pub fn with_writer(writer: Box<dyn Write + Send>) -> Self {
        TerminalEmitter {
            writer,
            use_color: false,
            simple_mode: true,
        }
    }

    pub fn with_color(mut self, use_color: bool) -> Self {
        self.use_color = use_color;
        self
    }

    pub fn with_simple_mode(mut self, simple: bool) -> Self {
        self.simple_mode = simple;
        self
    }

    /// Warna untuk level tertentu.
    fn level_color(&self, level: DiagLevel) -> &'static str {
        if !self.use_color {
            return "";
        }
        match level {
            DiagLevel::Fatal => MAGENTA,
            DiagLevel::Bug => MAGENTA,
            DiagLevel::Error => RED,
            DiagLevel::Warning => YELLOW,
            DiagLevel::Note => CYAN,
            DiagLevel::Info => BLUE,
            DiagLevel::Help => GREEN,
            DiagLevel::Trace => DIM,
            DiagLevel::Debug => DIM,
        }
    }

    /// Emit a single diagnostic.
    pub fn emit(&mut self, diag: &Diagnostic) -> io::Result<()> {
        if self.simple_mode {
            self.emit_plain(diag)
        } else if self.use_color {
            self.emit_rich_colored(diag)
        } else {
            self.emit_rich_plain(diag)
        }
    }

    /// ── Rich output dengan warna ──
    fn emit_rich_colored(&mut self, diag: &Diagnostic) -> io::Result<()> {
        let level_color = self.level_color(diag.level);

        // Header: level[code]: message (tanpa separator atas/bawah)
        write!(self.writer, "{}", level_color)?;
        write!(self.writer, "{}{}", BOLD, diag.level)?;
        write!(self.writer, "{}", RESET)?;
        write!(self.writer, "[{}", diag.code)?;
        write!(self.writer, "{}", level_color)?;
        write!(self.writer, "]{}: {}{}", RESET, BOLD, diag.message)?;
        writeln!(self.writer, "{}", RESET)?;
        writeln!(self.writer)?;

        // Source snippet — render kanonik (SourceSnippet::format), box cyan +
        // caret merah. Logika caret/label terpusat di satu tempat (Satu-satunya
        // sumber col:line); jangan render ulang di sini.
        if let Some(snippet) = &diag.source_snippet {
            write!(self.writer, "{}", snippet.format(true))?;
            writeln!(self.writer)?;
        }

        // Spans — hanya fallback bila TIDAK ada source snippet (span kini
        // selalu ikut membawa file_id+baris; render dua-duanya = lokasi ganda)
        if diag.source_snippet.is_none() {
            for span in &diag.spans {
                writeln!(
                    self.writer,
                    "   {}{}-->{} {}:{}:{}",
                    BLUE, BOX_TR, RESET, span.file, span.start, span.end
                )?;
                if let Some(label) = &span.label {
                    writeln!(self.writer, "   {}     {}{}", BLUE, label, RESET)?;
                }
            }
        }

        // Runtime context — format: = key : value (satu baris)
        if let Some(ctx) = &diag.runtime_context {
            let ctx_str = ctx.format();
            if !ctx_str.is_empty() {
                for line in ctx_str.lines() {
                    if line.starts_with("  •") {
                        let content = line.trim_start_matches("  • ");
                        if let Some((key, value)) = content.split_once(": ") {
                            writeln!(
                                self.writer,
                                "   {}= {}{} : {}{}",
                                DIM, CYAN, key, RESET, value
                            )?;
                        }
                    }
                }
                writeln!(self.writer)?;
            }
        }

        // Explanation
        if let Some(explanation) = &diag.explanation {
            writeln!(self.writer, "{}{}Explanation{}", BOLD, CYAN, RESET)?;
            writeln!(self.writer)?;
            writeln!(self.writer, "   {}", explanation)?;
            writeln!(self.writer)?;
        }

        // Notes
        for note in &diag.notes {
            writeln!(self.writer, "   {}note:{} {}", CYAN, RESET, note.message)?;
        }

        // Help / suggestion
        if let Some(suggestion) = &diag.suggestion {
            writeln!(self.writer)?;
            writeln!(self.writer, "   {}help:{} {}", GREEN, RESET, suggestion)?;
        }

        // Hints
        for hint in &diag.hints {
            writeln!(self.writer, "   {}help:{} {}", GREEN, RESET, hint)?;
        }

        // Fix-it hints
        if !diag.fix_its.is_empty() {
            writeln!(self.writer)?;
            writeln!(self.writer, "{}Fix-it suggestions:{}", BOLD, RESET)?;
            for fix_it in &diag.fix_its {
                writeln!(
                    self.writer,
                    "   {}fix-it:{} {} ({}:{}:{}-{}:{})",
                    MAGENTA,
                    RESET,
                    fix_it.description,
                    fix_it.file,
                    fix_it.start_line,
                    fix_it.start_col,
                    fix_it.end_line,
                    fix_it.end_col
                )?;
                if !fix_it.replacement.is_empty() {
                    writeln!(
                        self.writer,
                        "       {}-> '{}'{}",
                        CYAN,
                        fix_it.replacement.escape_debug(),
                        RESET
                    )?;
                } else {
                    writeln!(self.writer, "       {}delete{}", CYAN, RESET)?;
                }
            }
        }

        Ok(())
    }

    /// ── Rich output tanpa warna (plain text) ──
    fn emit_rich_plain(&mut self, diag: &Diagnostic) -> io::Result<()> {
        // Header: level[code]: message (tanpa separator atas/bawah)
        writeln!(
            self.writer,
            "{}[{}]: {}",
            diag.level, diag.code, diag.message
        )?;
        writeln!(self.writer)?;

        // Source snippet — render kanonik (SourceSnippet::format) tanpa warna.
        if let Some(snippet) = &diag.source_snippet {
            write!(self.writer, "{}", snippet.format(false))?;
            writeln!(self.writer)?;
        }

        // Spans — fallback bila tidak ada source snippet
        if diag.source_snippet.is_none() {
            for span in &diag.spans {
                writeln!(
                    self.writer,
                    "   {} {}:{}:{}",
                    BOX_TR, span.file, span.start, span.end
                )?;
                if let Some(label) = &span.label {
                    writeln!(self.writer, "   |    {}", label)?;
                }
            }
        }

        // Runtime context — format: = key : value (satu baris)
        if let Some(ctx) = &diag.runtime_context {
            let ctx_str = ctx.format();
            if !ctx_str.is_empty() {
                for line in ctx_str.lines() {
                    if line.starts_with("  •") {
                        let content = line.trim_start_matches("  • ");
                        if let Some((key, value)) = content.split_once(": ") {
                            writeln!(self.writer, "   = {} : {}", key, value)?;
                        }
                    }
                }
                writeln!(self.writer)?;
            }
        }

        // Explanation
        if let Some(explanation) = &diag.explanation {
            writeln!(self.writer, "Explanation")?;
            writeln!(self.writer)?;
            writeln!(self.writer, "   {}", explanation)?;
            writeln!(self.writer)?;
        }

        // Notes
        for note in &diag.notes {
            writeln!(self.writer, "   = note: {}", note.message)?;
        }

        // Help / suggestion
        if let Some(suggestion) = &diag.suggestion {
            writeln!(self.writer, "   = help: {}", suggestion)?;
        }

        // Hints
        for hint in &diag.hints {
            writeln!(self.writer, "   = help: {}", hint)?;
        }

        // Fix-it hints
        if !diag.fix_its.is_empty() {
            writeln!(self.writer)?;
            writeln!(self.writer, "Fix-it suggestions:")?;
            for fix_it in &diag.fix_its {
                writeln!(
                    self.writer,
                    "   = fix-it: {} ({}:{}:{}-{}:{})",
                    fix_it.description,
                    fix_it.file,
                    fix_it.start_line,
                    fix_it.start_col,
                    fix_it.end_line,
                    fix_it.end_col
                )?;
                if !fix_it.replacement.is_empty() {
                    writeln!(
                        self.writer,
                        "       -> '{}'",
                        fix_it.replacement.escape_debug()
                    )?;
                } else {
                    writeln!(self.writer, "       (delete)")?;
                }
            }
        }

        Ok(())
    }

    /// ── Plain output (tanpa box drawing, untuk non-terminal) ──
    fn emit_plain(&mut self, diag: &Diagnostic) -> io::Result<()> {
        if self.use_color {
            let level_color = self.level_color(diag.level);
            writeln!(
                self.writer,
                "{}{}{}[{}]: {}",
                level_color, diag.level, RESET, diag.code, diag.message
            )?;
        } else {
            writeln!(
                self.writer,
                "{}[{}]: {}",
                diag.level, diag.code, diag.message
            )?;
        }

        for span in &diag.spans {
            if diag.source_snippet.is_some() {
                continue; // snippet menampilkan lokasi — span tak perlu diulang
            }
            if self.use_color {
                write!(
                    self.writer,
                    "  {}{}-->{} {}:{}:{}",
                    BLUE, BOX_TR, RESET, span.file, span.start, span.end
                )?;
            } else {
                write!(
                    self.writer,
                    "  --> {}:{}:{}",
                    span.file, span.start, span.end
                )?;
            }
            if let Some(label) = &span.label {
                write!(self.writer, " — {}", label)?;
            }
            writeln!(self.writer)?;
        }

        // Source snippet (format sederhana)
        if let Some(snippet) = &diag.source_snippet {
            writeln!(self.writer, "  --> {}:{}", snippet.file, snippet.line)?;
            writeln!(self.writer, "   |")?;
            writeln!(self.writer, "{:>4} | {}", snippet.line, snippet.source_line)?;
            write!(self.writer, "     | ")?;
            for _ in 1..snippet.col {
                write!(self.writer, " ")?;
            }
            writeln!(self.writer, "^")?;
        }

        // Runtime context (format sederhana)
        if let Some(ctx) = &diag.runtime_context {
            let ctx_str = ctx.format();
            if !ctx_str.is_empty() {
                for line in ctx_str.lines() {
                    if line.starts_with("  •") {
                        writeln!(self.writer, "  = {}", line.trim_start_matches("  • "))?;
                    }
                }
            }
        }

        // Notes
        for note in &diag.notes {
            if self.use_color {
                writeln!(self.writer, "  {}note:{} {}", CYAN, RESET, note.message)?;
            } else {
                writeln!(self.writer, "  = note: {}", note.message)?;
            }
        }

        // Hints
        for hint in &diag.hints {
            if self.use_color {
                writeln!(self.writer, "  {}help:{} {}", GREEN, RESET, hint)?;
            } else {
                writeln!(self.writer, "  = help: {}", hint)?;
            }
        }

        // Fix-it hints
        if !diag.fix_its.is_empty() {
            writeln!(self.writer)?;
            if self.use_color {
                writeln!(self.writer, "{}Fix-it suggestions:{}", BOLD, RESET)?;
            } else {
                writeln!(self.writer, "Fix-it suggestions:")?;
            }
            for fix_it in &diag.fix_its {
                writeln!(
                    self.writer,
                    "   = fix-it: {} ({}:{}:{}-{}:{})",
                    fix_it.description,
                    fix_it.file,
                    fix_it.start_line,
                    fix_it.start_col,
                    fix_it.end_line,
                    fix_it.end_col
                )?;
                if !fix_it.replacement.is_empty() {
                    writeln!(
                        self.writer,
                        "       -> '{}'",
                        fix_it.replacement.escape_debug()
                    )?;
                } else {
                    writeln!(self.writer, "       (delete)")?;
                }
            }
        }

        Ok(())
    }

    /// Emit all diagnostics from a sink.
    pub fn emit_all(&mut self, sink: &DiagSink) -> io::Result<usize> {
        let diags = sink.diagnostics();
        let count = diags.len();
        for diag in &diags {
            self.emit(diag)?;
        }
        Ok(count)
    }

    /// Emit all diagnostics with a summary footer.
    pub fn emit_with_summary(&mut self, sink: &DiagSink) -> io::Result<usize> {
        let diags = sink.diagnostics();
        let count = diags.len();
        let error_count = diags.iter().filter(|d| d.is_error()).count();
        let warning_count = diags
            .iter()
            .filter(|d| d.level == DiagLevel::Warning)
            .count();
        let note_count = diags
            .iter()
            .filter(|d| d.level == DiagLevel::Note || d.level == DiagLevel::Info)
            .count();

        for diag in &diags {
            self.emit(diag)?;
        }

        // Summary footer
        if count > 0 && !self.simple_mode {
            writeln!(self.writer)?;

            if self.use_color {
                if error_count > 0 {
                    writeln!(
                        self.writer,
                        "{}{}{} error(s), {}{}{} warning(s), {} note(s){}",
                        RED, error_count, RESET, YELLOW, warning_count, RESET, note_count, RESET
                    )?;
                } else if warning_count > 0 {
                    writeln!(
                        self.writer,
                        "{} warning(s), {} note(s)",
                        warning_count, note_count
                    )?;
                } else {
                    writeln!(self.writer, "{} note(s)", note_count)?;
                }
            } else {
                writeln!(
                    self.writer,
                    "{} error(s), {} warning(s), {} note(s)",
                    error_count, warning_count, note_count
                )?;
            }
        }

        Ok(count)
    }
}

impl Default for TerminalEmitter {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if stderr is a terminal (for color support).
fn atty_is_terminal() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = std::io::stderr().as_raw_fd();
        libc_isatty(fd) != 0
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Minimal isatty wrapper (avoid libc dependency).
#[cfg(unix)]
fn libc_isatty(fd: i32) -> i32 {
    unsafe extern "C" {
        fn isatty(fd: i32) -> i32;
    }
    unsafe { isatty(fd) }
}

/// Format diagnostic sebagai string (untuk logging/testing).
pub fn format_diagnostic(diag: &Diagnostic) -> String {
    let mut output = String::new();

    // Header (tanpa separator atas/bawah)
    output.push_str(&format!(
        "{}[{}]: {}\n\n",
        diag.level, diag.code, diag.message
    ));

    // Source snippet — render kanonik (SourceSnippet::format) tanpa warna.
    // Logika caret terpusat di satu tempat; jangan render ulang di sini.
    if let Some(snippet) = &diag.source_snippet {
        output.push_str(&snippet.format(false));
    }

    // Spans — fallback bila tidak ada source snippet (span membawa file_id;
    // render keduanya = lokasi ganda)
    if diag.source_snippet.is_none() {
        for span in &diag.spans {
            output.push_str(&format!(
                "   {} {}:{}:{}",
                BOX_TR, span.file, span.start, span.end
            ));
            if let Some(label) = &span.label {
                output.push_str(&format!(" — {}", label));
            }
            output.push('\n');
        }
    }

    // Runtime context — format: = key : value (satu baris)
    if let Some(ctx) = &diag.runtime_context {
        let ctx_str = ctx.format();
        if !ctx_str.is_empty() {
            for line in ctx_str.lines() {
                if line.starts_with("  •") {
                    let content = line.trim_start_matches("  • ");
                    if let Some((key, value)) = content.split_once(": ") {
                        output.push_str(&format!("  = {} : {}\n", key, value));
                    }
                }
            }
            output.push('\n');
        }
    }

    // Explanation
    if let Some(explanation) = &diag.explanation {
        output.push_str(&format!("Explanation\n\n   {}\n\n", explanation));
    }

    // Notes
    for note in &diag.notes {
        output.push_str(&format!("  = note: {}\n", note.message));
    }

    // Help / suggestion
    if let Some(suggestion) = &diag.suggestion {
        output.push_str(&format!("  = help: {}\n", suggestion));
    }

    // Hints
    for hint in &diag.hints {
        output.push_str(&format!("  = help: {}\n", hint));
    }

    output
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::diagnostic::{DiagCode, DiagSpan, RuntimeContext, SourceSnippet};
    use crate::intern::Symbol;

    #[test]
    fn test_format_diagnostic_legacy() {
        let file = Symbol::intern("test.sv");
        let d = Diagnostic::error(DiagCode::UnexpectedToken, "found 'foo' where ';' expected")
            .with_span(DiagSpan::new(file, 10, 13).with_label("here"))
            .with_note("try adding a semicolon");

        let output = format_diagnostic(&d);
        assert!(output.contains("E1001"));
        assert!(output.contains("found 'foo'"));
        assert!(output.contains("test.sv:10:13"));
        assert!(output.contains("note: try adding"));
    }

    #[test]
    fn test_format_runtime_error() {
        let snippet = SourceSnippet::new("cpu_top.mr", 182, 17, "    axi.read(addr);")
            .with_label("interface is null");
        let ctx = RuntimeContext::new()
            .with_module("cpu_top")
            .with_process("fetch_stage")
            .with_time("125 ns")
            .with_thread("worker-2");
        let d = Diagnostic::error(DiagCode::NullInterface, "Null interface dereference")
            .with_source_snippet(snippet)
            .with_runtime_context(ctx)
            .with_explanation("The AXI interface was never connected before simulation started.")
            .with_suggestion("Initialize the interface before using it.");

        let output = format_diagnostic(&d);
        assert!(output.contains("RT0003"));
        assert!(output.contains("Null interface dereference"));
        assert!(output.contains("cpu_top.mr:182:17"));
        assert!(output.contains("axi.read(addr);"));
        assert!(output.contains("interface is null"));
        assert!(output.contains("cpu_top"));
        assert!(output.contains("125 ns"));
        assert!(output.contains("AXI interface was never connected"));
        assert!(output.contains("Initialize the interface"));
    }

    #[test]
    fn test_format_warning() {
        let snippet = SourceSnippet::new("alu.mr", 92, 8, "    result = reg_a + reg_b;")
            .with_label("Register \"reg_a\" was never initialized");
        let d = Diagnostic::warning(DiagCode::UninitializedRegister, "Uninitialized register")
            .with_source_snippet(snippet)
            .with_suggestion("Assign a reset value.");

        let output = format_diagnostic(&d);
        assert!(output.contains("WR0014"));
        assert!(output.contains("Uninitialized register"));
        assert!(output.contains("Assign a reset value."));
    }

    #[test]
    fn test_format_fatal() {
        let d = Diagnostic::fatal(DiagCode::InfiniteDelta, "Infinite delta cycle")
            .with_explanation("Simulation cannot advance past the current time step.")
            .with_note("This usually indicates a zero-delay combinational loop.");

        let output = format_diagnostic(&d);
        assert!(output.contains("fatal"));
        assert!(output.contains("RT2001"));
        assert!(output.contains("Infinite delta cycle"));
        assert!(output.contains("zero-delay combinational loop"));
    }

    #[test]
    fn test_terminal_emitter_plain() {
        let d = Diagnostic::warning(DiagCode::WidthMismatch, "width differs");
        let output = format_diagnostic(&d);
        assert!(output.contains("warning"));
        assert!(output.contains("E2003"));
    }

    #[test]
    fn test_emitter_with_summary() {
        let sink = DiagSink::new();
        sink.push(Diagnostic::error(DiagCode::NullHandle, "null handle"));
        sink.push(Diagnostic::warning(
            DiagCode::UninitializedRegister,
            "uninit reg",
        ));
        sink.push(Diagnostic::note("test note"));

        let mut emitter = TerminalEmitter::new()
            .with_color(false)
            .with_simple_mode(false);
        let count = emitter.emit_with_summary(&sink).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_emitter_colored() {
        let snippet = SourceSnippet::new("test.sv", 10, 5, "    signal = 1'b0;");
        let d = Diagnostic::error(
            DiagCode::SignalUninitialized,
            "Signal read before assignment",
        )
        .with_source_snippet(snippet)
        .with_explanation("Signal was read but never assigned a value.")
        .with_suggestion("Initialize signal in the reset block.");

        let output = format_diagnostic(&d);
        assert!(output.contains("RT1005"));
        assert!(output.contains("Signal read before assignment"));
        assert!(output.contains("Initialize signal"));
    }

    /// Col 1-based: caret harus mendarat tepat di kolom yang dilaporkan.
    /// `format_diagnostic` mendelegasikan ke `SourceSnippet::format` yang
    /// memakai prefix `"     │ "` = 7 CHARACTER (│ multi-byte UTF-8), jadi
    /// ukur berbasis karakter, bukan byte (regresi: byte `-7` akan salah).
    fn caret_col_of(snippet: &SourceSnippet) -> usize {
        let d = Diagnostic::error(DiagCode::UnexpectedToken, "test")
            .with_source_snippet(snippet.clone());
        let out = format_diagnostic(&d);
        let caret_line = out
            .lines()
            .find(|l| l.contains('^'))
            .expect("harus ada baris caret");
        caret_line.chars().take_while(|&c| c != '^').count() - 7 + 1
    }

    #[test]
    fn caret_aligns_col_on_multidigit_line() {
        let snippet = SourceSnippet::new("t.sv", 3, 20, "  assign y = bad + ;");
        assert_eq!(caret_col_of(&snippet), 20);
    }

    #[test]
    fn caret_aligns_col_one() {
        let snippet = SourceSnippet::new("t.sv", 3, 1, "module x;");
        assert_eq!(caret_col_of(&snippet), 1);
    }

    #[test]
    fn caret_aligns_col_two_digit_source() {
        let snippet = SourceSnippet::new("t.sv", 12, 6, "      y = a;");
        assert_eq!(caret_col_of(&snippet), 6);
    }
}
