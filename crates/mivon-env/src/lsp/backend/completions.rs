//! Completion (LSP-09): konteks baris, snippet keyword, detail item, dan
//! `resolve` untuk dokumentasi tambahan.
use super::*;

/// Offset byte untuk indeks karakter ke-char_idx (di-clamp ke akhir baris).
///
/// Klien mengirim kolom sebagai indeks karakter; memakainya langsung sebagai
/// indeks byte membuat potongan &str panic di tengah karakter multi-byte
/// (mis. komentar ber-UTF-8 sebelum caret) dan prefix terhitung salah.
pub(super) fn char_index_to_byte(line: &str, char_idx: usize) -> usize {
    line.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(line.len())
}

impl LspBackend {
    /// LSP-02 tahap 2a: snippet blok untuk keyword struktural.
    /// Return body snippet LSP (`$1` = tab stop pertama) atau None.
    pub(crate) fn snippet_for_keyword(kw: &str) -> Option<&'static str> {
        Some(match kw {
            "module" => "module $1;\n\t$0\nendmodule",
            "interface" => "interface $1;\n\t$0\nendinterface",
            "package" => "package $1;\n\t$0\nendpackage",
            "function" => "function $1;\n\t$0\nendfunction",
            "task" => "task $1;\n\t$0\nendtask",
            "begin" => "begin\n\t$0\nend",
            "fork" => "fork\n\t$0\njoin",
            "case" => "case ($1)\n\tdefault: $0\nendcase",
            "if" => "if ($1) begin\n\t$0\nend",
            "always_ff" => "always_ff @(posedge $1) begin\n\t$0\nend",
            "always_comb" => "always_comb begin\n\t$0\nend",
            "for" => "for ($1) begin\n\t$0\nend",
            "foreach" => "foreach ($1) begin\n\t$0\nend",
            _ => return None,
        })
    }

    /// LSP-02 tahap 2c: label detail singkat per kind (ditampilkan di
    /// samping kandidat). `has_snippet` menandai kandidat blok snippet.
    pub(crate) fn completion_detail(kind: CompletionItemKind, has_snippet: bool) -> Option<String> {
        let base = match kind {
            CompletionItemKind::MODULE => "module",
            CompletionItemKind::FUNCTION => "function",
            CompletionItemKind::CLASS => "class",
            CompletionItemKind::INTERFACE => "interface",
            CompletionItemKind::KEYWORD => "keyword",
            CompletionItemKind::VARIABLE => "signal",
            CompletionItemKind::CONSTANT => "parameter",
            _ => return None,
        };
        Some(if has_snippet {
            format!("{base} · snippet")
        } else {
            base.to_string()
        })
    }

    /// LSP-02 tahap 2c: fallback detail saat resolve — pertahankan bila
    /// sudah ada, isi via completion_detail bila kosong.
    pub(crate) fn resolve_detail(
        existing: Option<String>,
        kind: Option<CompletionItemKind>,
        format: Option<InsertTextFormat>,
    ) -> Option<String> {
        if existing.is_some() {
            return existing;
        }
        let k = kind?;
        Self::completion_detail(k, format == Some(InsertTextFormat::SNIPPET))
    }

    /// LSP-02 tahap 2b: konteks baris kursor — "port" (deklarasi
    /// arah), "param" (daftar `#(...)` / parameter), atau "code".
    pub(crate) fn completion_context(line_text: &str) -> &'static str {
        let t = line_text.trim_start().to_lowercase();
        let first: &str = t
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches([':', ';', ',']);
        if matches!(first, "input" | "output" | "inout") {
            return "port";
        }
        if t.contains("#(") || matches!(first, "parameter" | "localparam") {
            return "param";
        }
        "code"
    }

    /// Autocomplete core (murni, bisa diuji) — LSP-02 tahap 1:
    /// kandidat = simbol dokumen (module/port/signal/param/function via
    /// outline) + keyword SystemVerilog umum, difilter prefix identifier
    /// di kursor. Return (label, kind) dengan kind DocSymbol::KIND_*;
    /// keyword memakai KIND_KEYWORD.
    pub(crate) fn compute_completions(
        source: &str,
        line: u32,
        character: u32,
    ) -> Vec<(String, u8)> {
        const KEYWORDS: &[&str] = &[
            "always",
            "always_comb",
            "always_ff",
            "always_latch",
            "assign",
            "begin",
            "case",
            "casex",
            "casez",
            "class",
            "clocking",
            "default",
            "disable",
            "else",
            "end",
            "endcase",
            "endclass",
            "endfunction",
            "endgenerate",
            "endinterface",
            "endmodule",
            "endpackage",
            "endtask",
            "enum",
            "for",
            "forever",
            "fork",
            "function",
            "generate",
            "genvar",
            "if",
            "iff",
            "import",
            "initial",
            "inout",
            "input",
            "int",
            "integer",
            "interface",
            "join",
            "logic",
            "localparam",
            "longint",
            "macromodule",
            "modport",
            "module",
            "negedge",
            "or",
            "output",
            "package",
            "parameter",
            "posedge",
            "primitive",
            "priority",
            "program",
            "property",
            "reg",
            "repeat",
            "return",
            "sequence",
            "shortint",
            "signed",
            "string",
            "struct",
            "time",
            "typedef",
            "unique",
            "unsigned",
            "wait",
            "while",
            "wire",
            "with",
        ];

        let line_text = source.lines().nth(line as usize).unwrap_or("");
        // Prefix = karakter identifier yang sudah diketik sebelum kursor.
        let bytes = line_text.as_bytes();
        let ch = char_index_to_byte(line_text, character as usize);
        let mut start = ch;
        while start > 0
            && (bytes[start - 1].is_ascii_alphanumeric()
                || bytes[start - 1] == b'_'
                || bytes[start - 1] == b'$')
        {
            start -= 1;
        }
        let prefix = line_text[start..ch].to_lowercase();
        if prefix.is_empty() {
            return Vec::new();
        }

        let mut out: Vec<(String, u8)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Simbol dokumen dulu (lebih relevan).
        fn walk(
            items: &[DocSymbol],
            prefix: &str,
            out: &mut Vec<(String, u8)>,
            seen: &mut std::collections::HashSet<String>,
        ) {
            for s in items {
                if s.name.to_lowercase().starts_with(prefix) && seen.insert(s.name.clone()) {
                    out.push((s.name.clone(), s.kind));
                }
                walk(&s.children, prefix, out, seen);
            }
        }
        walk(
            &Self::document_symbols(source),
            &prefix,
            &mut out,
            &mut seen,
        );

        // Keyword SystemVerilog.
        for kw in KEYWORDS {
            if kw.starts_with(&prefix) && seen.insert((*kw).to_string()) {
                out.push(((*kw).to_string(), DocSymbol::KIND_KEYWORD));
            }
        }
        // LSP-02 tahap 2b: ranking context-aware — di konteks port/param,
        // keyword tipe didahulukan (simbol tetap rank 0 agar nama interface
        // / tipe user tidak tenggelam).
        const TYPES: &[&str] = &[
            "logic", "bit", "reg", "wire", "int", "integer", "byte", "shortint", "longint",
            "string", "signed", "unsigned",
        ];
        let ctx = Self::completion_context(line_text);
        let rank = |name: &str, kind: u8| -> u8 {
            if kind != DocSymbol::KIND_KEYWORD || ctx == "code" {
                return 0;
            }
            if TYPES.contains(&name) {
                return 0;
            }
            if ctx == "param" && (name == "parameter" || name == "localparam") {
                return 0;
            }
            1
        };
        out.sort_by(|a, b| rank(&a.0, a.1).cmp(&rank(&b.0, b.1)).then(a.0.cmp(&b.0)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_index_to_byte_handles_multibyte_and_clamps() {
        assert_eq!(char_index_to_byte("abc", 2), 2);
        assert_eq!(char_index_to_byte("é", 1), 2, "é = 2 byte");
        assert_eq!(
            char_index_to_byte("é", 9),
            2,
            "kolom melewati baris di-clamp"
        );
        assert_eq!(char_index_to_byte("", 3), 0);
    }

    #[test]
    fn completions_do_not_panic_when_column_lands_mid_character() {
        // Baris 0 = "é" (1 karakter, 2 byte). Kolom karakter 1 = byte 2;
        // memakai kolom sebagai indeks byte membuat potongan &str mulai di
        // tengah karakter → panic. Dengan konversi benar prefix kosong.
        let src = "é\nmodule m;\n  logic clk;\nendmodule\n";
        let items = LspBackend::compute_completions(src, 0, 1);
        assert!(items.is_empty(), "tidak ada prefix identifier: {items:?}");
    }

    #[test]
    fn completions_prefix_after_multibyte_line() {
        // Baris 0 = "// é clk" (8 karakter, 9 byte). Kolom karakter 8 harus
        // menghasilkan prefix "clk" — memakai kolom sebagai indeks byte
        // membuat potongan str panic / prefix terpotong.
        let src = "// é clk
module m;
  logic clk;
endmodule
";
        let items = LspBackend::compute_completions(src, 0, 8);
        // Prefix harus tepat "clk" — prefix "cl" (kolom dipakai sebagai byte)
        // juga akan memuat keyword class/clocking, jadi daftar dikunci persis.
        let names: Vec<&str> = items.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["clk"], "prefix clk: {names:?}");
    }
}
