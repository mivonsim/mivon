/// Source code snippet yang menunjukkan lokasi error.
#[derive(Debug, Clone)]
pub struct SourceSnippet {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub source_line: String,
    pub pointer_label: Option<String>,
}

impl SourceSnippet {
    pub fn new(
        file: impl Into<String>,
        line: usize,
        col: usize,
        source_line: impl Into<String>,
    ) -> Self {
        SourceSnippet {
            file: file.into(),
            line,
            col,
            source_line: source_line.into(),
            pointer_label: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.pointer_label = Some(label.into());
        self
    }

    /// Format source snippet dalam gaya Rust.
    pub fn format(&self, use_color: bool) -> String {
        let cyan = if use_color { "\x1b[36m" } else { "" };
        let red = if use_color { "\x1b[31m" } else { "" };
        let reset = if use_color { "\x1b[0m" } else { "" };

        let mut out = String::new();
        // Garis atas
        out.push_str(&format!(
            "   {0}┌─{1} {2}:{3}:{4}\n",
            cyan, reset, self.file, self.line, self.col
        ));
        out.push_str(&format!("   {0}│{1}\n", cyan, reset));

        // Baris source
        out.push_str(&format!("{:>4} │ {}\n", self.line, self.source_line));

        // Pointer — caret harus mendarat di kolom `self.col` (1-based).
        // Prefix `"     │ "` = 7 char, jadi butuh (col-1) spasi agar baris
        // caret sejajar dengan baris source `{:>4} │ ` (prefix 7 char).
        // JANGAN pakai saturating_sub(1): itu = col-2 spasi → caret meleset
        // 1 kolom ke kiri (renderer lain di emitter.rs sudah pakai col-1).
        // Ini SATU-SATUNYA renderer snippet; emitter lain (rich/plain/
        // format_diagnostic/emit_rich_plain) mendelegasikan ke sini supaya
        // logika caret + label tidak terduplikasi 5× (sumber drift col:line).
        out.push_str("     │ ");
        for _ in 1..self.col {
            out.push(' ');
        }
        out.push_str(red);
        out.push('^');

        // Label pointer
        if let Some(label) = &self.pointer_label {
            out.push_str(&format!(" ─── {label}"));
        }
        out.push_str(reset);
        out.push('\n');

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_snippet_with_fields() {
        let s = SourceSnippet::new("foo.sv", 10, 5, "always @(posedge clk) begin");
        assert_eq!(s.file, "foo.sv");
        assert_eq!(s.line, 10);
        assert_eq!(s.col, 5);
        assert_eq!(s.source_line, "always @(posedge clk) begin");
        assert!(s.pointer_label.is_none());
    }

    #[test]
    fn test_with_label_sets_pointer_label() {
        let s = SourceSnippet::new("foo.sv", 10, 5, "always @(posedge clk) begin")
            .with_label("here");
        assert_eq!(s.pointer_label.as_deref(), Some("here"));
    }

    #[test]
    fn test_format_plain_no_color() {
        let s = SourceSnippet::new("foo.sv", 3, 1, "module m;");
        let out = s.format(false);
        assert!(!out.contains("\x1b["), "no ANSI codes when color off: {out:?}");
        assert!(out.contains("foo.sv:3:1"));
        assert!(out.contains("module m;"));
        assert!(out.contains("^"), "caret should be present: {out:?}");
    }

    #[test]
    fn test_format_with_color_uses_ansi() {
        let s = SourceSnippet::new("foo.sv", 3, 1, "module m;");
        let out = s.format(true);
        assert!(out.contains("\x1b[36m"), "cyan header should be present: {out:?}");
        assert!(out.contains("\x1b[31m"), "red caret should be present: {out:?}");
        assert!(out.contains("\x1b[0m"), "reset should be present: {out:?}");
    }

    #[test]
    fn test_caret_placement_at_column_1() {
        // Column 1 → no leading spaces before caret.
        let s = SourceSnippet::new("foo.sv", 1, 1, "module m;");
        let out = s.format(false);
        let caret_line = out.lines().find(|l| l.contains('^')).unwrap();
        assert!(caret_line.starts_with("     │ "));
        let after_prefix: String = caret_line.chars().skip(7).collect(); // skip "     │ " (7 chars)
        let spaces = after_prefix.chars().take_while(|c| *c == ' ').count();
        assert_eq!(spaces, 0); // col - 1 = 0
        assert!(after_prefix.ends_with('^'));
    }

    #[test]
    fn test_caret_placement_at_column_5() {
        // Column 5 → col-1 = 4 leading spaces before caret.
        let s = SourceSnippet::new("foo.sv", 1, 5, "always @clk");
        let out = s.format(false);
        let caret_line = out.lines().find(|l| l.contains('^')).unwrap();
        assert!(caret_line.starts_with("     │ "));
        let after_prefix: String = caret_line.chars().skip(7).collect(); // skip "     │ " (7 chars)
        let spaces = after_prefix.chars().take_while(|c| *c == ' ').count();
        assert_eq!(spaces, 4); // col - 1 = 4
        assert!(after_prefix.ends_with('^'));
    }

    #[test]
    fn test_pointer_label_appended() {
        let s = SourceSnippet::new("foo.sv", 1, 1, "module m;").with_label("error here");
        let out = s.format(false);
        assert!(out.contains(" ─── error here"), "label should be present: {out:?}");
    }

    #[test]
    fn test_format_includes_line_and_col() {
        let s = SourceSnippet::new("foo.sv", 42, 7, "x = a + b;");
        let out = s.format(false);
        assert!(out.contains("foo.sv:42:7"), "line:col should be present: {out:?}");
    }
}
