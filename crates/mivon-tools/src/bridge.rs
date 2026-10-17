//! Bridge konversi tipe diagnostik paralel → `mivon_core::diagnostics::Diagnostic`.
//!
//! Tujuannya: SEMUA jalur error (SimError, mv, micd, gui/fuzz kelak) dapat
//! dirender lewat renderer tunggal `format_diagnostic` tanpa duplikasi logika
//! formatting. Tipe asli TIDAK dihapus (pendekatan konservatif) — bridge ini
//! hanya menyediakan titik konversi bersih (`to_diagnostic`/`into`).
//!
//! `mivon-mv` sengaja murni-std (tanpa dependensi eksternal), sehingga
//! konversi untuk `MvError`/`MvWarning` diletakkan di sini sebagai free
//! function alih-alih method, agar desain std-only mv tetap utuh.

use mivon_core::diagnostics::{DiagCode, DiagLevel, Diagnostic};
use mivon_mv::{MvError, MvWarning};

/// Konversi `MvError` (.mv) → `Diagnostic` core (render via `format_diagnostic`).
///
/// Posisi `line:col` disertakan bila bukan nol. Kode `DiagCode` bersifat
/// netral (placeholder) karena ruang kode .mv (`E2xxx`) tidak dipetakan ke
/// enum `DiagCode` core; pesan lengkap tetap lossless.
pub fn mv_error_to_diagnostic(e: &MvError) -> Diagnostic {
    let message = if e.line == 0 && e.col == 0 {
        e.msg.clone()
    } else {
        format!("{}:{}: {}", e.line, e.col, e.msg)
    };
    Diagnostic::new(DiagLevel::Error, DiagCode::InvalidSyntax, message)
}

/// Konversi `MvWarning` (.mv) → `Diagnostic` core.
/// Kode hazard (`W3xxx`) dibawa ke dalam pesan agar tidak hilang.
pub fn mv_warning_to_diagnostic(w: &MvWarning) -> Diagnostic {
    let pos = if w.line == 0 && w.col == 0 {
        String::new()
    } else {
        format!("{}:{}: ", w.line, w.col)
    };
    let message = format!("{}[{}] {}", pos, w.code, w.msg);
    Diagnostic::new(DiagLevel::Warning, DiagCode::InvalidSyntax, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::diagnostics::format_diagnostic;
    use mivon_mv::{MvError, MvWarning};

    #[test]
    fn test_mv_error_bridge_renders() {
        let e = MvError::new(3, 5, "unexpected token".to_string());
        let d = mv_error_to_diagnostic(&e);
        let s = format_diagnostic(&d);
        assert!(s.contains("unexpected token"));
        assert!(s.contains("error"));
        assert!(s.contains("3:5"));
    }

    #[test]
    fn test_mv_error_bridge_no_position() {
        let e = MvError::new(0, 0, "boom".to_string());
        let d = mv_error_to_diagnostic(&e);
        let s = format_diagnostic(&d);
        assert!(s.contains("boom"));
        assert!(!s.contains("0:0"));
    }

    #[test]
    fn test_mv_warning_bridge_renders() {
        let w = MvWarning::new(2, 1, "W3001", "unused signal".to_string());
        let d = mv_warning_to_diagnostic(&w);
        let s = format_diagnostic(&d);
        assert!(s.contains("unused signal"));
        assert!(s.contains("W3001"));
        assert!(s.contains("warning"));
        assert!(s.contains("2:1"));
    }
}
