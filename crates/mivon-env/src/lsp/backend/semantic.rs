//! Semantic tokens (LSP-13): tokenisasi ringan lalu delta-encoded menjadi
//! `SemanticToken` untuk seluruh dokumen maupun range tertentu.
use super::*;

impl LspBackend {
    /// Semantic tokens core (murni, bisa diuji) — LSP-07 tahap 1.
    /// Legend: 0=namespace(module), 1=type(interface), 2=class,
    ///         3=interface, 4=function, 5=variable,
    ///         6=constant(param), 7=keyword.
    /// Modifier (tahap 2d): bit0=declaration (definisi outline),
    ///         bit1=readonly (parameter/konstanta).
    pub(crate) fn compute_semantic_tokens(source: &str) -> Vec<SemanticToken> {
        Self::encode_semantic_tokens(&Self::semantic_raw_tokens(source))
    }

    /// Bit modifier token semantik (urutan = legend token_modifiers).
    pub(crate) const MOD_DECLARATION: u32 = 1;
    pub(crate) const MOD_READONLY: u32 = 2;

    /// LSP-07 tahap 2b: token mentah absolut (line, col, len, legend_idx,
    /// modifiers) sebelum delta-encoding — dipakai full & range agar
    /// filter baris benar (filter SESUDAH encode akan merusak delta).
    pub(crate) fn semantic_raw_tokens(source: &str) -> Vec<(u32, u32, u32, u32, u32)> {
        // Kumpulkan definisi dari outline → (line, col, len, legend_idx, mods).
        let syms = Self::document_symbols(source);
        let mut raw: Vec<(u32, u32, u32, u32, u32)> = Vec::new();

        fn collect(syms: &[DocSymbol], raw: &mut Vec<(u32, u32, u32, u32, u32)>) {
            for s in syms {
                let idx = match s.kind {
                    DocSymbol::KIND_MODULE => 0,
                    DocSymbol::KIND_INTERFACE => 1,
                    DocSymbol::KIND_CLASS => 2,
                    DocSymbol::KIND_PACKAGE => 1,
                    DocSymbol::KIND_FUNCTION => 4,
                    DocSymbol::KIND_CONSTANT => 6,
                    DocSymbol::KIND_VARIABLE => 5,
                    _ => 5,
                };
                let mods = if s.kind == DocSymbol::KIND_CONSTANT {
                    LspBackend::MOD_DECLARATION | LspBackend::MOD_READONLY
                } else {
                    LspBackend::MOD_DECLARATION
                };
                raw.push((s.line, s.col, s.name.len() as u32, idx, mods));
                collect(&s.children, raw);
            }
        }
        collect(&syms, &mut raw);

        // Tambah keyword SV sebagai token type=7 (keyword).
        const KW: &[&str] = &[
            "module",
            "endmodule",
            "always_comb",
            "always_ff",
            "assign",
            "begin",
            "case",
            "default",
            "else",
            "end",
            "endcase",
            "if",
            "input",
            "logic",
            "output",
            "parameter",
            "reg",
            "wire",
            "localparam",
            "function",
            "task",
            "endfunction",
            "endtask",
            "interface",
            "endinterface",
            "package",
            "endpackage",
            "typedef",
            "enum",
            "struct",
            "posedge",
            "negedge",
        ];
        for (idx, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for (w, col) in Self::line_tokens(line) {
                if KW.contains(&w.as_str()) {
                    raw.push((idx as u32, col as u32, w.len() as u32, 7, 0));
                }
            }
        }

        // Urutkan by (line, col).
        raw.sort_by_key(|&(l, c, _, _, _)| (l, c));
        raw
    }

    /// LSP-07 tahap 2b: delta-encoding LSP dari token mentah absolut.
    pub(crate) fn encode_semantic_tokens(raw: &[(u32, u32, u32, u32, u32)]) -> Vec<SemanticToken> {
        // Konversi ke delta encoding LSP.
        let mut out: Vec<SemanticToken> = Vec::with_capacity(raw.len());
        let mut prev_line = 0u32;
        let mut prev_col = 0u32;
        for &(l, c, len, typ, mods) in raw {
            let dl = l - prev_line;
            let ds = if l == prev_line { c - prev_col } else { c };
            out.push(SemanticToken {
                delta_line: dl,
                delta_start: ds,
                length: len,
                token_type: typ,
                token_modifiers_bitset: mods,
            });
            prev_line = l;
            prev_col = c;
        }
        out
    }

    /// LSP-07 tahap 2c: diff prefix/suffix dua deret token → edit
    /// minimal (0-1 edit; kosong = identik). Murni, bisa diuji.
    pub(crate) fn semantic_tokens_delta(
        prev: &[SemanticToken],
        next: &[SemanticToken],
    ) -> Vec<SemanticTokensEdit> {
        let mut prefix = 0usize;
        while prefix < prev.len() && prefix < next.len() && prev[prefix] == next[prefix] {
            prefix += 1;
        }
        let mut suffix = 0usize;
        while suffix < prev.len() - prefix
            && suffix < next.len() - prefix
            && prev[prev.len() - 1 - suffix] == next[next.len() - 1 - suffix]
        {
            suffix += 1;
        }
        if prefix + suffix == prev.len() && prefix + suffix == next.len() {
            return Vec::new();
        }
        vec![SemanticTokensEdit {
            start: prefix as u32,
            delete_count: (prev.len() - prefix - suffix) as u32,
            data: Some(next[prefix..next.len() - suffix].to_vec()),
        }]
    }

    /// LSP-07 tahap 2b: token semantik untuk rentang baris inklusif
    /// [start_line, end_line] — filter SEBELUM encode agar delta valid.
    /// `start > end` → kosong.
    pub(crate) fn semantic_tokens_in_range(
        source: &str,
        start_line: u32,
        end_line: u32,
    ) -> Vec<SemanticToken> {
        if start_line > end_line {
            return Vec::new();
        }
        let raw: Vec<(u32, u32, u32, u32, u32)> = Self::semantic_raw_tokens(source)
            .into_iter()
            .filter(|(l, _, _, _, _)| *l >= start_line && *l <= end_line)
            .collect();
        Self::encode_semantic_tokens(&raw)
    }
}
