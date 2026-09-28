//! Mutasi source SystemVerilog (LangFuzz-style, REDQUEEN-inspired).
//!
//! Semua operasi bekerja pada seed nyata. Operasi byte-safe (char-boundary)
//! untuk hindari panic slicing unicode (temuan fuzz nyata di mvm-fuzz).

use crate::{corpus::Corpus, Rng};

/// REDQUEEN-style input-to-state keywords (SystemVerilog).
pub static KEYWORD_LIST: &[&str] = &[
    "module",
    "endmodule",
    "input",
    "output",
    "inout",
    "wire",
    "reg",
    "logic",
    "bit",
    "integer",
    "real",
    "string",
    "byte",
    "shortint",
    "int",
    "longint",
    "unsigned",
    "signed",
    "parameter",
    "localparam",
    "assign",
    "always",
    "always_ff",
    "always_comb",
    "always_latch",
    "initial",
    "final",
    "begin",
    "end",
    "if",
    "else",
    "case",
    "casez",
    "casex",
    "endcase",
    "default",
    "for",
    "foreach",
    "while",
    "do",
    "forever",
    "repeat",
    "break",
    "continue",
    "return",
    "function",
    "endfunction",
    "task",
    "endtask",
    "class",
    "endclass",
    "extends",
    "implements",
    "interface",
    "endinterface",
    "package",
    "endpackage",
    "import",
    "export",
    "struct",
    "union",
    "enum",
    "typedef",
    "generate",
    "endgenerate",
    "genvar",
    "assert",
    "assume",
    "cover",
    "property",
    "sequence",
    "endproperty",
    "constraint",
    "rand",
    "randc",
    "randomize",
    "fork",
    "join",
    "join_any",
    "join_none",
    "posedge",
    "negedge",
    "clocking",
    "#",
    "and",
    "or",
    "not",
    "xor",
    "xnor",
    "nand",
    "nor",
    // ── System-task/function tokens (REDQUEEN inject — elaborator const-fold
    //    + format-string parser) ──
    "$display",
    "$bits",
    "$clog2",
    "$size",
    "$left",
    "$right",
    "$low",
    "$high",
    "$urandom",
    "$random",
    "$fopen",
    // ── SV-only constructs (parsing robustness) ──
    "foreach",
    "inside",
    "push",
    "pop",
    "dist",
    "with",
    "::",
    "{<<",
    "{>>",
];

pub struct Mutator<'r> {
    rng: &'r mut Rng,
}

impl<'r> Mutator<'r> {
    pub fn new(rng: &'r mut Rng) -> Self {
        Self { rng }
    }

    /// Terapkan 1 operasi mutasi random (19 opsi — 13 lama + 3 baru area
    /// sistem-task `$display`/const-fold `$bits`/streaming `{<<{`).
    pub fn mutate(&mut self, source: &str, corpus: &Corpus) -> String {
        if source.is_empty() {
            return String::new();
        }
        match self.rng.below(19) {
            0 => self.splice_from_corpus(source, corpus),
            1 => self.replace_keyword(source),
            2 => self.delete_chunk(source),
            3 => self.duplicate_chunk(source),
            4 => self.tweak_literal(source),
            5 => self.insert_garbage(source),
            6 => self.remove_line(source),
            7 => self.flip_char(source),
            // ── Area: preprocessor / delay / macro / width ──
            8 => self.insert_directive(source),
            9 => self.dup_ifdef(source),
            10 => self.inject_delay(source),
            11 => self.dup_macro_call(source),
            12 => self.extreme_width(source),
            13 => self.inject_include(source),
            // ── Area baru: sistem-task / const-fold / streaming ──
            14 => self.inject_sysfunc_assign(source),
            15 => self.inject_display_format(source),
            16 => self.inject_stream_assign(source),
            _ => source.to_string(), // no-op
        }
    }

    /// Sisipkan directive preprocessor di posisi acak (`` `ifdef ``+`` `endif ``
    /// tak seimbang / `` `define `` baris tengah).
    fn insert_directive(&mut self, source: &str) -> String {
        let directives = [
            "`ifdef FZ_UNDEF\n`endif\n",
            "`ifndef FZ_UNDEF\n`endif\n",
            "`define FZ_MACRO 32'hDEADBEEF\n",
            "`else\n",
            "`elsif FZ_FLAG\n",
        ];
        let chars: Vec<char> = source.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let at = self.rng.below(chars.len() + 1);
        let d = self.rng.pick(&directives);
        let mut out: String = chars[..at].iter().collect();
        out.push_str(d);
        out.extend(&chars[at..]);
        out
    }

    /// Duplikasikan satu baris `` `ifdef ``/`` `ifndef `` (ifdef ganda tanpa
    /// endif — preprocessor stack stress).
    fn dup_ifdef(&mut self, source: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            out.push_str(line);
            out.push('\n');
            if !done {
                let t = line.trim_start();
                if t.starts_with("`ifdef") || t.starts_with("`ifndef") {
                    out.push_str(line);
                    out.push('\n');
                    done = true;
                }
            }
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }

    /// Sisipkan delay `#N` di depan satu statement (event scheduling stress).
    fn inject_delay(&mut self, source: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            if !done && t.starts_with("assign") && !t.starts_with("assign #") {
                let indent: String = line
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                out.push_str(&format!(
                    "{}assign #1 {};\n",
                    indent,
                    t.trim_end_matches(';')
                ));
                done = true;
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }

    /// Duplikasi panggilan macro/`$display` di akhir baris.
    fn dup_macro_call(&mut self, source: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            out.push_str(line);
            out.push('\n');
            if !done {
                let t = line.trim();
                if t.starts_with('`') && t.ends_with(')') {
                    out.push_str(line);
                    out.push('\n');
                    done = true;
                }
            }
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }

    /// Ekstremisasi lebar literal: `8'hFF` → lebar tak wajar (1, 128, 64'd…).
    fn extreme_width(&mut self, source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut idx = 0usize;
        while idx + 2 < chars.len() {
            if chars[idx].is_ascii_digit() && chars[idx + 1] == '\'' {
                let mut start = idx;
                while start > 0 && chars[start - 1].is_ascii_digit() {
                    start -= 1;
                }
                if start < idx {
                    let widths = [1usize, 128, 512, 4096];
                    let w = self.rng.pick(&widths);
                    let mut out: String = chars[..start].iter().collect();
                    out.push_str(&w.to_string());
                    out.extend(&chars[idx..]);
                    return out;
                }
            }
            idx += 1;
        }
        source.to_string()
    }

    /// Sisipkan `` `include "fz.svh" `` di baris acak (include tak ada).
    fn inject_include(&mut self, source: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            if !done
                && (line.trim_start().starts_with("module")
                    || line.trim_start().starts_with("assign"))
            {
                out.push_str("`include \"fz_undefined_inc.svh\"\n");
                done = true;
            }
            out.push_str(line);
            out.push('\n');
        }
        if done {
            out
        } else {
            format!("`include \"fz_undefined_inc.svh\"\n{source}")
        }
    }

    /// LangFuzz-style splice: ambil potongan seed donor, sisipkan ke source.
    fn splice_from_corpus(&mut self, source: &str, corpus: &Corpus) -> String {
        let Some(donor) = corpus.random_seed(self.rng) else {
            return source.to_string();
        };
        let donor = &donor.text;
        if donor.is_empty() {
            return source.to_string();
        }
        // Potongan karakter (bukan byte) untuk boundary safety
        let src_chars: Vec<char> = source.chars().collect();
        let donor_chars: Vec<char> = donor.chars().collect();
        if src_chars.is_empty() || donor_chars.is_empty() {
            return source.to_string();
        }

        let cut_len = self.rng.below(donor_chars.len().min(source.len()) / 2 + 1);
        if cut_len == 0 {
            return source.to_string();
        }
        let donor_start = self
            .rng
            .below(donor_chars.len() - cut_len.min(donor_chars.len()));
        let frag: String = donor_chars[donor_start..donor_start + cut_len]
            .iter()
            .collect();

        let insert_at = self.rng.below(src_chars.len());
        let mut out: String = src_chars[..insert_at].iter().collect();
        out.push_str(&frag);
        out.extend(&src_chars[insert_at..]);
        out
    }

    /// Ganti keyword SV dengan keyword lain.
    fn replace_keyword(&mut self, source: &str) -> String {
        let words = KEYWORD_LIST;
        let from = self.rng.pick(words);
        let to = self.rng.pick(words);
        // Word-boundary utk token diawali char "wordish": dulu `find` polos →
        // substring di tengah kata ikut terganti (`for` di `forever` jadi
        // `$left`+`ever` = `$leftever`) → sampah tokenisasi yang tak
        // merepresentasikan cacat sintaks nyata (noise roundtrip MV: output
        // SV `$leftever;` E1002). Token non-word (mis. `#`) tetap bebas.
        let wordish = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        let needs_boundary = from.chars().next().map(wordish).unwrap_or(false);
        let mut out = String::with_capacity(source.len());
        let mut rest = source;
        while let Some(pos) = rest.find(from) {
            let end = pos + from.len();
            let boundary_ok = !needs_boundary || {
                let before_ok = pos == 0 || !wordish(rest[..pos].chars().next_back().unwrap());
                let after_ok = end >= rest.len() || !wordish(rest[end..].chars().next().unwrap());
                before_ok && after_ok
            };
            if boundary_ok {
                out.push_str(&rest[..pos]);
                out.push_str(to);
                rest = &rest[end..];
            } else {
                // Bukan kata utuh — pertahankan teks asli, lanjut setelahnya.
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            }
        }
        out.push_str(rest);
        out
    }

    /// Hapus chunk baris acak.
    fn delete_chunk(&mut self, source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        if lines.len() < 2 {
            return source.to_string();
        }
        let start = self.rng.below(lines.len());
        let end = (start + 1 + self.rng.below(lines.len() - start)).min(lines.len());
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i < start || i >= end {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// Duplikat chunk baris acak.
    fn duplicate_chunk(&mut self, source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        if lines.is_empty() {
            return source.to_string();
        }
        let start = self.rng.below(lines.len());
        let end = (start + 1 + self.rng.below(lines.len() - start)).min(lines.len());
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            out.push_str(line);
            out.push('\n');
            if i == end - 1 {
                // sisipkan salinan chunk di sini
                for line in &lines[start..end] {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// Ubah literal angka — sisipkan nilai ekstrem.
    fn tweak_literal(&mut self, source: &str) -> String {
        let extremes = [
            "'0",
            "'1",
            "'x",
            "'z",
            "32'hffffffff",
            "64'd0",
            "-1",
            "1e308",
            "2**31",
            "18446744073709551615",
        ];
        let (mut num_start, mut num_end) = (0usize, 0usize);
        let mut in_number = false;
        let mut chosen = None;

        for (i, c) in source.char_indices() {
            if c.is_ascii_digit() || c == '\'' || c == 'x' || c == 'h' || c == 'b' || c == '_' {
                if !in_number {
                    num_start = i;
                    in_number = true;
                }
            } else if in_number {
                if self.rng.chance(10) {
                    chosen = Some((num_start, i));
                    break;
                }
                in_number = false;
            }
            num_end = i;
        }
        if in_number && chosen.is_none() && self.rng.chance(10) {
            chosen = Some((num_start, num_end + 1));
        }

        let Some((s, e)) = chosen else {
            return source.to_string();
        };
        let mut out = String::with_capacity(source.len() + 8);
        out.push_str(&source[..s]);
        out.push_str(self.rng.pick(&extremes));
        out.push_str(&source[e..]);
        out
    }

    /// Sisipkan sampah (garbage) di posisi acak.
    fn insert_garbage(&mut self, source: &str) -> String {
        let garbage = [
            "{",
            "::",
            "&&&",
            "'''",
            "é中",
            "@@@",
            "~~~",
            "/*",
            "*/",
            "`define X Y",
            "`include \"fz.sv\"",
            "endmodule",
            "begin end",
            "0x",
            "##1",
        ];
        let chars: Vec<char> = source.chars().collect();
        if chars.is_empty() {
            return source.to_string();
        }
        let at = self.rng.below(chars.len() + 1);
        let g = self.rng.pick(&garbage);
        let mut out: String = chars[..at].iter().collect();
        out.push_str(g);
        out.extend(&chars[at..]);
        out
    }

    /// Hapus satu baris acak.
    fn remove_line(&mut self, source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        if lines.len() < 2 {
            return source.to_string();
        }
        let idx = self.rng.below(lines.len());
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i != idx {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// Balik satu karakter ascii acak.
    fn flip_char(&mut self, source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let at = self.rng.below(chars.len());
        let flip_table = [
            '0', '1', ';', '(', ')', '"', '\'', '+', '-', '&', '|', '^', '~', '!', '=', '<', '>',
        ];
        let new_char = self.rng.pick(&flip_table);
        let mut out: String = chars[..at].iter().collect();
        out.push(*new_char);
        out.extend(&chars[at + 1..]);
        out
    }

    /// Bungkus RHS assegna dgn `$bits(...)`/`$clog2(...)` — elaborator
    /// const-fold `$bits`/`$clog2`/`$size`/`$left`/`$right`/`$low`/`$high`.
    fn inject_sysfunc_assign(&mut self, source: &str) -> String {
        let sysfuncs = [
            "$bits(", "$clog2(", "$size(", "$left(", "$right(", "$low(", "$high(",
        ];
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            if !done && t.starts_with("assign") {
                if let Some(e) = t.find('=') {
                    let lhs = t["assign".len()..e].trim();
                    let rhs = t[e + 1..].trim_end_matches(';').trim();
                    let f = self.rng.pick(&sysfuncs);
                    out.push_str(&format!("{}assign {} = {}({});\n", indent, lhs, f, rhs));
                    done = true;
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }

    /// Sisipkan `$display` dgn format campuran `%0d/%0b/%0h/%s/%%` — stress
    /// format-string parser (proyek mendukung `%0d` zero-pad secara eksplisit).
    fn inject_display_format(&mut self, source: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            if !done && t.starts_with("assign") {
                out.push_str(&format!(
                    "{}initial $display(\"fz %0d %0b %0h %0s %%x\", 1, 2'b01, 8'hf, \"q\");\n",
                    indent
                ));
                done = true;
            }
            out.push_str(line);
            out.push('\n');
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }

    /// Sisipkan ekspresi streaming `{<<{a}}`/`{>>{a}}` (SV-only) sebagai assegna —
    /// parser streaming operator. iverilog ref biasanya N/A; mivon tidak boleh
    /// crash (parser harus recover).
    fn inject_stream_assign(&mut self, source: &str) -> String {
        let ops = ["{<<{8'hA5}}", "{>>{8'hA5}}", "{<<{ {4{1'b1}} }}"];
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            if !done && t.starts_with("assign") {
                if let Some(e) = t.find('=') {
                    let lhs = t["assign".len()..e].trim();
                    let pat = self.rng.pick(&ops);
                    out.push_str(&format!("{}assign {} = {};\n", indent, lhs, pat));
                    done = true;
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi: `replace_keyword` dulu pakai `find` polos — substring di
    /// tengah kata ikut terganti (`for` di `forever` → `$left`+`ever` =
    /// `$leftever`) → sampah tokenisasi (noise roundtrip MV: output SV
    /// `$leftever;` E1002) yang tak merepresentasikan cacat sintaks nyata.
    #[test]
    fn replace_keyword_respects_word_boundary() {
        for seed in 0..300u64 {
            let mut rng = crate::Rng::new(seed);
            let mut m = Mutator::new(&mut rng);
            let out = m.replace_keyword("forever begin end");
            // `forever` hanya bisa pecah bila substring `for`/`or`/`re`/`ver`
            // diganti tanpa boundary — dengan boundary, kata utuh dipertahankan.
            assert!(
                out.contains("forever") || !out.contains("ver"),
                "seed {seed}: kata pecah di tengah: {out}"
            );
        }
    }
}
