//! collector — ubah output runner mivon → MivonObservation terstruktur.

#![cfg(feature = "dev")]

use super::{
    ExitStatus, MivonObservation, ObsDiagnostic, ObsLocation, ObsSeverity, ValueObservation,
};
use crate::lrm_model::LrmValue;
use crate::runner::{Kind, Outcome};

/// Bangun `MivonObservation` dari `Outcome` subprocess runner.
///
/// Ini jalur utama: runner.rs menjalankan mivon sebagai subprocess,
/// collector mengubah hasilnya menjadi struktur yang dapat dievaluasi hakim.
pub fn collect_from_runner(outcome: &Outcome) -> MivonObservation {
    let exit_status = map_kind(&outcome.kind);
    let diagnostics = parse_diagnostics(&outcome.stderr);

    MivonObservation {
        exit_status,
        diagnostics,
        elaboration: None, // diisi oleh in-process observer bila tersedia
        runtime: None,
        values: parse_signal_values(&outcome.stdout),
        scheduling: Vec::new(),
        raw_stdout: outcome.stdout.clone(),
        raw_stderr: outcome.stderr.clone(),
    }
}

/// Bangun `MivonObservation` dari hasil in-process mivon-api.
///
/// Jalur ini lebih kaya: bisa mengisi `elaboration` dan `values` langsung
/// dari `IrDesign` + signal state tanpa parsing teks.
#[cfg(feature = "dev")]
pub fn collect_from_api(
    exit_status: ExitStatus,
    stderr: String,
    signals: Vec<(String, mivon_ir::LogicVec)>,
) -> MivonObservation {
    let diagnostics = parse_diagnostics(&stderr);
    let values = signals
        .into_iter()
        .map(|(name, lv)| ValueObservation {
            signal: name,
            time: 0,
            value: logicvec_to_lrm(&lv),
        })
        .collect();

    MivonObservation {
        exit_status,
        diagnostics,
        elaboration: None,
        runtime: None,
        values,
        scheduling: Vec::new(),
        raw_stdout: String::new(),
        raw_stderr: stderr,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn map_kind(kind: &Kind) -> ExitStatus {
    match kind {
        Kind::Ok => ExitStatus::Ok,
        Kind::CleanError => ExitStatus::CleanError,
        Kind::Panic => ExitStatus::Panic,
        Kind::Abort => ExitStatus::Abort,
        Kind::Hang => ExitStatus::Hang,
        Kind::Crash(code) => ExitStatus::Crash { code: *code },
    }
}

/// Parse baris stderr mivon → daftar ObsDiagnostic.
///
/// Format mivon: `error[E1002]: message → file:line:col`
/// atau: `warning[WR0102]: message → file:line:col`
fn parse_diagnostics(stderr: &str) -> Vec<ObsDiagnostic> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let t = line.trim();
        if let Some(d) = try_parse_diag_line(t) {
            out.push(d);
        }
    }
    out
}

fn try_parse_diag_line(line: &str) -> Option<ObsDiagnostic> {
    // Pola: "error[CODE]: message" atau "warning[CODE]: message"
    let (severity, rest) = if let Some(r) = line.strip_prefix("error[") {
        (ObsSeverity::Error, r)
    } else if let Some(r) = line.strip_prefix("warning[") {
        (ObsSeverity::Warning, r)
    } else if let Some(r) = line.strip_prefix("note[") {
        (ObsSeverity::Note, r)
    } else if line.contains("panicked at") || line.contains("RUST_BACKTRACE") {
        return Some(ObsDiagnostic {
            severity: ObsSeverity::Fatal,
            code: "PANIC".to_string(),
            message: line.to_string(),
            location: None,
        });
    } else {
        return None;
    };

    // Extract code — isi antara [ dan ]
    let close = rest.find(']')?;
    let code = rest[..close].to_string();
    let after_bracket = rest[close + 1..].trim_start_matches(':').trim();

    // Cari lokasi "file:line:col" di akhir pesan (pola mivon)
    let (message, location) = extract_location(after_bracket);

    Some(ObsDiagnostic {
        severity,
        code,
        message,
        location,
    })
}

/// Ekstrak lokasi `file:line:col` dari akhir pesan diagnostik mivon.
fn extract_location(msg: &str) -> (String, Option<ObsLocation>) {
    // Pola: "... → file.sv:12:7" atau "... at file.sv:12:7"
    // Cari pola digit:digit di akhir string
    let mut parts = msg.rsplitn(3, ':');
    if let (Some(col_s), Some(line_s), Some(rest)) = (parts.next(), parts.next(), parts.next()) {
        if let (Ok(col), Ok(line)) = (col_s.trim().parse::<u32>(), line_s.trim().parse::<u32>()) {
            // rest mungkin diakhiri nama file setelah "→" atau spasi
            let file_start = rest.rfind(|c: char| c == ' ' || c == '→')
                .map(|i| i + 1)
                .unwrap_or(0);
            let file = rest[file_start..].trim().to_string();
            if !file.is_empty() && (file.ends_with(".sv") || file.ends_with(".svh") || file.ends_with(".v")) {
                let message = rest[..file_start].trim_end_matches([' ', '→', ':']).to_string();
                return (
                    if message.is_empty() { msg.to_string() } else { message },
                    Some(ObsLocation { file, line, col }),
                );
            }
        }
    }
    (msg.to_string(), None)
}

/// Parse nilai sinyal dari stdout mivon.
/// Format: baris "SIGNAL_NAME=VALUE" atau dari $display.
fn parse_signal_values(stdout: &str) -> Vec<ValueObservation> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        // Format sederhana: "name=value" — diemit $display di testcase fuzz
        if let Some(eq) = t.find('=') {
            let name = t[..eq].trim();
            let val_s = t[eq + 1..].trim();
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                if let Some(val) = parse_lrm_value(val_s) {
                    out.push(ValueObservation {
                        signal: name.to_string(),
                        time: 0,
                        value: val,
                    });
                }
            }
        }
    }
    out
}

fn parse_lrm_value(s: &str) -> Option<LrmValue> {
    // Hex: 0x... atau 'hXX
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if let Ok(v) = u64::from_str_radix(hex, 16) {
            return Some(LrmValue::from_u64(v, (hex.len() as u32) * 4));
        }
    }
    // Decimal integer
    if let Ok(v) = s.parse::<u64>() {
        return Some(LrmValue::from_u64(v, 32));
    }
    // Real
    if let Ok(f) = s.parse::<f64>() {
        return Some(LrmValue::Real(f));
    }
    None
}

/// Konversi `LogicVec` mivon-ir → `LrmValue`.
#[cfg(feature = "dev")]
fn logicvec_to_lrm(lv: &mivon_ir::LogicVec) -> LrmValue {
    use crate::lrm_model::LogicBit;
    use mivon_ir::LogicVal;
    let bits: Vec<LogicBit> = lv.bits.iter().map(|b| match b {
        LogicVal::Zero => LogicBit::Zero,
        LogicVal::One => LogicBit::One,
        LogicVal::X => LogicBit::X,
        LogicVal::Z => LogicBit::Z,
    }).collect();
    let width = bits.len() as u32;
    LrmValue::FourState { bits, width }
}
