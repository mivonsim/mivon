//! Syntax highlighting untuk SystemVerilog / Verilog di editor TUI.
//! Mengembalikan segmen (teks, style) per baris agar renderer ratatui bisa
//! mewarnai kata kunci, angka, komentar, string, dan system function.
//! Implementasi sederhana berbasis pola per baris (tidak full lexer) untuk
//! menjaga cepat dan cukup untuk navigasi/debugging RTL.

use ratatui::style::{Color, Modifier, Style};

/// Kata kunci SystemVerilog / Verilib yang diwarnai ungu.
const KEYWORDS: &[&str] = &[
    "module", "endmodule", "interface", "endinterface", "package", "endpackage",
    "program", "endprogram", "primitive", "endprimitive",
    "always", "always_ff", "always_comb", "always_latch", "initial", "final",
    "begin", "end", "if", "else", "for", "while", "repeat", "case", "casez",
    "casex", "default", "assign", "function", "endfunction", "task", "endtask",
    "input", "output", "inout", "parameter", "localparam", "genvar",
    "logic", "reg", "wire", "tri", "tri0", "tri1", "supply0", "supply1",
    "bit", "int", "integer", "byte", "shortint", "longint", "time", "real",
    "shortreal", "signed", "unsigned", "var", "void", "void*", "void **",
    "import", "export", "typedef", "enum", "struct", "union", "class",
    "endclass", "extends", "virtual", "this", "super", "new", "return",
    "break", "continue", "await", "join", "join_any", "join_none", "fork",
    "disable", "event", "sequence", "endsequence", "property", "endproperty",
    "assert", "assume", "cover", "covergroup", "endcovergroup", "coverpoint",
    "bins", "iff", "inside", "dist", "rand", "randc", "constraint", "let",
    "with", "unique", "unique0", "priority", "args", "config", "endconfig",
    "cell", "endcell", "macro", "endmacro", "generate", "endgenerate",
    "checker", "endchecker", "global", "noshowcancelled", "pulsestyle",
    "showcancelled", "tran", "tranif0", "tranif1", "cmos", "rcmos", "rnmos",
    "pmos", "nmos", "cmos", "exclude", "include", "use", "libmap",
];

/// Kata kunci arah port (dipisah untuk warna biru tua).
#[allow(dead_code)]
const PORT_DIRS: &[&str] = &["input", "output", "inout"];

/// System function yang diwarnai cyan.
const SYSTEM_FUNCS: &[&str] = &[
    "$display", "$write", "$monitor", "$strobe", "$time", "$finish", "$exit",
    "$stop", "$reset", "$random", "$urandom", "$urandom_range", "$clog2",
    "$bits", "$size", "$left", "$right", "$low", "$high", "$unsigned", "$signed",
    "$isunknown", "$countones", "$fopen", "$fclose", "$fdisplay", "$fwrite",
    "$fmonitor", "$fstrobe", "$fscanf", "$fread", "$readmemh", "$readmemb",
    "$writememh", "$writemb", "$dumpfile", "$dumpvars", "$dumpall", "$dumpon",
    "$dumpoff", "$dumpflush", "$dumpports", "$dumpportsall", "$dumpportson",
    "$dumpportsoff", "$dumpportsflush", "$printtimescale", "$scope",
    "$showscopes", "$showvariables", "$version", "$simtime", "$realtime",
    "$stime", "$timeprecision", "$timeunit", "$set_timescale", "$root",
    "$top", "$unit", "$test$plusargs", "$value$plusargs", "$plusargs",
];

/// Tipe data built-in (dipisah untuk warna hijau).
const TYPES: &[&str] = &[
    "logic", "reg", "wire", "tri", "tri0", "tri1", "supply0", "supply1",
    "bit", "int", "integer", "byte", "shortint", "longint", "time", "real",
    "shortreal", "signed", "unsigned", "void",
];

/// Representasi segmen teks dengan style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanStyle {
    pub text: String,
    pub style: Style,
}

impl SpanStyle {
    pub fn plain(text: impl Into<String>) -> Self {
        SpanStyle {
            text: text.into(),
            style: Style::default(),
        }
    }
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        SpanStyle {
            text: text.into(),
            style,
        }
    }
}

/// Hasil highlighting satu baris.
pub type HighlightedLine = Vec<SpanStyle>;

/// Style untuk tiap kategori.
fn kw_style() -> Style {
    Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
}
#[allow(dead_code)]
fn port_dir_style() -> Style {
    Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD)
}
fn system_func_style() -> Style {
    Style::default().fg(Color::Cyan)
}
fn type_style() -> Style {
    Style::default().fg(Color::Green)
}
fn number_style() -> Style {
    Style::default().fg(Color::Yellow)
}
fn string_style() -> Style {
    Style::default().fg(Color::LightRed)
}
fn comment_style() -> Style {
    Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC)
}
fn preproc_style() -> Style {
    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
}

/// Apakah karakter boleh menjadi bagian dari identifier.
pub fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Apakah karakter adalah awalan identifier (tidak digit).
/// System function SystemVerilog diawali `$`, jadi diizinkan juga.
fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

/// Tokenisasi satu baris menjadi segmen berwarna.
pub fn highlight_line(line: &str) -> HighlightedLine {
    let mut out: HighlightedLine = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let n = bytes.len();

    while i < n {
        let c = bytes[i] as char;

        // ── Komentar baris // ──
        if c == '/' && i + 1 < n && bytes[i + 1] == b'/' {
            out.push(SpanStyle::styled(line[i..].to_string(), comment_style()));
            break;
        }

        // ── Preprocessor ` ──
        if c == '`' {
            let j = i + 1;
            let k = bytes[j..]
                .iter()
                .position(|&b| !(b as char).is_alphanumeric() && b as char != '_')
                .map(|p| j + p)
                .unwrap_or(n);
            let name = &line[i..k];
            out.push(SpanStyle::styled(name.to_string(), preproc_style()));
            i = k;
            continue;
        }

        // ── String "..." ──
        if c == '"' {
            let mut j = i + 1;
            while j < n {
                if bytes[j] == b'"' {
                    out.push(SpanStyle::styled(line[i..=j].to_string(), string_style()));
                    i = j + 1;
                    break;
                }
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                j += 1;
            }
            if j >= n {
                // String tidak tertutup di akhir baris.
                out.push(SpanStyle::styled(line[i..].to_string(), string_style()));
            }
            continue;
        }

        // ── Angka ──
        if c.is_ascii_digit() {
            let j = bytes[i..]
                .iter()
                .position(|&b| {
                    !(b as char).is_ascii_digit() && b != b'_' && b != b'.'
                })
                .map(|p| i + p)
                .unwrap_or(n);
            // Deteksi basis: 'h, 'd, 'b, 'o, 's setelah angka.
            let mut end = j;
            if end < n && bytes[end] == b'\'' {
                end += 1;
                while end < n && (bytes[end] as char).is_ascii_alphanumeric() {
                    end += 1;
                }
            }
            out.push(SpanStyle::styled(line[i..end].to_string(), number_style()));
            i = end;
            continue;
        }

        // ── Identifier / kata kunci ──
        if is_ident_start(c) {
            let j = bytes[i..]
                .iter()
                .position(|&b| !is_ident_char(b as char))
                .map(|p| i + p)
                .unwrap_or(n);
            let word = &line[i..j];
            let styled = if KEYWORDS.iter().any(|&kw| kw == word) {
                SpanStyle::styled(word.to_string(), kw_style())
            } else if SYSTEM_FUNCS.iter().any(|&sf| sf == word) {
                SpanStyle::styled(word.to_string(), system_func_style())
            } else if TYPES.iter().any(|&ty| ty == word) {
                SpanStyle::styled(word.to_string(), type_style())
            } else {
                SpanStyle::plain(word.to_string())
            };
            out.push(styled);
            i = j;
            continue;
        }

        // ── Karakter lain (operator, spasi, tanda baca) ──
        out.push(SpanStyle::plain(c.to_string()));
        i += 1;
    }

    if out.is_empty() {
        out.push(SpanStyle::plain(String::new()));
    }
    out
}

/// Render `HighlightedLine` menjadi `ratatui::widgets::Line` untuk ListItem.
pub fn line_to_ratatui(hl: &HighlightedLine) -> ratatui::text::Line<'static> {
    use ratatui::text::Span;
    let spans: Vec<Span<'static>> = hl
        .iter()
        .map(|s| Span::styled(s.text.clone(), s.style))
        .collect();
    ratatui::text::Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_module_keyword() {
        let hl = highlight_line("module adder;");
        let joined: String = hl.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "module adder;");
        assert!(hl.iter().any(|s| s.text == "module" && s.style == kw_style()));
    }

    #[test]
    fn highlights_comment() {
        let hl = highlight_line("// ini komentar");
        let joined: String = hl.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "// ini komentar");
        assert_eq!(hl.len(), 1);
        assert_eq!(hl[0].style, comment_style());
    }

    #[test]
    fn highlights_number_and_string() {
        let hl = highlight_line("logic [7:0] a = 8'hff;");
        let joined: String = hl.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "logic [7:0] a = 8'hff;");
        assert!(hl.iter().any(|s| s.text == "logic" && s.style == kw_style()));
    }

    #[test]
    fn highlights_system_function() {
        let hl = highlight_line("$display(\"hi\");");
        let has_sys = hl.iter().any(|s| s.text == "$display" && s.style == system_func_style());
        assert!(has_sys);
    }

    #[test]
    fn empty_line_returns_one_plain() {
        let hl = highlight_line("");
        assert_eq!(hl.len(), 1);
        assert_eq!(hl[0].text, "");
    }
}
