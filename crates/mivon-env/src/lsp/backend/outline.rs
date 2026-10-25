//! Outline dokumen (LSP-12) dan pencarian simbol workspace (LSP-11):
//! `document_symbols` membangun pohon `DocSymbol` dari token baris, lalu
//! `search_workspace_symbols` memakainya untuk query lintas dokumen.

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
        // LSP-12 tahap 2b: qualifier deklarasi yang dilewati sebelum keyword
        // efektif. `pure`/`extern` SENGAJA tidak ada di sini: prototype
        // tidak punya endfunction sehingga membuka frame akan menelan simbol
        // berikutnya.
        const QUAL_KW: &[&str] = &[
            "virtual",
            "static",
            "local",
            "protected",
            "rand",
            "randc",
            "const",
            "var",
            "automatic",
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
            // LSP-12 tahap 2b: lewati qualifier deklarasi lebih dulu supaya
            // `virtual function ...` dan `rand bit [7:0] addr;` ikut
            // ter-outline (sebelumnya baris seperti ini tak menghasilkan
            // simbol sama sekali karena t0 = "virtual"/"rand").
            let mut qi = 0usize;
            while qi < toks.len() && QUAL_KW.contains(&toks[qi].0.as_str()) {
                // `virtual interface <tipe> <var>;` adalah deklarasi
                // variabel virtual-interface, BUKAN pembukaan scope
                // interface (tidak ada endinterface) — jangan lewati
                // `virtual` di sini (review R197 BUG-1).
                if toks[qi].0 == "virtual"
                    && toks.get(qi + 1).map(|t| t.0.as_str()) == Some("interface")
                {
                    break;
                }
                qi += 1;
            }
            let Some((t0, c0)) = toks.get(qi).cloned() else {
                continue;
            };
            // Sisa token setelah keyword efektif (indeks relatif).
            let rest = &toks[qi + 1..];

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
                if let Some((w1, c1)) = rest.first().cloned() {
                    if Self::is_valid_identifier(&w1) {
                        // LSP-14 tahap 2a: `class A extends B` (+ parameter
                        // `class A #(...) extends B` — cari token extends).
                        let base = (*kind == DocSymbol::KIND_CLASS)
                            .then(|| {
                                rest.iter()
                                    .skip(1)
                                    .position(|(w, _)| w == "extends")
                                    .and_then(|p| rest.get(p + 2))
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
                        for i in 0..rest.len() {
                            if matches!(rest[i].0.as_str(), "parameter" | "localparam") {
                                for (w, c) in rest.iter().skip(i + 1).cloned() {
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
                for (w, c) in rest.iter().cloned() {
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
                for (w, c) in rest.iter().cloned() {
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
                for (w, c) in rest.iter().cloned() {
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
