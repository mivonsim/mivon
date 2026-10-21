//! Hover LSP: konteks deklarasi identifier di posisi kursor.
//!
//! `hover_info` adalah inti murni (bisa diuji tanpa server) yang dipakai
//! handler `hover` di `backend.rs`. Dipisah ke file sendiri karena ukurannya
//! ~1100 baris: menebak kind simbol (module/port/signal/param/fungsi/class),
//! menyusun detail port, dan merender doc comment `//` jadi markdown.

use super::*;

impl LspBackend {
    /// Hover core (murni, bisa diuji) — LSP-06 tahap 1: konteks deklarasi
    /// identifier di posisi kursor, berformat markdown:
    ///   **kind** `name` — snippet baris deklarasi.
    pub(super) fn hover_info(source: &str, line: u32, character: u32) -> Option<String> {
        let line_text = source.lines().nth(line as usize)?;
        let (word, _, _) = Self::word_at_position(line_text, character)?;

        let (def_line, _, _) = Self::find_definition(source, line, character)?;
        let decl_text = source.lines().nth(def_line as usize)?.trim().to_string();

        // LSP-06 tahap 2a: doc comment `//` tepat di atas definisi.
        let doc = Self::doc_comment_above(source, def_line);

        // Tebak kind dari keyword di baris definisi.
        let toks = Self::line_tokens(&decl_text);
        let kind = toks
            .iter()
            .find_map(|(w, _)| match w.as_str() {
                "module" | "interface" | "package" | "program" | "class" | "checker" => {
                    Some(w.clone())
                }
                "input" => Some("port input".into()),
                "output" => Some("port output".into()),
                "inout" => Some("port inout".into()),
                "parameter" | "localparam" => Some(w.clone()),
                "function" | "task" => Some(w.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "declaration".into());

        Some(format!(
            "**{kind}** `{word}`\n\n```systemverilog\n{decl_text}\n```{}{}",
            Self::port_detail(&kind, &decl_text)
                .map(|d| format!("\n\n{d}"))
                .unwrap_or_default(),
            doc.map(|d| format!("\n\n{d}")).unwrap_or_default()
        ))
    }

    /// LSP-06 tahap 2b: detail eksplisit port — `direction: input ·
    /// range: `[7:0]`` (atau `scalar` tanpa kurung). Return None bila
    /// bukan port.
    pub(crate) fn port_detail(kind: &str, decl_text: &str) -> Option<String> {
        let dir = kind.strip_prefix("port ")?;
        let mut depth = 0usize;
        let mut start = None;
        let mut range = None;
        for (i, c) in decl_text.char_indices() {
            if c == '[' {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            } else if c == ']' {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s) = start.take() {
                        range = Some(decl_text[s..=i].to_string());
                        break;
                    }
                }
            }
        }
        Some(match range {
            Some(r) => format!("direction: {dir} · range: `{r}`"),
            None => format!("direction: {dir} · scalar"),
        })
    }

    /// LSP-06 tahap 2a: kumpulkan baris komentar `//` berurutan tepat di
    /// atas baris definisi (lewati baris kosong). Return teks tanpa `//`.
    pub(crate) fn doc_comment_above(source: &str, def_line: u32) -> Option<String> {
        let lines: Vec<&str> = source.lines().collect();
        let mut out: Vec<String> = Vec::new();
        let mut i = def_line.checked_sub(1)? as usize;
        while let Some(l) = lines.get(i) {
            let t = l.trim();
            if t.is_empty() {
                if i == 0 {
                    break;
                }
                i -= 1;
                continue;
            }
            if let Some(c) = t.strip_prefix("//") {
                out.push(c.trim().to_string());
            } else {
                break;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        if out.is_empty() {
            return None;
        }
        out.reverse();
        Some(out.join("\n"))
    }

    /// Document symbol core (murni, bisa diuji) — LSP-12 tahap 1+2a:
    /// outline simbol dokumen dengan nesting scope sederhana
    /// (module/interface/package/class/checker sebagai container;
    /// function/task sebagai scope frame — lokal di badannya jadi anak
    /// function; parameter/port/signal sebagai anak).
    pub(crate) fn document_symbols(source: &str) -> Vec<DocSymbol> {
        const OPEN_KW: &[(&str, u8)] = &[
            ("module", DocSymbol::KIND_MODULE),
            ("macromodule", DocSymbol::KIND_MODULE),
            ("interface", DocSymbol::KIND_INTERFACE),
            ("package", DocSymbol::KIND_PACKAGE),
            ("class", DocSymbol::KIND_CLASS),
            ("checker", DocSymbol::KIND_CLASS),
        ];
        const CLOSE_KW: &[&str] = &[
            "endmodule",
            "endinterface",
            "endpackage",
            "endclass",
            "endchecker",
            "endfunction",
            "endtask",
        ];
        const FUNC_KW: &[&str] = &["function", "task"];
        const CONST_KW: &[&str] = &["parameter", "localparam"];
        const VAR_KW: &[&str] = &[
            "input", "output", "inout", "wire", "reg", "logic", "bit", "int", "integer", "byte",
            "shortint", "longint", "real", "time", "string",
        ];

        struct Frame {
            sym: Option<DocSymbol>,
            children: Vec<DocSymbol>,
        }

        let mut stack: Vec<Frame> = vec![Frame {
            sym: None,
            children: Vec::new(),
        }];

        for (idx, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let toks = Self::line_tokens(line);
            let Some((t0, c0)) = toks.first().cloned() else {
                continue;
            };

            // Tutup scope → merge symbol dengan children terkumpul.
            if CLOSE_KW.contains(&t0.as_str()) && stack.len() > 1 {
                let mut frame = stack.pop().unwrap();
                if let Some(sym) = frame.sym.as_mut() {
                    std::mem::swap(&mut sym.children, &mut frame.children);
                }
                if let Some(sym) = frame.sym.take() {
                    stack.last_mut().unwrap().children.push(sym);
                } else {
                    stack.last_mut().unwrap().children.extend(frame.children);
                }
                continue;
            }

            // Buka scope: keyword + nama.
            if let Some((_, kind)) = OPEN_KW.iter().find(|(kw, _)| *kw == t0.as_str()) {
                if let Some((w1, c1)) = toks.get(1).cloned() {
                    if Self::is_valid_identifier(&w1) {
                        // LSP-14 tahap 2a: `class A extends B` (+ parameter
                        // `class A #(...) extends B` — cari token extends).
                        let base = (*kind == DocSymbol::KIND_CLASS)
                            .then(|| {
                                toks.iter()
                                    .skip(2)
                                    .position(|(w, _)| w == "extends")
                                    .and_then(|p| toks.get(p + 3))
                                    .filter(|(w, _)| Self::is_valid_identifier(w))
                                    .map(|(w, _)| w.clone())
                            })
                            .flatten();
                        stack.push(Frame {
                            sym: Some(DocSymbol {
                                name: w1.clone(),
                                kind: *kind,
                                line: idx as u32,
                                col: c0 as u32,
                                children: Vec::new(),
                                base,
                                name_col: c1 as u32,
                            }),
                            children: Vec::new(),
                        });
                        // Parameter inline pada baris yang sama
                        // (`module m #(parameter WIDTH = 8) (`).
                        let frame = stack.last_mut().unwrap();
                        for i in 2..toks.len() {
                            if matches!(toks[i].0.as_str(), "parameter" | "localparam") {
                                for (w, c) in toks.iter().skip(i + 1).cloned() {
                                    if Self::is_valid_identifier(&w)
                                        && !matches!(
                                            w.as_str(),
                                            "logic"
                                                | "bit"
                                                | "int"
                                                | "integer"
                                                | "signed"
                                                | "unsigned"
                                                | "real"
                                                | "string"
                                                | "type"
                                        )
                                        && !frame.children.iter().any(|ch| ch.name == w)
                                    {
                                        frame.children.push(DocSymbol {
                                            name: w,
                                            kind: DocSymbol::KIND_CONSTANT,
                                            line: idx as u32,
                                            col: c as u32,
                                            children: Vec::new(),
                                            base: None,
                                            name_col: c as u32,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                continue;
            }

            // Function/task → scope frame (LSP-12 tahap 2a): deklarasi
            // lokal di badan function menjadi children function, bukan
            // sibling module. Ditutup oleh `endfunction`/`endtask`.
            if FUNC_KW.contains(&t0.as_str()) {
                for (w, c) in toks.iter().skip(1).cloned() {
                    if Self::is_valid_identifier(&w)
                        && !matches!(
                            w.as_str(),
                            "logic"
                                | "bit"
                                | "int"
                                | "integer"
                                | "signed"
                                | "unsigned"
                                | "automatic"
                                | "static"
                                | "void"
                        )
                    {
                        stack.push(Frame {
                            sym: Some(DocSymbol {
                                name: w.clone(),
                                kind: DocSymbol::KIND_FUNCTION,
                                line: idx as u32,
                                col: c as u32,
                                children: Vec::new(),
                                base: None,
                                name_col: c as u32,
                            }),
                            children: Vec::new(),
                        });
                        break;
                    }
                }
                continue;
            }

            // parameter/localparam → Constant.
            if CONST_KW.contains(&t0.as_str()) {
                for (w, c) in toks.iter().skip(1).cloned() {
                    if Self::is_valid_identifier(&w)
                        && !matches!(
                            w.as_str(),
                            "logic"
                                | "bit"
                                | "int"
                                | "integer"
                                | "signed"
                                | "unsigned"
                                | "real"
                                | "string"
                                | "type"
                        )
                    {
                        stack.last_mut().unwrap().children.push(DocSymbol {
                            name: w.clone(),
                            kind: DocSymbol::KIND_CONSTANT,
                            line: idx as u32,
                            col: c as u32,
                            children: Vec::new(),
                            base: None,
                            name_col: c as u32,
                        });
                    }
                }
                continue;
            }

            // Port/signal decl → Variable per nama.
            if VAR_KW.contains(&t0.as_str()) {
                for (w, c) in toks.iter().skip(1).cloned() {
                    if Self::is_valid_identifier(&w)
                        && !matches!(
                            w.as_str(),
                            "logic"
                                | "reg"
                                | "wire"
                                | "bit"
                                | "int"
                                | "integer"
                                | "signed"
                                | "unsigned"
                                | "real"
                                | "string"
                                | "input"
                                | "output"
                                | "inout"
                        )
                    {
                        stack.last_mut().unwrap().children.push(DocSymbol {
                            name: w.clone(),
                            kind: DocSymbol::KIND_VARIABLE,
                            line: idx as u32,
                            col: c as u32,
                            children: Vec::new(),
                            base: None,
                            name_col: c as u32,
                        });
                    }
                }
                continue;
            }
        }

        // Tutup scope yang belum tertutup (dokumen rusak).
        while stack.len() > 1 {
            let mut frame = stack.pop().unwrap();
            if let Some(sym) = frame.sym.as_mut() {
                std::mem::swap(&mut sym.children, &mut frame.children);
            }
            if let Some(sym) = frame.sym.take() {
                stack.last_mut().unwrap().children.push(sym);
            } else {
                stack.last_mut().unwrap().children.extend(frame.children);
            }
        }
        std::mem::take(&mut stack[0].children)
    }

    /// Folding range core (murni, bisa diuji) — LSP-15 tahap 1:
    /// region lipat untuk blok scope berdasarkan token baris:
    ///   - module/interface/package/class/checker/function/task/generate
    ///     dibuka sebagai token PERTAMA baris, ditutup end* sebagai token
    ///     pertama baris.
    ///   - begin...end dan case...endcase: `begin`/`case` membuka (begin
    ///     sebagai token terakhir baris header; case sebagai token pertama),
    ///     ditutup bila baris memuat token `end` / `endcase`.
    ///     Return (start_line, end_line) urut.
    ///
    /// LSP-14 tahap 2a: rantai supertype (langsung → akar) mengikuti
    /// `base` (`class A extends B`). Hanya base terdefinisi yang masuk
    /// rantai (unknown menghentikan). Cycle-guard. Non-class → kosong.
    pub(crate) fn type_supertypes(symbols: &[DocSymbol], name: &str) -> Vec<String> {
        fn find<'a>(syms: &'a [DocSymbol], name: &str) -> Option<&'a DocSymbol> {
            for s in syms {
                if s.kind == DocSymbol::KIND_CLASS && s.name == name {
                    return Some(s);
                }
                if let Some(f) = find(&s.children, name) {
                    return Some(f);
                }
            }
            None
        }
        let mut out = Vec::new();
        let mut cur = name.to_string();
        while let Some(sym) = find(symbols, &cur) {
            let Some(b) = sym.base.clone() else {
                break;
            };
            // Cycle-guard + base tak terdefinisi menghentikan rantai
            // tanpa dimasukkan (kontrak: hanya class terdefinisi, dan
            // peminta sendiri tidak termasuk).
            if b == name || out.contains(&b) || find(symbols, &b).is_none() {
                break;
            }
            out.push(b.clone());
            cur = b;
        }
        out
    }

    /// LSP-14 tahap 2a: subtype langsung + transitif (BFS mengikuti
    /// edge base→child). Urut BFS deterministik.
    pub(crate) fn type_subtypes(symbols: &[DocSymbol], name: &str) -> Vec<String> {
        fn all_classes<'a>(syms: &'a [DocSymbol], out: &mut Vec<&'a DocSymbol>) {
            for s in syms {
                if s.kind == DocSymbol::KIND_CLASS {
                    out.push(s);
                }
                all_classes(&s.children, out);
            }
        }
        let mut all = Vec::new();
        all_classes(symbols, &mut all);
        let mut out = Vec::new();
        let mut queue = std::collections::VecDeque::from([name.to_string()]);
        while let Some(cur) = queue.pop_front() {
            for c in &all {
                // Self-extends / siklus kembali ke peminta dikecualikan.
                if c.name == name {
                    continue;
                }
                if c.base.as_deref() == Some(cur.as_str()) && !out.contains(&c.name) {
                    out.push(c.name.clone());
                    queue.push_back(c.name.clone());
                }
            }
        }
        out
    }

    /// LSP-14 tahap 2a: posisi (line, name_col) deklarasi class ber-nama.
    pub(crate) fn find_class_symbol(symbols: &[DocSymbol], name: &str) -> Option<(u32, u32)> {
        for s in symbols {
            if s.kind == DocSymbol::KIND_CLASS && s.name == name {
                return Some((s.line, s.name_col));
            }
            if let Some(p) = Self::find_class_symbol(&s.children, name) {
                return Some(p);
            }
        }
        None
    }

    /// LSP-14 tahap 2a: bangun TypeHierarchyItem untuk nama-nama class
    /// yang ada deklarasinya (lewati base tak terdefinisi).
    pub(crate) fn hierarchy_items(
        uri: &Url,
        symbols: &[DocSymbol],
        names: &[String],
    ) -> Vec<TypeHierarchyItem> {
        let mut out = Vec::new();
        for n in names {
            let Some((line, col)) = Self::find_class_symbol(symbols, n) else {
                continue;
            };
            let len = n.len() as u32;
            out.push(TypeHierarchyItem {
                name: n.clone(),
                kind: SymbolKind::CLASS,
                tags: None,
                detail: None,
                uri: uri.clone(),
                range: Range {
                    start: Position { line, character: 0 },
                    end: Position {
                        line,
                        character: col + len,
                    },
                },
                selection_range: Range {
                    start: Position {
                        line,
                        character: col,
                    },
                    end: Position {
                        line,
                        character: col + len,
                    },
                },
                data: None,
            });
        }
        out
    }

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
        let ch = (character as usize).min(bytes.len());
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

    /// Workspace symbol search core (murni, bisa diuji) — LSP-11 tahap 1:
    /// cari di SEMUA dokumen terbuka (cache), flatten outline, filter
    /// case-insensitive substring. Query kosong = semua simbol.
    pub(crate) fn search_workspace_symbols(
        docs: &[(String, String)],
        query: &str,
    ) -> Vec<WsSymbolHit> {
        let q = query.to_lowercase();
        let mut out: Vec<WsSymbolHit> = Vec::new();

        fn walk(items: &[DocSymbol], uri: &str, q: &str, out: &mut Vec<WsSymbolHit>) {
            for s in items {
                if q.is_empty() || s.name.to_lowercase().contains(q) {
                    out.push(WsSymbolHit {
                        uri: uri.to_string(),
                        name: s.name.clone(),
                        kind: s.kind,
                        line: s.line,
                        col: s.col,
                    });
                }
                walk(&s.children, uri, q, out);
            }
        }

        for (uri, text) in docs {
            walk(&Self::document_symbols(text), uri, &q, &mut out);
        }
        out.sort_by(|a, b| (&a.uri, a.line, a.col).cmp(&(&b.uri, b.line, b.col)));
        out
    }

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

    /// Find-references core (murni, bisa diuji) — LSP-04 tahap 1:
    /// semua kemunculan identifier (sebagai token mandiri) dalam dokumen,
    /// urut baris. `include_declaration` tetap dikembalikan penuh — caller
    /// (handler) yang memfilter bila diminta tanpa deklarasi.
    pub(crate) fn find_references(source: &str, line: u32, character: u32) -> Vec<(u32, u32, u32)> {
        let Some(line_text) = source.lines().nth(line as usize) else {
            return Vec::new();
        };
        let Some((word, _, _)) = Self::word_at_position(line_text, character) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for (idx, text) in source.lines().enumerate() {
            // Skip komentar baris.
            let trimmed = text.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for (w, col) in Self::line_tokens(text) {
                if w == word {
                    out.push((idx as u32, col as u32, (col + w.len()) as u32));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi pemisahan modul: `hover_info` dipindah ke `backend/hover.rs` —
    /// pastikan masih mengembalikan konteks deklarasi (bukan `None`) dan
    /// menyertakan baris deklarasi aslinya.
    #[test]
    fn hover_reports_signal_declaration() {
        let src = "module m;\n  logic [3:0] cnt;\n  assign o = cnt;\nendmodule\n";
        let line = src.lines().position(|l| l.contains("assign")).unwrap() as u32;
        let col = src.lines().nth(line as usize).unwrap().find("cnt").unwrap() as u32;
        let info = LspBackend::hover_info(src, line, col).expect("hover signal");
        assert!(info.contains("cnt"), "{info}");
        assert!(info.contains("logic [3:0] cnt;"), "{info}");
    }
}
