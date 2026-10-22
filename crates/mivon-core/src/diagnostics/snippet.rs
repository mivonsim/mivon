use std::borrow::Cow;

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
