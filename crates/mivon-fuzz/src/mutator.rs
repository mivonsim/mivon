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

    /// Terapkan 1 operasi mutasi random (21 opsi — termasuk range decl
    /// dan timescale).
    pub fn mutate(&mut self, source: &str, corpus: &Corpus) -> String {
        if source.is_empty() {
            return String::new();
        }
        match self.rng.below(26) {
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
            // ── Area baru: range decl / timescale ──
            17 => self.tweak_range(source),
            18 => self.inject_timescale(source),
            // ── Area baru: construct-aware (LSB bug fuzz verify_bad_0021 /
            //    0586): initializer deklarasi + seleksi bit/range/part ──
            19 => self.inject_decl_init(source),
            20 => self.inject_bitselect(source),
            // ── Area baru (tahap perluasan): concat/signed/index/terminasi ──
            21 => self.inject_concat(source),
            22 => self.toggle_signed(source),
            23 => self.index_oob(source),
            24 => self.inject_initial_finish(source),
            _ => source.to_string(), // no-op
        }
    }

    /// Mutasi RINGAN tak-pengembang utk source raksasa (>SIZE_CAP).
    ///
    /// Subset dari `mutate` yang dijamin tak meledakkan ukuran: tanpa
    /// `splice`/`duplicate_chunk`/`insert_garbage`/`insert_directive`/
    /// `inject_*`/`dup_*` (dulu large = `n_mut=0` total — dievaluasi mentah
    /// tanpa variasi mutasi sama sekali = coverage gap). Ukuran output
    /// <= input + 32B (`tweak_literal` ekstrem terpanjang).
    pub fn mutate_light(&mut self, source: &str, _corpus: &Corpus) -> String {
        if source.is_empty() {
            return String::new();
        }
        match self.rng.below(5) {
            0 => self.replace_keyword(source),
            1 => self.delete_chunk(source),
            2 => self.tweak_literal(source),
            3 => self.remove_line(source),
            _ => self.flip_char(source),
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

    /// Ubah range deklarasi `[a:b]` → balik `[b:a]` atau lebarkan MSB+8
    /// (width resolution / range validation stress).
    fn tweak_range(&mut self, source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut idx = 0usize;
        while idx < chars.len() {
            if chars[idx] == '[' {
                let mut j = idx + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j > idx + 1 && j < chars.len() && chars[j] == ':' {
                    let mut k = j + 1;
                    while k < chars.len() && chars[k].is_ascii_digit() {
                        k += 1;
                    }
                    if k > j + 1 && k < chars.len() && chars[k] == ']' {
                        let a: String = chars[idx + 1..j].iter().collect();
                        let b: String = chars[j + 1..k].iter().collect();
                        let rep = if self.rng.below(2) == 0 {
                            format!("[{b}:{a}]")
                        } else {
                            let widened: usize =
                                a.parse::<usize>().unwrap_or(7).saturating_add(8);
                            format!("[{widened}:{b}]")
                        };
                        let mut out: String = chars[..idx].iter().collect();
                        out.push_str(&rep);
                        out.extend(&chars[k + 1..]);
                        return out;
                    }
                }
            }
            idx += 1;
        }
        source.to_string()
    }

    /// Sisipkan initializer konstanta pada SATU deklarasi variabel
    /// (`logic x;` → `logic x = 0;`). Menyerang area initialization LRM §6.8 —
    /// temuan fuzz `verify_bad_0586` (negedge palsu di t=0 dari initializer
    /// yang memicu transisi X→0). Hanya terapkan bila deklarasi BELUM punya
    /// `=` (hindari initializer ganda). Word-boundary aman utk keyword.
    fn inject_decl_init(&mut self, source: &str) -> String {
        let inits = [" = 0", " = '0", " = 1'b0", " = 4'b0", " = 8'h0"];
        let decl_kw = [
            "logic", "reg", "bit", "integer", "int", "longint", "shortint", "byte",
        ];
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            if !done {
                let t = line.trim_start();
                let kw = decl_kw
                    .iter()
                    .find(|k| kw_at(t, k));
                if let Some(_k) = kw {
                    // Sudah punya initializer? lewati.
                    if !t.contains('=') {
                        if let Some(semi) = line.rfind(';') {
                            let init = self.rng.pick(&inits);
                            let mut l = String::from(&line[..semi]);
                            l.push_str(init);
                            l.push(';');
                            out.push_str(&l);
                            out.push('\n');
                            done = true;
                            continue;
                        }
                    }
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

    /// Bungkus RHS assegna berupa identifier tunggal dgn bit/range/part-select
    /// berbagai bentuk range: menurun `[3:0]`, menaik `[0:3]`, lsb≠0 `[7:4]`,
    /// statis `[2]`, serta part-select `+:`/`-:`. Menyerang seleksi bit packed
    /// (temuan fuzz `verify_bad_0021`): posisi bit fisik vs indeks deklarasi.
    /// Hanya terapkan bila RHS murni identifier (huruf/`_`/`$` di awal) — RHS
    /// literal (`8'hFF[3:0]` tak valid) sengaja dilewati.
    fn inject_bitselect(&mut self, source: &str) -> String {
        let sels = ["[3:0]", "[0:3]", "[7:4]", "[2]", "[0 +: 4]", "[3 -: 4]"];
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            if !done && t.starts_with("assign") {
                if let Some(eq) = t.find('=') {
                    let rhs = t[eq + 1..].trim().trim_end_matches(';');
                    let first = rhs.chars().next().unwrap_or(' ');
                    if first.is_alphabetic() || first == '_' || first == '$' {
                        let lhs = t["assign".len()..eq].trim();
                        let indent: String = line
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .collect();
                        let sel = self.rng.pick(&sels);
                        out.push_str(&format!(
                            "{}assign {} = {}{};\n",
                            indent, lhs, rhs, sel
                        ));
                        done = true;
                        continue;
                    }
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

    /// Sisipkan directive `` `timescale `` di baris pertama bila belum ada
    /// (timescale resolution stress; no-op bila sudah ada).
    fn inject_timescale(&mut self, source: &str) -> String {
        if source.contains("timescale") {
            return source.to_string();
        }
        let ts = self.rng.pick(&["`timescale 1ns/1ps\n", "`timescale 1ps/1ps\n"]);
        format!("{ts}{source}")
    }

    /// Bungkus RHS assegna bersih dgn concat `{...}` — stress resolusi
    /// lebar/tipe pada konteks concatenation (LRM §11.4.12).
    fn inject_concat(&mut self, source: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            if !done && t.starts_with("assign") {
                if let Some(eq) = t.find('=') {
                    let rhs = t[eq + 1..].trim_end_matches(';').trim();
                    if !rhs.is_empty() && !rhs.contains('{') {
                        let lhs = t["assign".len()..eq].trim();
                        let indent: String = line
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .collect();
                        out.push_str(&format!("{indent}assign {lhs} = {{{rhs}}};\n"));
                        done = true;
                        continue;
                    }
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

    /// Tambah `signed` pada satu deklarasi vektor (sign/width resolution
    /// stress: `logic [7:0]` → `logic signed [7:0]`).
    fn toggle_signed(&mut self, source: &str) -> String {
        let kws = ["logic", "wire", "reg", "bit"];
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            if !done && !t.contains("signed") {
                if let Some(kw) = kws.iter().find(|k| kw_at(t, k)) {
                    if t[kw.len()..].trim_start().starts_with('[') {
                        let indent_len = line.len() - t.len();
                        let insert_at = indent_len + kw.len();
                        let mut l = line.to_string();
                        l.insert_str(insert_at, " signed");
                        out.push_str(&l);
                        out.push('\n');
                        done = true;
                        continue;
                    }
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

    /// Naikkan satu indeks statis `x[7]` → `x[8]` (out-of-bounds/partial
    /// select stress; hanya angka polos, range `[a:b]` dilewati).
    fn index_oob(&mut self, source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut i = 0usize;
        while i + 2 < chars.len() {
            if chars[i] == '[' && chars[i + 1].is_ascii_digit() {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ']' {
                    let n: String = chars[i + 1..j].iter().collect();
                    if let Ok(v) = n.parse::<usize>() {
                        let mut out: String = chars[..i + 1].iter().collect();
                        out.push_str(&(v.saturating_add(1)).to_string());
                        out.extend(&chars[j..]);
                        return out;
                    }
                }
            }
            i += 1;
        }
        source.to_string()
    }

    /// Tambah `initial #100 $finish;` bila source belum punya `$finish` —
    /// terminasi deterministik (cegah tb panjang membakar budget watchdog).
    fn inject_initial_finish(&mut self, source: &str) -> String {
        if source.contains("$finish") {
            return source.to_string();
        }
        format!("{source}\ninitial #100 $finish;\n")
    }
}

/// Cek keyword `kw` di awal teks `t` dengan word-boundary setelahnya
/// (huruf/angka/`_`/`$` tak boleh langsung mengikuti). Mencegah
/// `byte` cocok di `byte_enable`, dsb.
fn kw_at(t: &str, kw: &str) -> bool {
    if !t.starts_with(kw) {
        return false;
    }
    match t.chars().nth(kw.len()) {
        None => true,
        Some(c) => !(c.is_alphanumeric() || c == '_' || c == '$'),
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

    /// Mutasi ringan large-seed: tak pernah blowup (<= input + 32B),
    /// tak pernah panic, deterministik per seed.
    #[test]
    fn mutate_light_never_grows() {
        let dir = std::env::temp_dir().join("fzlight_empty");
        let _ = std::fs::create_dir_all(&dir);
        let corpus = Corpus::load(Some(&dir));
        let src = "module tb;\n  logic [7:0] a;\n  assign a = 8'hFF;\n  always_comb begin\n    a = a + 1;\n  end\nendmodule\n";
        for seed in 0..200u64 {
            let mut rng = crate::Rng::new(seed);
            let mut m = Mutator::new(&mut rng);
            let out = m.mutate_light(src, &corpus);
            assert!(
                out.len() <= src.len() + 32,
                "seed {seed}: light membesar {} -> {}",
                src.len(),
                out.len()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `tweak_range` membalik/melebarkan range; tanpa range → no-op.
    /// `inject_timescale` prepend sekali; sudah ada → no-op.
    #[test]
    fn range_and_timescale_mutations() {
        let mut rng = crate::Rng::new(42);
        let mut m = Mutator::new(&mut rng);
        let src = "module m;\n  logic [7:0] a;\nendmodule\n";
        let out = m.tweak_range(src);
        assert!(
            out.contains("[0:7]") || out.contains("[15:0]"),
            "range berubah: {out}"
        );
        assert!(!m.tweak_range("module m; endmodule").contains('['));
        let ts = m.inject_timescale(src);
        assert!(ts.starts_with("`timescale"), "prepend timescale: {ts}");
        assert_eq!(m.inject_timescale(&ts), ts, "ganda = no-op");
    }

    /// Regresi mutator construct-aware: `inject_decl_init` menyisipkan
    /// initializer konstanta pada deklarasi (tanpa dobel `=`), dan
    /// `inject_bitselect` membungkus RHS identifier dgn seleksi bit/range/
    /// part. Keduanya deterministik per seed dan tak pernah panic.
    #[test]
    fn construct_aware_inject_decl_init_and_bitselect() {
        let src = "module m;\n  logic a;\n  logic [7:0] b;\n  assign a = b;\nendmodule\n";
        let mut saw_init = false;
        let mut saw_sel = false;
        for seed in 0..200u64 {
            let mut rng = crate::Rng::new(seed);
            let mut m = Mutator::new(&mut rng);
            let init_out = m.inject_decl_init(src);
            if init_out.contains("= 0")
                || init_out.contains("= '0")
                || init_out.contains("= 1'b0")
            {
                saw_init = true;
                // Tak boleh ada deklarasi dobel '='.
                assert!(
                    init_out.matches('=').count() <= 2,
                    "seed {seed}: initializer dobel: {init_out}"
                );
            }
            let mut rng2 = crate::Rng::new(seed ^ 0xABC);
            let mut m2 = Mutator::new(&mut rng2);
            let sel_out = m2.inject_bitselect(src);
            // Select harus diterapkan ke assegna RHS identifier (`b` → `b[...]`).
            if sel_out
                .lines()
                .any(|l| l.trim_start().starts_with("assign") && l.contains("b["))
            {
                saw_sel = true;
            }
        }
        assert!(saw_init, "inject_decl_init minimal sekali terpicu");
        assert!(saw_sel, "inject_bitselect minimal sekali terpicu");
        // Tanpa deklarasi → no-op (tak panic).
        let mut rng3 = crate::Rng::new(99);
        let mut m3 = Mutator::new(&mut rng3);
        assert_eq!(
            m3.inject_decl_init("module m; endmodule"),
            "module m; endmodule"
        );

        // Mutasi baru (tahap perluasan): concat/signed/index/finish.
        let base =
            "module m;\n  logic [7:0] a;\n  logic [3:0] b;\n  assign a = b;\nendmodule\n";
        let mut rng4 = crate::Rng::new(7);
        let mut m4 = Mutator::new(&mut rng4);
        assert!(m4.inject_concat(base).contains("assign a = {b};"));
        assert!(m4.toggle_signed(base).contains("logic signed [7:0]"));
        let idx = "module m;\n  logic [1:0] y;\n  assign y = a[2];\nendmodule\n";
        assert!(m4.index_oob(idx).contains("[3]"));
        let fin = m4.inject_initial_finish(base);
        assert!(fin.contains("initial #100 $finish;"));
        assert_eq!(
            m4.inject_initial_finish(&fin),
            fin,
            "sudah ada $finish = no-op"
        );
    }

    /// `kw_at` menghargai word-boundary: `byte` tak cocok di `byte_enable`,
    /// tapi cocok di `byte x;`.
    #[test]
    fn kw_at_word_boundary() {
        assert!(kw_at("logic x;", "logic"));
        assert!(!kw_at("logical x;", "logic"));
        assert!(kw_at("byte x;", "byte"));
        assert!(!kw_at("byte_enable x;", "byte"));
    }
}
