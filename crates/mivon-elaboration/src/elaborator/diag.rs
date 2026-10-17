//! Diagnostic helpers elaborator — extract dari mod.rs untuk Arsitektur-06
//! (elaborator monolithic → submodule).
//!
//! Seluruh method di sini adalah `impl Elaborator` (butuh field
//! source_lines/source_file/cache/current_module/diag_sink); call site
//! (`self.elab_diag...`) tidak berubah.

use mivon_core::diagnostics::diagnostic::{
    DiagCode, DiagLevel, Diagnostic, RuntimeContext, SourceSnippet,
};
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

use super::Elaborator;

/// Batas lebar packed vector (bit) yang masih di-elaborate. Di atas batas ini
/// input ditolak dengan diag bersih (E3012) — bukan clamp senyap / hang /
/// OOM. Nilai jauh di atas packed terbesar realistis (OpenTitan SRAM elem 76
/// bit; datapath kripto 4096 bit) tapi di bawah ambang alokasi absurd.
/// Temuan fuzzer: `parameter W = 2**31` lalu `logic [W-1:0]` → elab hang
/// (loop init 256 × 2^31 iterasi) di inisialisasi array.
const MAX_PACKED_WIDTH: usize = 1 << 24;

impl Elaborator {
    /// Buat structured diagnostic untuk elaboration error dengan error code tepat.
    pub(crate) fn elab_diag(&self, code: DiagCode, message: impl Into<String>) -> SimError {
        self.elab_diag_at(code, message, 0, 0)
    }

    /// Tolak packed width di atas `MAX_PACKED_WIDTH` dgn diag bersih (E3012).
    /// Mencegah hang/OOM/silent-clamp pada width absurd (mis. `2**31`).
    /// Nama module yang sedang di-elaborasi disertakan dalam pesan — tanpa
    /// ini error E3012 tercatat tanpa konteks modul dan `find_name_in_source`
    /// (fallback lokasi) bisa menunjuk ke kemunculan nama port yang sama di
    /// FILE/modul lain (pesan menyesatkan).
    pub(crate) fn check_packed_width(&self, width: usize, what: &str) -> Result<(), SimError> {
        if width > MAX_PACKED_WIDTH {
            let mod_ctx = self
                .current_module
                .map(|m| format!(" in module '{}'", m.as_str()))
                .unwrap_or_default();
            return Err(self.elab_diag(
                DiagCode::PackedWidthTooLarge,
                format!(
                    "{what}{mod_ctx}: packed width {width} bit exceeds the implementation limit \
                     ({MAX_PACKED_WIDTH} bits) — refusing to elaborate an impractical \
                     vector (hang/OOM/incorrect truncation)"
                ),
            ));
        }
        Ok(())
    }

    /// Extract source file name from `line directives in source_lines for a given line.
    /// The source_lines contain `` `line 1 "filename.sv" `` directives from preprocessing.
    /// Resolve nama file + baris relatif-file untuk posisi di merged source.
    pub(crate) fn resolve_source_location(&self, line: usize) -> (String, usize) {
        mivon_core::diagnostics::resolve_source_location(
            &self.source_lines,
            &self.source_file,
            line,
        )
    }

    /// Cari posisi (baris, kolom) kemunculan pertama sebuah nama di merged
    /// source. Dipakai sebagai FALLBACK saat error/warning elaboration tidak
    /// membawa lokasi (line=0,col=0): nama diekstrak dari pesan `'X'` lalu
    /// baris pertama yang memuat X dijadikan posisi snippet. Baris directive
    /// `` `line `` dilewati (bukan konten user). Mengembalikan (0,0) bila nama
    /// tidak bisa diekstrak / tidak ditemukan — diagnostic tetap tanpa snippet.
    pub(crate) fn find_name_in_source(&self, message: &str) -> (usize, usize) {
        let name = match message.find('\'') {
            Some(s) => {
                let rest = &message[s + 1..];
                match rest.find('\'') {
                    Some(e) => &rest[..e],
                    None => return self.find_literal_in_source(message),
                }
            }
            None => {
                // Pesan dengan string literal (mis. `invalid lvalue expression:
                // string literal "fz.sv"`) memakai double-quote — ambil isinya
                // agar posisi bisa ditemukan di source.
                let Some(s) = message.find('"') else {
                    return self.find_literal_in_source(message);
                };
                let rest = &message[s + 1..];
                match rest.find('"') {
                    Some(e) => &rest[..e],
                    None => return self.find_literal_in_source(message),
                }
            }
        };
        if name.is_empty() || name.len() > 128 {
            return self.find_literal_in_source(message);
        }
        // Nama yang TIDAK plausibel sebagai token source (mengandung spasi,
        // kurung kurawal, dll. — mis. debug-format `Ident { name: Symbol(...)
        // }.mosi` dari pesan width-mismatch) TIDAK akan pernah ditemukan di
        // source. Tanpa guard ini, setiap nama unik memaksa scan penuh 1.1M
        // baris merged source (~1.5-2s) — module TB/DV dengan banyak diagnostic
        // seperti itu (mis. spid_status_tb) bisa memakan 100+ detik.
        // Karakter yang diizinkan: identifier SV + kualifikasi `::` +
        // member access `.` + array `[]` + `$` (system) + angka.
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '[' | ']' | '$' | '-'))
        {
            return self.find_literal_in_source(message);
        }
        // Pure function dari source_lines — memoize per nama. Tanpa ini setiap
        // diagnostic tanpa lokasi meng-scan seluruh merged source (1.1M baris
        // di OpenTitan) yang memakan ~1.5-2s per panggilan.
        let sym = Symbol::intern(name);
        if let Some(&loc) = self.source_name_loc.borrow().get(&sym) {
            return loc;
        }
        let result = (|| {
            // Cocokkan nama sebagai TOKEN utuh (bukan substring): fallback
            // lokasi untuk `in_i` tidak boleh menunjuk ke `tl_in_i`/`in_io` di
            // file lain (snippet menyesatkan). Boundary luar = bukan
            // alphanumeric/'_'. Nama bisa memuat `::`/`.`/`[]` — substring
            // utuh dibandingkan, hanya tepi luar yang dicek.
            let is_id = |c: char| c.is_alphanumeric() || c == '_';
            for (i, line) in self.source_lines.iter().enumerate() {
                if line.trim_start().starts_with('`') {
                    continue;
                }
                let mut start = 0usize;
                while let Some(rel) = line[start..].find(name) {
                    let abs = start + rel;
                    let before_ok = match line[..abs].chars().next_back() {
                        Some(c) => !is_id(c),
                        None => true,
                    };
                    let after_end = abs + name.len();
                    let after_ok = match line[after_end..].chars().next() {
                        Some(c) => !is_id(c),
                        None => true,
                    };
                    if before_ok && after_ok {
                        return (i + 1, abs + 1);
                    }
                    start = abs + 1;
                }
            }
            (0, 0)
        })();
        self.source_name_loc.borrow_mut().insert(sym, result);
        result
    }

    /// Fallback: pesan tanpa nama-quoted (mis. `fill literal ('0/'1/'x/'z)`)
    /// — cari pola literal di source (baris/lokasi assignment paling dekat).
    pub(crate) fn find_literal_in_source(&self, message: &str) -> (usize, usize) {
        // Hasil scan HANYA bergantung pada source_lines: targets literal
        // tetap (['0,'1,'x,'z,'X,'Z) dan message tidak dipakai untuk memilih
        // baris — jadi hasil konstan per Elaborator. Lihat komentar field
        // `source_literal_loc` (hotspot gdb: 3 sample identik, 100% CPU di
        // sini pada run OpenTitan).
        let _ = message;
        if let Some(loc) = *self.source_literal_loc.borrow() {
            return loc;
        }
        // Cari pola literal yang mungkin muncul di pesan: `'0`, `'1`, `'x`, `'z`.
        let targets = ["'0", "'1", "'x", "'z", "'X", "'Z"];
        let mut best: Option<(usize, usize)> = None;
        for (i, line) in self.source_lines.iter().enumerate() {
            if line.trim_start().starts_with('`') {
                continue;
            }
            let fits = targets.iter().find(|t| line.contains(**t));
            if let Some(t) = fits {
                if let Some(col) = line.find(t) {
                    // Pilih lokasi PALING BELAKANG di source (assignment
                    // biasanya di baris akhir blok) — memoize per-pesan tak
                    // perlu di sini (hasil konstan per source).
                    best = Some((i + 1, col + 1));
                }
            }
        }
        // Tidak menemukan literal — pakai baris terakhir source sebagai
        // anker (masih lebih baik dari tanpa lokasi).
        let loc = best.unwrap_or_else(|| {
            let n = self.source_lines.len();
            if n > 0 {
                (n, 1)
            } else {
                (0, 0)
            }
        });
        *self.source_literal_loc.borrow_mut() = Some(loc);
        loc
    }

    /// Buat error diagnostic dengan posisi source.
    pub(crate) fn elab_diag_at(
        &self,
        code: DiagCode,
        message: impl Into<String>,
        line: usize,
        col: usize,
    ) -> SimError {
        let msg: String = message.into();
        let (line, col) = if line > 0 && col > 0 {
            (line, col)
        } else {
            // Fallback global: cari nama dari pesan di source (mis.
            // `'UVM_HDL_MAX_WIDTH' not found in parameter context` dari
            // resolve_param_values yang tidak membawa lokasi).
            self.find_name_in_source(&msg)
        };
        let mut diag = Diagnostic::new(DiagLevel::Error, code, msg).with_code_context();
        if line > 0 && line <= self.source_lines.len() {
            let source_line = &self.source_lines[line - 1];
            let (file, display_line) = self.resolve_source_location(line);
            let snippet = SourceSnippet::new(file, display_line, col, source_line);
            diag = diag.with_source_snippet(snippet);
        }
        if let Some(ref mod_name) = self.current_module {
            let ctx = RuntimeContext::new().with_module(mod_name.as_str());
            diag = diag.with_runtime_context(ctx);
        }
        SimError::from_elab_diagnostic(diag)
    }

    /// Emit warning diagnostic ke DiagSink (elaboration-time warnings).
    #[allow(dead_code)]
    pub(crate) fn elab_warn(&self, code: DiagCode, message: impl Into<String>) {
        self.elab_warn_at(code, message, 0, 0)
    }

    /// Emit warning dengan posisi source.
    pub(crate) fn elab_warn_at(
        &self,
        code: DiagCode,
        message: impl Into<String>,
        line: usize,
        col: usize,
    ) {
        let msg: String = message.into();
        let (line, col) = if line > 0 && col > 0 {
            (line, col)
        } else {
            // Fallback global (lihat elab_diag_at): `unknown type 'X' is not
            // defined in this scope` (resolve_type_width) dipanggil tanpa
            // lokasi — posisi diambil dari baris pemakaian nama type.
            self.find_name_in_source(&msg)
        };
        let mut diag = Diagnostic::new(DiagLevel::Warning, code, msg).with_code_context();
        if line > 0 && line <= self.source_lines.len() {
            let source_line = &self.source_lines[line - 1];
            let (file, display_line) = self.resolve_source_location(line);
            let snippet = SourceSnippet::new(file, display_line, col, source_line);
            diag = diag.with_source_snippet(snippet);
        }
        self.diag_sink.push(diag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ast::types::Design;

    fn elab_with_lines(lines: &[&str]) -> Elaborator {
        Elaborator::with_source(
            Design::default(),
            lines.iter().map(|s| s.to_string()).collect(),
            "t.sv".to_string(),
        )
    }

    #[test]
    fn test_find_name_token_boundary() {
        // `in_i` tidak boleh match `tl_in_i` (substring) — token utuh.
        let e = elab_with_lines(&["module tl_in_i;", "wire in_i;"]);
        assert_eq!(e.find_name_in_source("'in_i' not found"), (2, 6));
        assert_eq!(e.find_name_in_source("'tl_in_i' not found"), (1, 8));
        // Tanpa quoted → fallback literal → anker baris terakhir.
        assert_eq!(e.find_name_in_source("tanpa quoted"), (2, 1));
        let empty = elab_with_lines(&[]);
        assert_eq!(empty.find_name_in_source("tanpa quoted"), (0, 0));
    }
}
