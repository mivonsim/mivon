//! Find references (LSP-10): semua pemakaian identifier dengan word-boundary.
//! Berpasangan dengan `find_definition` di `backend.rs`.
use super::*;

impl LspBackend {
    /// Find-references core (murni, bisa diuji) — LSP-04 tahap 1:
    /// semua kemunculan identifier (sebagai token mandiri) dalam dokumen,
    /// urut baris. Deklarasi SELALU ikut di sini; pemfilteran sesuai
    /// `include_declaration` dilakukan `references_hits` (dipakai handler).
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
                    // LSP-26/29: kolom token = offset byte → kolom UTF-16
                    // sebelum masuk Range LSP.
                    out.push((
                        idx as u32,
                        Self::byte_col_to_utf16(text, col as u32),
                        Self::byte_col_to_utf16(text, (col + w.len()) as u32),
                    ));
                }
            }
        }
        out
    }

    /// Find-references lintas file (LSP-04 tahap 2): cari `word` di SEMUA
    /// dokumen (uri, teks) dengan aturan yang sama seperti tahap 1 —
    /// token mandiri (word-boundary) dan komentar baris `//` dilewati.
    ///
    /// Hasil: (uri, line, start_col, end_col) dengan kolom UTF-16 (siap
    /// menjadi `Position.character`, lihat LSP-26), urut sesuai urutan `docs`.
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
            if !Self::collect_refs_in_text(uri, text, word, &mut out, max_results) {
                return out;
            }
        }
        out
    }

    /// LSP-04: kumpulkan PEMAKAIAN `word` (deklarasi dibuang) ke `out`,
    /// berhenti saat `max` tercapai. Return false bila `max` tercapai.
    ///
    /// Deteksi deklarasi memakai `definition_col` — heuristik yang sama
    /// dengan go-to-definition. Biaya terbatas: `definition_col` dihitung
    /// SEKALI per baris yang memuat `word`, dan hasil langsung dipotong di
    /// `max` (tidak ada pengumpulan tanpa batas — review LSP-04 PERF-1).
    pub(crate) fn collect_uses_in_text(
        uri: &str,
        text: &str,
        word: &str,
        out: &mut Vec<(String, u32, u32, u32)>,
        max: usize,
    ) -> bool {
        for (idx, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut decl: Option<Option<usize>> = None;
            for (w, col) in Self::line_tokens(line) {
                if w != word {
                    continue;
                }
                let decl_col = *decl.get_or_insert_with(|| Self::definition_col(line, word));
                if decl_col == Some(col) {
                    continue;
                }
                out.push((
                    uri.to_string(),
                    idx as u32,
                    Self::byte_col_to_utf16(line, col as u32),
                    Self::byte_col_to_utf16(line, (col + w.len()) as u32),
                ));
                if out.len() >= max {
                    return false;
                }
            }
        }
        true
    }

    /// LSP-04: hasil find-references sesuai `include_declaration`.
    ///
    /// Bila deklarasi TIDAK diminta, filter dilakukan SAAT pengumpulan
    /// (sebelum memotong ke `max`) — kalau tidak, kuota bisa habis oleh
    /// hit deklarasi dan pemakaian nyata di dokumen berikutnya hilang
    /// (review LSP-04). Handler cukup meneruskan
    /// `params.context.include_declaration` ke sini.
    pub(crate) fn references_hits(
        docs: &[(String, String)],
        word: &str,
        include_declaration: bool,
        max: usize,
    ) -> Vec<(String, u32, u32, u32)> {
        if word.is_empty() || max == 0 {
            return Vec::new();
        }
        if include_declaration {
            return Self::find_references_in_docs(docs, word, max);
        }
        let mut out = Vec::new();
        for (uri, text) in docs {
            if !Self::collect_uses_in_text(uri, text, word, &mut out, max) {
                break;
            }
        }
        out
    }

    /// Kumpulkan kemunculan `word` dalam SATU dokumen ke `out` (helper
    /// bersama find-references & rename). Return false bila `max` tercapai
    /// (pemanggil berhenti).
    pub(crate) fn collect_refs_in_text(
        uri: &str,
        text: &str,
        word: &str,
        out: &mut Vec<(String, u32, u32, u32)>,
        max: usize,
    ) -> bool {
        for (idx, line) in text.lines().enumerate() {
            // Defensif (line_tokens sudah berhenti di `//`): baris komentar
            // penuh tidak pernah menghasilkan token.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for (w, col) in Self::line_tokens(line) {
                if w == word {
                    out.push((
                        uri.to_string(),
                        idx as u32,
                        Self::byte_col_to_utf16(line, col as u32),
                        Self::byte_col_to_utf16(line, (col + w.len()) as u32),
                    ));
                    if out.len() >= max {
                        return false;
                    }
                }
            }
        }
        true
    }
}
