//! Find references (LSP-10): semua pemakaian identifier dengan word-boundary.
//! Berpasangan dengan `find_definition` di `backend.rs`.
use super::*;

impl LspBackend {
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

