//! Emitter diagnostik — membungkus `TerminalEmitter` dengan mode simple/pretty.

use crate::diagnostics::{Diagnostic, TerminalEmitter};

/// Handle ke emitter terminal. Bisa diclone ringan (Clone di belakangnya).
#[derive(Debug, Clone)]
pub struct EmitterHandle {
    simple_mode: bool,
}

impl EmitterHandle {
    pub fn new() -> Self {
        EmitterHandle { simple_mode: false }
    }

    pub fn simple_mode(mut self, on: bool) -> Self {
        self.simple_mode = on;
        self
    }

    /// Emit satu diagnostic ke terminal.
    pub fn emit(&self, diag: &Diagnostic) {
        let mut emitter = TerminalEmitter::new();
        if self.simple_mode {
            emitter = emitter.with_simple_mode(true);
        }
        let _ = emitter.emit(diag);
    }

    /// Emit daftar diagnostic (menggunakan emitter baru per item).
    pub fn emit_all(&self, diags: &[Diagnostic]) {
        for d in diags {
            self.emit(d);
        }
    }
}

impl Default for EmitterHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::DiagCode;

    #[test]
    fn test_emitter_new() {
        let e = EmitterHandle::new();
        // Emit tidak panic untuk diag apa pun.
        let d = Diagnostic::warning(DiagCode::WaveformError, "tes");
        e.emit(&d);
    }
}
