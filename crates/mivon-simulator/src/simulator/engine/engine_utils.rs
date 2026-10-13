//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini berisi fungsi-fungsi helper standalone yang digunakan oleh
//! SimulationEngine. Fungsi-fungsi ini dipisahkan dari module.rs untuk memisahkan
//! tanggung jawab (Single Responsibility Principle).
//!
//! module.rs hanya berisi:
//!   - Definisi struct SimulationEngine
//!   - Definisi struct SequenceAttempt
//!   - Konstanta MAX_LOOP_ITER
//!   - Module declarations (pub mod)
//!
//! engine_utils.rs berisi:
//!   - evaluate_string_method() — evaluasi method string seperti .len(), .substr(), dll.
//!   - sym_char_matches() — pencocokan karakter simbol untuk UDP
//!   - edge_matches_abbrev() — pencocokan edge transition untuk UDP
//!
//! ──────────────────────────────────────────────────────────────────────────────

use crate::simulator::util::logicvec_to_string;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_ir::{LogicVal, LogicVec};

/// Evaluasi method string untuk tipe data string SystemVerilog.
/// Mendukung: .len(), .substr(), .atoi(), .hextoi(), .bintoi(), .octtoi(),
/// .tolower(), .toupper(), .compare(), .icompare()
pub(crate) fn evaluate_string_method(
    s: &str,
    method: &str,
    args: &[LogicVec],
) -> Result<LogicVec, SimError> {
    match method {
        "len" => Ok(LogicVec::from_u64(s.len() as u64, 32)),
        "substr" => {
            if args.len() != 2 {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    format!("substr expects 2 arguments, got {}", args.len()),
                ));
            }
            let i = args[0].to_u64() as usize;
            let j = args[1].to_u64() as usize;
            if i > j || j >= s.len() {
                return Err(SimError::with_diag(
                    DiagCode::MemoryOutOfBounds,
                    format!(
                        "substr({}, {}) out of range for string of len {}",
                        i,
                        j,
                        s.len()
                    ),
                ));
            }
            let sub = &s[i..=j];
            let mut bits = Vec::with_capacity(sub.len() * 8);
            for c in sub.chars() {
                let byte = c as u8;
                for b in 0..8 {
                    bits.push(if (byte >> b) & 1 == 1 {
                        LogicVal::One
                    } else {
                        LogicVal::Zero
                    });
                }
            }
            Ok(LogicVec {
                width: bits.len(),
                bits,
            })
        }
        "atoi" => {
            let val: i64 = s.trim().parse().unwrap_or(0);
            Ok(LogicVec::from_u64(val as u64, 32))
        }
        "hextoi" => {
            let trimmed = s.trim().trim_start_matches("0x").trim_start_matches("0X");
            let val = i64::from_str_radix(trimmed, 16).unwrap_or(0);
            Ok(LogicVec::from_u64(val as u64, 32))
        }
        "bintoi" => {
            let trimmed = s.trim();
            let val = i64::from_str_radix(trimmed, 2).unwrap_or(0);
            Ok(LogicVec::from_u64(val as u64, 32))
        }
        "octtoi" => {
            let trimmed = s.trim();
            let val = i64::from_str_radix(trimmed, 8).unwrap_or(0);
            Ok(LogicVec::from_u64(val as u64, 32))
        }
        "tolower" => {
            let lower = s.to_lowercase();
            let mut bits = Vec::with_capacity(lower.len() * 8);
            for c in lower.chars() {
                let byte = c as u8;
                for b in 0..8 {
                    bits.push(if (byte >> b) & 1 == 1 {
                        LogicVal::One
                    } else {
                        LogicVal::Zero
                    });
                }
            }
            Ok(LogicVec {
                width: bits.len(),
                bits,
            })
        }
        "toupper" => {
            let upper = s.to_uppercase();
            let mut bits = Vec::with_capacity(upper.len() * 8);
            for c in upper.chars() {
                let byte = c as u8;
                for b in 0..8 {
                    bits.push(if (byte >> b) & 1 == 1 {
                        LogicVal::One
                    } else {
                        LogicVal::Zero
                    });
                }
            }
            Ok(LogicVec {
                width: bits.len(),
                bits,
            })
        }
        "compare" | "icompare" => {
            if args.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    format!("{} expects 1 argument", method),
                ));
            }
            let other_val = &args[0];
            let other = logicvec_to_string(other_val);
            let ordering = if method == "icompare" {
                s.to_lowercase().cmp(&other.to_lowercase())
            } else {
                s.cmp(&other)
            };
            let result = match ordering {
                std::cmp::Ordering::Less => -1i64,
                std::cmp::Ordering::Equal => 0i64,
                std::cmp::Ordering::Greater => 1i64,
            };
            Ok(LogicVec::from_u64(result as u64, 32))
        }
        // LRM 1800 §6.16.1 — `getc(i)` mengembalikan nilai ASCII (8-bit) karakter
        // pada indeks `i`; indeks di luar batas → 0 (sesuai LRM).
        "getc" => {
            if args.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    "getc expects 1 argument".to_string(),
                ));
            }
            let i = args[0].to_u64() as usize;
            let byte = s.as_bytes().get(i).copied().unwrap_or(0);
            Ok(LogicVec::from_u64(byte as u64, 8))
        }
        // Nama LRM-STANDAR untuk konversi string→integer (pengganti alias non-
        // standar `hextoi`/`bintoi`/`octtoi` yang tetap dipertahankan).
        "atohex" => {
            let trimmed = s.trim().trim_start_matches("0x").trim_start_matches("0X");
            let val = i64::from_str_radix(trimmed, 16).unwrap_or(0);
            Ok(LogicVec::from_u64(val as u64, 32))
        }
        "atobin" => {
            let trimmed = s.trim();
            let val = i64::from_str_radix(trimmed, 2).unwrap_or(0);
            Ok(LogicVec::from_u64(val as u64, 32))
        }
        "atooct" => {
            let trimmed = s.trim();
            let val = i64::from_str_radix(trimmed, 8).unwrap_or(0);
            Ok(LogicVec::from_u64(val as u64, 32))
        }
        // LRM 1800 §6.16.1 — `atoreal()` → nilai real (bit-pattern f64 64-bit,
        // konsisten dengan penanganan real di simulator ini, mis. `$itor`).
        "atoreal" => {
            let val: f64 = s.trim().parse().unwrap_or(0.0);
            Ok(LogicVec::from_u64(f64::to_bits(val), 64))
        }
        _ => Err(SimError::with_diag(
            DiagCode::NotImplemented,
            format!("unknown string method: {}", method),
        )),
    }
}

/// Cek apakah karakter simbol UDP cocok dengan nilai LogicVal.
/// Digunakan untuk tabel UDP (User-Defined Primitives).
pub(crate) fn sym_char_matches(c: char, val: LogicVal) -> bool {
    match c {
        '0' => val == LogicVal::Zero,
        '1' => val == LogicVal::One,
        'x' | 'X' => val == LogicVal::X,
        '?' => true,
        'b' | 'B' => val == LogicVal::Zero || val == LogicVal::One,
        _ => false,
    }
}

/// Cek apakah edge transition UDP cocok dengan singkatan edge.
/// Digunakan untuk edge detection di tabel UDP.
pub(crate) fn edge_matches_abbrev(edge: &str, prev: LogicVal, curr: LogicVal) -> bool {
    match edge {
        "r" | "R" => prev == LogicVal::Zero && curr == LogicVal::One,
        "f" | "F" => prev == LogicVal::One && curr == LogicVal::Zero,
        "p" | "P" => {
            (prev == LogicVal::Zero || prev == LogicVal::X || prev == LogicVal::Z)
                && curr == LogicVal::One
        }
        "n" | "N" => {
            (prev == LogicVal::One || prev == LogicVal::X || prev == LogicVal::Z)
                && curr == LogicVal::Zero
        }
        "*" => prev != curr,
        _ => false,
    }
}
