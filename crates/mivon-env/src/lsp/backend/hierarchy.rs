//! Hierarki tipe (LSP-14): relasi `extends`/`implements` dari outline —
//! rantai supertype, daftar subtype, pencarian class, dan item `typeHierarchy`.

use super::*;

impl LspBackend {
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
        source: &str,
        symbols: &[DocSymbol],
        names: &[String],
    ) -> Vec<TypeHierarchyItem> {
        let mut out = Vec::new();
        // LSP-28: baris dikumpulkan sekali (bukan nth per nama).
        let lines: Vec<&str> = source.lines().collect();
        for n in names {
            let Some((line, col)) = Self::find_class_symbol(symbols, n) else {
                continue;
            };
            // name_col adalah offset byte → kolom karakter.
            let line_text = lines.get(line as usize).copied().unwrap_or("");
            let col = Self::byte_col_to_char(line_text, col);
            let len = n.chars().count() as u32;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `type_supertypes` mengikuti rantai `extends` (LSP-14) dan berhenti pada
    /// base yang tidak terdefinisi — inti hierarki tipe yang dipindah ke
    /// `backend/hierarchy.rs`.
    #[test]
    fn supertypes_follow_extends_chain() {
        let src = "class A; endclass\nclass B extends A; endclass\nclass C extends B; endclass\n";
        let syms = LspBackend::document_symbols(src);
        assert_eq!(
            LspBackend::type_supertypes(&syms, "C"),
            vec!["B".to_string(), "A".to_string()]
        );
        assert!(LspBackend::type_supertypes(&syms, "A").is_empty());
        assert!(LspBackend::type_supertypes(&syms, "Missing").is_empty());
        // Base tak terdefinisi menghentikan rantai (bukan error, bukan entri).
        let orphan = "class D extends UnknownBase; endclass\n";
        let osyms = LspBackend::document_symbols(orphan);
        assert!(LspBackend::type_supertypes(&osyms, "D").is_empty());
    }
}
