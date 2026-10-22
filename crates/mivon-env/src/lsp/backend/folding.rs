//! Folding ranges (LSP-08): blok begin/end, region komentar, dan grup
//! import/include — dihitung dari teks sumber, tanpa parser penuh.
use super::*;

impl LspBackend {
    pub(crate) fn compute_folding_ranges(source: &str) -> Vec<(u32, u32)> {
        // (keyword pembuka, keyword penutup) — pasangan scope utama.
        const PAIRS: &[(&str, &str)] = &[
            ("module", "endmodule"),
            ("macromodule", "endmodule"),
            ("interface", "endinterface"),
            ("package", "endpackage"),
            ("program", "endprogram"),
            ("class", "endclass"),
            ("checker", "endchecker"),
            ("function", "endfunction"),
            ("task", "endtask"),
            ("generate", "endgenerate"),
        ];

        let mut out: Vec<(u32, u32)> = Vec::new();
        // Stack: (keyword pembuka, start_line).
        let mut stack: Vec<(&str, u32)> = Vec::new();

        let close_top = |stack: &mut Vec<(&str, u32)>, out: &mut Vec<(u32, u32)>, idx: u32| {
            if let Some((_, start)) = stack.pop() {
                out.push((start, idx));
            }
        };

        for (idx, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let toks = Self::line_tokens(line);
            if toks.is_empty() {
                continue;
            }
            let t0 = toks[0].0.as_str();

            // Scope utama dibuka sebagai token pertama.
            if let Some((_, closer)) = PAIRS.iter().find(|(kw, _)| *kw == t0) {
                stack.push((*closer, idx as u32));
                continue;
            }
            // Scope utama ditutup sebagai token pertama.
            if let Some((_, opener)) = PAIRS.iter().find(|(_, ew)| *ew == t0) {
                let _ = opener;
                close_top(&mut stack, &mut out, idx as u32);
                continue;
            }

            let words: Vec<&str> = toks.iter().map(|(w, _)| w.as_str()).collect();

            // Penutup DULU (baris `end else begin` menutup lalu membuka):
            // endcase lalu end.
            if words.contains(&"endcase") {
                if let Some(pos) = stack
                    .iter()
                    .rposition(|(kw, _)| matches!(*kw, "case" | "casex" | "casez"))
                {
                    let (_, start) = stack.remove(pos);
                    out.push((start, idx as u32));
                }
            }
            // Token `end` mandiri (bukan bagian endmodule dkk — tokenizer
            // memisahkan kata utuh) menutup begin paling dalam.
            if words.contains(&"end") {
                if let Some(pos) = stack.iter().rposition(|(kw, _)| *kw == "begin") {
                    let (_, start) = stack.remove(pos);
                    out.push((start, idx as u32));
                } else if stack.len() > 1 {
                    // begin tidak ada → fallback tutup scope utama dalam.
                    close_top(&mut stack, &mut out, idx as u32);
                }
            }

            // Pembukaan setelah penutupan:
            // case/casex/casez sebagai token pertama membuka blok.
            if matches!(t0, "case" | "casex" | "casez") {
                stack.push(("case", idx as u32));
            }
            // `begin` sebagai token TERAKHIR baris header membuka blok.
            if words.len() > 1 && words[words.len() - 1] == "begin" {
                stack.push(("begin", idx as u32));
            }
        }

        // LSP-15 tahap 2a: fold blok komentar `/* ... */` multi-baris +
        // region `// #region` / `// #endregion` (alias tanpa `#`).
        out.extend(Self::comment_folding_ranges(source));
        // LSP-15 tahap 2b: fold grup baris `import` berurutan (2+).
        out.extend(Self::import_folding_ranges(source));

        out.sort_by_key(|(s, e)| (*s, *e));
        out.dedup();
        out
    }

    /// LSP-15 tahap 2b: rentang fold grup `import` berurutan (2+ baris,
    /// baris kosong di dalam grup dilewati). Murni.
    pub(crate) fn import_folding_ranges(source: &str) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let mut start: Option<u32> = None;
        let mut last: u32 = 0;
        let mut count = 0u32;
        let flush = |start: &mut Option<u32>, last: u32, count: u32, out: &mut Vec<(u32, u32)>| {
            if count >= 2 {
                out.push((start.take().unwrap(), last));
            } else {
                start.take();
            }
        };
        for (idx, line) in source.lines().enumerate() {
            let t = line.trim_start();
            if t.is_empty() {
                continue;
            }
            if t.starts_with("//") {
                continue;
            }
            let is_import = Self::line_tokens(line)
                .first()
                .map(|(w, _)| w == "import")
                .unwrap_or(false);
            if is_import {
                if start.is_none() {
                    start = Some(idx as u32);
                }
                last = idx as u32;
                count += 1;
            } else {
                flush(&mut start, last, count, &mut out);
                count = 0;
            }
        }
        flush(&mut start, last, count, &mut out);
        out
    }

    /// LSP-15 tahap 2a: rentang fold komentar blok & region marker.
    /// Murni, tanpa sentuh stack scope utama.
    pub(crate) fn comment_folding_ranges(source: &str) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = Vec::new();
        let lines: Vec<&str> = source.lines().collect();
        let mut block_start: Option<u32> = None;
        let mut region_stack: Vec<u32> = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            // Region marker: `// #region` / `// region` (alias tanpa `#`).
            // Kata pertama setelah `//` harus tepat keyword (batas kata —
            // `// regional` bukan marker).
            if let Some(c) = t.strip_prefix("//") {
                let first = c
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(':')
                    .to_lowercase();
                if first == "#endregion" || first == "endregion" {
                    if let Some(s) = region_stack.pop() {
                        out.push((s, idx as u32));
                    }
                    continue;
                }
                if first == "#region" || first == "region" {
                    region_stack.push(idx as u32);
                    continue;
                }
                // Komentar garis lain: bukan marker DAN bukan blok.
                continue;
            }
            // Blok komentar /* ... */.
            if block_start.is_none() {
                if let Some(open) = line.find("/*") {
                    let rest = &line[open + 2..];
                    if !rest.contains("*/") {
                        block_start = Some(idx as u32);
                    }
                }
            } else if line.contains("*/") {
                out.push((block_start.take().unwrap(), idx as u32));
            }
        }
        out
    }

}

