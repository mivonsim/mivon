//! Find references (LSP-10): semua pemakaian identifier dengan word-boundary.
//! Berpasangan dengan `find_definition` di `backend.rs`.
use super::*;

impl LspBackend {
    /// Find-references core (murni, bisa diuji) — LSP-04 tahap 1:
    /// semua kemunculan identifier (sebagai token mandiri) dalam dokumen,
    /// urut baris. `include_declaration` tetap dikembalikan penuh — caller
    /// (handler) yang memfilter bila diminta tanpa deklarasi.
    #[allow(dead_code)] // API tahap 1 (single-doc) — dipakai test; produksi
                        // memakai find_references_in_docs (tahap 2).
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

    /// Find-references lintas file (LSP-04 tahap 2): cari `word` di SEMUA
    /// dokumen (uri, teks) dengan aturan yang sama seperti tahap 1 —
    /// token mandiri (word-boundary) dan komentar baris `//` dilewati.
    ///
    /// Hasil: (uri, line, start_col, end_col), urut sesuai urutan `docs`.
    /// PEMANGGIL WAJIB mengurutkan `docs` (mis. by uri) agar urutan hasil —
    /// dan himpunan hasil saat `max_results` tercapai — deterministik
    /// (review R193 BUG-1: `documents` berupa HashMap, urutan acak).
    pub(crate) fn find_references_in_docs(
        docs: &[(String, String)],
        word: &str,
        max_results: usize,
    ) -> Vec<(String, u32, u32, u32)> {
        let mut out: Vec<(String, u32, u32, u32)> = Vec::new();
        if word.is_empty() || max_results == 0 {
            return out;
        }
        for (uri, text) in docs {
            for (idx, line) in text.lines().enumerate() {
                // Defensif (line_tokens sudah berhenti di `//`): baris
                // komentar penuh tidak pernah menghasilkan token.
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                for (w, col) in Self::line_tokens(line) {
                    if w == word {
                        out.push((uri.clone(), idx as u32, col as u32, (col + w.len()) as u32));
                        if out.len() >= max_results {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }
}
