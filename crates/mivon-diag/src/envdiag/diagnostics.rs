use super::EmitterHandle;
use crate::diagnostics::{DiagLevel, DiagSink, Diagnostic};

/// DiagnosticsContext — pusat penerima diagnostik dari seluruh pipeline.
///
/// Posisi dalam dependency rule: hanya MENERIMA data hasil proses, tidak
/// pernah menjalankan parser/compiler. Compiler cukup `diagnostics.report(diag)`.
pub struct DiagnosticsContext {
    sink: DiagSink,
    emitter: EmitterHandle,
    warnings_are_errors: bool,
}

impl DiagnosticsContext {
    pub fn new() -> Self {
        DiagnosticsContext {
            sink: DiagSink::new(),
            emitter: EmitterHandle::new(),
            warnings_are_errors: false,
        }
    }

    pub fn with_warnings_as_errors(mut self, on: bool) -> Self {
        self.warnings_are_errors = on;
        self
    }

    /// Sink mentah (MPSC, thread-safe) untuk push lintas thread.
    pub fn sink(&self) -> &DiagSink {
        &self.sink
    }

    pub fn emitter(&self) -> &EmitterHandle {
        &self.emitter
    }

    /// Catat + kirim ke sink (bukan output langsung).
    pub fn push(&self, diag: Diagnostic) {
        self.sink.push(diag);
    }

    /// Catat + emit ke terminal.
    pub fn emit(&self, diag: &Diagnostic) {
        self.emitter.emit(diag);
    }

    /// Kirim + emit (paket lengkap: sink tersimpan, terminal tercetak).
    pub fn report(&self, diag: Diagnostic) {
        self.sink.push(diag.clone());
        self.emitter.emit(&diag);
    }

    /// Ambil seluruh diagnostik tersimpan (sorted by file/posisi).
    pub fn drain(&self) -> Vec<Diagnostic> {
        self.sink.diagnostics()
    }

    pub fn error_count(&self) -> usize {
        self.sink.error_count()
    }

    pub fn has_errors(&self) -> bool {
        self.sink.has_errors()
    }

    /// Apakah `warning` harus dianggap error (config warnings_are_errors).
    pub fn is_fatal(&self, diag: &Diagnostic) -> bool {
        diag.is_error() || (self.warnings_are_errors && diag.level == DiagLevel::Warning)
    }
}

impl Default for DiagnosticsContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::DiagCode;

    #[test]
    fn test_report_counts() {
        let ctx = DiagnosticsContext::new();
        ctx.report(Diagnostic::error(DiagCode::ModuleNotFound, "m1"));
        ctx.report(Diagnostic::warning(DiagCode::WaveformError, "w1"));
        ctx.report(Diagnostic::error(DiagCode::InvalidSyntax, "m2"));
        assert_eq!(ctx.error_count(), 2);
        assert!(ctx.has_errors());
        assert_eq!(ctx.drain().len(), 3);
    }

    #[test]
    fn test_sink_counter() {
        let ctx = DiagnosticsContext::new();
        ctx.push(Diagnostic::warning(DiagCode::WaveformError, "w"));
        ctx.push(Diagnostic::warning(DiagCode::WaveformError, "w2"));
        assert_eq!(
            ctx.sink()
                .total_pushed
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        assert!(!ctx.has_errors());
    }

    #[test]
    fn test_warnings_as_errors() {
        let ctx = DiagnosticsContext::new().with_warnings_as_errors(true);
        let w = Diagnostic::warning(DiagCode::WaveformError, "w");
        let e = Diagnostic::error(DiagCode::InvalidSyntax, "e");
        assert!(ctx.is_fatal(&w), "warning harus fatal saat flag aktif");
        assert!(ctx.is_fatal(&e));
        // Flag mati: warning bukan fatal.
        let off = DiagnosticsContext::new();
        assert!(!off.is_fatal(&w));
    }
}
