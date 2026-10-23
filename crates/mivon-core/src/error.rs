use crate::diagnostics::diagnostic::{DiagCode, DiagLevel, Diagnostic, SourceSnippet};
use crate::diagnostics::emitter::format_diagnostic;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum SimError {
    Parse(String),
    Elaborate(String),
    Runtime(String),
    Preprocessor(String),
    Waveform(String),
    Debugger(String),
    Io(std::io::ErrorKind, String),
    /// Error yang membawa Diagnostic terstruktur (source snippet, error code, dll).
    /// Ini adalah varian modern — menggantikan Parse/String untuk error dengan konteks kaya.
    Diagnostic(Diagnostic),
}

impl SimError {
    pub fn new(line: Option<usize>, message: impl Into<String>) -> Self {
        let msg = message.into();
        match line {
            Some(line) => SimError::Parse(format!("line {}: {}", line, msg)),
            None => SimError::Runtime(msg),
        }
    }

    pub fn parse(msg: impl Into<String>) -> Self {
        SimError::Parse(msg.into())
    }
    pub fn elaborate(msg: impl Into<String>) -> Self {
        SimError::Elaborate(msg.into())
    }
    pub fn runtime(msg: impl Into<String>) -> Self {
        SimError::Runtime(msg.into())
    }
    pub fn preprocessor(msg: impl Into<String>) -> Self {
        SimError::Preprocessor(msg.into())
    }
    pub fn waveform(msg: impl Into<String>) -> Self {
        SimError::Waveform(msg.into())
    }
    pub fn debugger(msg: impl Into<String>) -> Self {
        SimError::Debugger(msg.into())
    }

    /// Buat error dengan structured Diagnostic (error code, konteks, dll).
    /// Menggantikan format string-based `[CODE] message` dengan Diagnostic nyata.
    pub fn with_diag(code: DiagCode, message: impl Into<String>) -> Self {
        let diag = Diagnostic::new(DiagLevel::Error, code, message.into()).with_code_context();
        Self::Diagnostic(diag)
    }

    /// Buat runtime error dari Diagnostic struct.
    /// OBSOLETE: Gunakan SimError::Diagnostic(diag) langsung.
    pub fn from_diagnostic(diag: &Diagnostic) -> Self {
        Self::Diagnostic(diag.clone())
    }

    /// Buat error parser dari Diagnostic yang sudah jadi dengan source snippet.
    pub fn from_parse_diagnostic(diag: Diagnostic) -> Self {
        Self::Diagnostic(diag)
    }

    /// Buat error elaboration dari Diagnostic.
    pub fn from_elab_diagnostic(diag: Diagnostic) -> Self {
        Self::Diagnostic(diag)
    }

    /// Ekstrak error code dari message format "[RT0001] message"
    fn parse_error_code(msg: &str) -> Option<&'static str> {
        if msg.starts_with('[') {
            if let Some(end) = msg.find(']') {
                let code_str = &msg[1..end];
                if let Some(code) = crate::diagnostics::codes::lookup_code(code_str) {
                    return Some(code.as_str());
                }
            }
        }
        None
    }

    /// Try to extract file:line:col from common error format.
    fn extract_location(msg: &str) -> (Option<String>, Option<usize>, Option<usize>) {
        // Format: "filename:line:col: message"
        let parts: Vec<&str> = msg.splitn(4, ':').collect();
        if parts.len() >= 4 {
            let file = parts[0].trim().to_string();
            if let Ok(line) = parts[1].trim().parse::<usize>() {
                if let Ok(col) = parts[2].trim().parse::<usize>() {
                    return (Some(file), Some(line), Some(col));
                }
            }
        }
        // Format: "line N: message"
        if let Some(rest) = msg.strip_prefix("line ") {
            if let Some(end) = rest.find(':') {
                if let Ok(line) = rest[..end].trim().parse::<usize>() {
                    return (None, Some(line), None);
                }
            }
        }
        (None, None, None)
    }

    /// Dapatkan error code yang sesuai.
    pub fn error_code(&self) -> &'static str {
        let msg = self.to_string();

        // Coba parse dari format "[RT0001] message"
        if let Some(code) = Self::parse_error_code(&msg) {
            return code;
        }

        // For Diagnostic variant, use its code directly
        if let SimError::Diagnostic(diag) = self {
            return diag.code.as_str();
        }

        match self {
            // String variant: kode statis per varian — TANPA tebak dari isi
            // pesan (aturan "line/col/kode 100% akurat"). Kode spesifik
            // (E3001/E3002/E3005, dll.) seharusnya dibawa lewat jalur
            // Diagnostic (`SimError::with_diag`/`from_*_diagnostic`) oleh
            // produsen, bukan ditebak dari substring.
            SimError::Parse(_) => DiagCode::InvalidSyntax.as_str(),
            // Elaborate tanpa Diagnostic = fallback generik.
            SimError::Elaborate(_) => "E3004",
            SimError::Runtime(_) => "E9001",
            SimError::Preprocessor(_) => "E0101",
            SimError::Waveform(_) => "W0001",
            SimError::Debugger(_) => "E0003",
            SimError::Io(_, _) => "E0004",
            SimError::Diagnostic(diag) => diag.code.as_str(),
        }
    }

    /// Ekstrak posisi source (line, col) dari error, jika tersedia.
    /// Prioritas: source_snippet → span pertama → (0,0).
    pub fn source_position(&self) -> (usize, usize) {
        if let SimError::Diagnostic(diag) = self {
            if let Some(snippet) = &diag.source_snippet {
                return (snippet.line, snippet.col);
            }
            if let Some(span) = diag.spans.first() {
                return (span.start as usize, span.end as usize);
            }
        }
        (0, 0)
    }

    /// Convert ke Diagnostic struct untuk formatting penuh.
    /// Jika message mengandung "[CODE] ...", extract code dan message terpisah.
    pub fn to_diagnostic(&self) -> Diagnostic {
        // Fast path: already a Diagnostic — return clone immediately
        // (strip nested error codes juga di sini karena with_diag menghasilkan
        //  SimError::Diagnostic langsung, yang men-bypass jalur strip di bawah)
        if let SimError::Diagnostic(diag) = self {
            let mut diag = diag.clone();
            diag.message = Self::strip_nested_error_codes(&diag.message).into();
            return diag;
        }

        let msg = self.to_string();
        let (level, code) = match self {
            SimError::Parse(_) => (DiagLevel::Error, DiagCode::InvalidSyntax),
            SimError::Elaborate(_) => (DiagLevel::Error, DiagCode::SimulationError),
            SimError::Runtime(_) => (DiagLevel::Error, DiagCode::SimulationError),
            SimError::Preprocessor(_) => (DiagLevel::Error, DiagCode::PreprocessorError),
            SimError::Waveform(_) => (DiagLevel::Error, DiagCode::WaveformError),
            SimError::Debugger(_) => (DiagLevel::Error, DiagCode::DebuggerError),
            SimError::Io(_, _) => (DiagLevel::Error, DiagCode::IoError),
            SimError::Diagnostic(_) => unreachable!(), // caught by fast path above
        };

        // Jika ada format "[CODE] message", extract code dan gunakan message bersih
        let clean_msg = if msg.starts_with('[') {
            if let Some(end) = msg.find(']') {
                if end + 2 < msg.len() {
                    msg[end + 2..].to_string()
                } else {
                    msg.clone()
                }
            } else {
                msg.clone()
            }
        } else {
            msg.clone()
        };

        // Hapus kode error nested (mis. "error[E0101]: ") dari dalam message agar
        // tidak ada duplikasi kode saat error dibungkus berlapis (preprocessor, dll).
        let clean_msg = Self::strip_nested_error_codes(&clean_msg);

        let mut diag = Diagnostic::new(level, code, clean_msg);

        // Try to reconstruct source snippet from flat string format like "file:line:col: message"
        if let (Some(file), Some(line), Some(col)) = Self::extract_location(&msg) {
            let parts: Vec<&str> = msg.splitn(4, ':').collect();
            let error_msg = if parts.len() >= 4 {
                parts[3..].join(":").trim().to_string()
            } else {
                msg.clone()
            };

            let snippet = SourceSnippet::new(file, line, col, error_msg);
            diag = diag.with_source_snippet(snippet);
        }

        // Tambah explanation + help dari DiagCode untuk diagnostic yang kaya
        diag = diag.with_code_context();

        diag
    }

    /// Hapus kode error nested seperti "error[E0101]: " dari dalam message.
    /// Hanya strip pattern yang valid `error[EXXXX]: ` agar tidak merusak konten lain.
    /// Case-sensitive karena formatter kita selalu memancarkan `error[` huruf kecil.
    fn strip_nested_error_codes(msg: &str) -> String {
        let mut result = msg.to_string();
        while let Some(idx) = result.find("error[") {
            let Some(rel_end) = result[idx + 6..].find(']') else {
                break;
            };
            let end = idx + 6 + rel_end;
            let code = &result[idx + 6..end];
            // Validasi format kode: EXXXX (huruf E + 4 digit)
            let valid = code.len() == 5
                && code.as_bytes()[0] == b'E'
                && code.as_bytes()[1..].iter().all(|b| b.is_ascii_digit());
            if !valid {
                break;
            }
            let Some(stripped) = result[end + 1..].strip_prefix(": ") else {
                break;
            };
            result = format!("{}{}", &result[..idx], stripped);
        }
        result
    }

    /// Format error sebagai string dengan konteks.
    /// Render error lewat renderer tunggal `format_diagnostic` untuk SEMUA
    /// varian (konsisten dengan jalur `Diagnostic`). `to_diagnostic` sudah
    /// membangun `Diagnostic` kaya (source snippet + context) per varian.
    pub fn format_with_context(&self) -> String {
        format_diagnostic(&self.to_diagnostic())
    }

    /// Get exit code for CI/CD integration (Rule 9).
    /// 0  Success
    /// 10 Parse Error
    /// 20 Semantic Error
    /// 30 Elaboration Error
    /// 40 Top Resolution Failed
    /// 50 Scheduler Failed
    /// 60 Simulation Failed
    /// 70 DPI Link Failed
    pub fn exit_code(&self) -> i32 {
        let code = self.error_code();
        match code {
            // Parse errors (E1xxx)
            c if c.starts_with("E1") => 10,
            // Semantic errors (E2xxx)
            c if c.starts_with("E2") => 20,
            // Top resolution failed (E3005 — dahulu "EL3001"; exit 40 dipertahankan,
            // arm harus SEBELUM guard E3 supaya tidak tertangkap sebagai E3 biasa)
            "E3005" => 40,
            // Elaboration errors (E3xxx)
            c if c.starts_with("E3") => 30,
            // Runtime Scheduler (RT2xxx)
            c if c.starts_with("RT2") => 50,
            // Runtime Simulation (RT7xxx assertions, RT1xxx signals, RT9xxx internal)
            c if c.starts_with("RT7")
                || c.starts_with("RT1")
                || c.starts_with("RT9")
                || c == "E9001" =>
            {
                60
            }
            // DPI Link failed (RT8xxx)
            c if c.starts_with("RT8") => 70,
            // Preprocessor, IO, Debugger, Waveform
            "E0101" | "E0003" | "E0004" | "E0005" | "W0001" => 10,
            _ => 1,
        }
    }
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimError::Parse(msg) => write!(f, "{}", msg),
            SimError::Elaborate(msg) => write!(f, "{}", msg),
            SimError::Runtime(msg) => write!(f, "{}", msg),
            SimError::Preprocessor(msg) => write!(f, "{}", msg),
            SimError::Waveform(msg) => write!(f, "{}", msg),
            SimError::Debugger(msg) => write!(f, "{}", msg),
            SimError::Io(kind, msg) => write!(f, "I/O error ({}): {}", kind, msg),
            SimError::Diagnostic(diag) => {
                // Sertakan file:line:col dari source_snippet jika ada;
                // fallback ke span pertama bila snippet kosong.
                match &diag.source_snippet {
                    Some(ss) if ss.line > 0 => {
                        write!(
                            f,
                            "{}[{}]: {}\n  --> {}:{}:{}",
                            diag.level, diag.code, diag.message, ss.file, ss.line, ss.col
                        )
                    }
                    None => {
                        if let Some(sp) = diag.spans.first() {
                            write!(
                                f,
                                "{}[{}]: {}\n  --> {}:{}:{}",
                                diag.level, diag.code, diag.message, sp.file, sp.start, sp.end
                            )
                        } else {
                            write!(f, "{}[{}]: {}", diag.level, diag.code, diag.message)
                        }
                    }
                    _ => write!(f, "{}[{}]: {}", diag.level, diag.code, diag.message),
                }
            }
        }
    }
}

impl std::error::Error for SimError {}

impl From<std::io::Error> for SimError {
    fn from(e: std::io::Error) -> Self {
        SimError::Io(e.kind(), e.to_string())
    }
}

impl From<String> for SimError {
    fn from(msg: String) -> Self {
        let is_parse = msg.starts_with("line ") && msg[5..].find(':').is_some();
        if is_parse {
            SimError::Parse(msg)
        } else {
            SimError::Runtime(msg)
        }
    }
}

impl From<&str> for SimError {
    fn from(msg: &str) -> Self {
        SimError::Runtime(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Top resolution failed = kode E3005 (DiagCode::TopResolutionFailed —
    /// dibawa TERSTRUKTUR via `with_diag`, bukan ketebalan dari isi pesan)
    /// dan HARUS tetap exit 40 — arm E3005 wajib berada sebelum guard
    /// `starts_with("E3") => 30` supaya tidak tertangkap E3 biasa.
    #[test]
    fn top_resolution_exit_code_stays_40() {
        let e = SimError::with_diag(
            DiagCode::TopResolutionFailed,
            "Unable to determine top-level design",
        );
        assert_eq!(e.error_code(), "E3005");
        assert_eq!(e.exit_code(), 40);
    }

    /// Elaborate tanpa Diagnostic = fallback generik E3004 (exit 30). Kode
    /// spesifik (E3001/E3002/E3003) hanya dari produsen yang membawa Diagnostic.
    #[test]
    fn unstructured_elaborate_defaults_generic_e3004() {
        let e = SimError::Elaborate("module foo not found".into());
        assert_eq!(e.error_code(), "E3004");
        assert_eq!(e.exit_code(), 30);
    }

    #[test]
    fn format_with_context_uses_unified_renderer() {
        let e = SimError::Runtime("null handle access".into());
        let s = e.format_with_context();
        // Header kaya dari renderer tunggal: level + [code] + pesan.
        assert!(s.contains("null handle access"));
        assert!(s.contains("error["));
    }
}
