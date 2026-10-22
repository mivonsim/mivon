//! Outline dokumen, hierarki tipe, dan pencarian simbol workspace
//! (LSP-11/LSP-12/LSP-14): `document_symbols`, relasi extends/implements,
//! `hierarchy_items`, dan `search_workspace_symbols`.
use super::*;

impl LspBackend {
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

}

