//! Hover LSP (LSP-06): konteks deklarasi identifier di posisi kursor.
//!
//! `hover_info` menyusun markdown (**kind** `nama` — baris deklarasi + detail
//! port + doc comment `//`), dibantu `port_detail` dan `doc_comment_above`.

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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi pemisahan modul: `hover_info` ada di `backend/hover.rs` —
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
