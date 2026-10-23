//! Diagnostics context — wrapper tipis di atas `DiagSink` + emitter terminal.
//!
//! Aturan "1 pusat diag": satu-satunya koleksi adalah `DiagSink` dan satu-
//! satuannya renderer adalah `format_diagnostic`. Context ini TIDAK punya
//! sistem stats kedua — agregat cukup dari `DiagSink.total_pushed` dan
//! engine global (`diag_global()`, counter per level/kode).

mod diagnostics;
mod emitter;

pub use diagnostics::DiagnosticsContext;
pub use emitter::EmitterHandle;
