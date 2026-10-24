//! diagnostics.mdb — database diagnostic per file.
//!
//! IDE/LSP/GUI cukup query file → daftar diagnostic tanpa compile ulang.
//!
//! Satu sumber tipe (konsolidasi diag 2026-10): `DiagEntry`/`FileDiags`
//! (bersama tipe rich-entry Kritik-12: span/related/fixit/code-action) hidup
//! di `mivon-diag`; micd HANYA re-export — compiler crate consumer murni
//! tanpa definisi diag sendiri. Alias `Span` dipertahankan utk stabilitas
//! API internal (pusat = `TextSpan`); wire format (urutan field bincode)
//! identik → tanpa bump VERSION MICD.

pub use mivon_core::diagnostics::entry::{DiagEntry, FileDiags};
pub use mivon_core::diagnostics::TextSpan as Span;
pub use mivon_core::diagnostics::{CodeAction, FixIt, RelatedInfo};
