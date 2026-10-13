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

/// Method string yang MUTATING receiver (LRM 1800 §6.16.1): `itoa`/`hextoa`/
/// `bintoa`/`octtoa`/`putc`. Evaluator melakukan write-back hasil ke receiver
/// (signal string) setelah evaluasi.
pub(crate) fn is_mutating_string_method(method: &str) -> bool {
    matches!(method, "itoa" | "hextoa" | "bintoa" | "octtoa" | "putc")
}

/// Bangun `LogicVec` dari string TANPA null-terminator — konsisten dgn
/// method pengembalian string lainnya (`tolower`/`toupper`/`substr`).
fn string_logicvec_no_term(s: &str) -> LogicVec {
    let mut bits = Vec::with_capacity(s.len() * 8);
    for byte in s.bytes() {
        for b in 0..8 {
            bits.push(if (byte >> b) & 1 == 1 {
                LogicVal::One
            } else {
                LogicVal::Zero
            });
        }
    }
    LogicVec {
        width: bits.len(),
        bits,
    }
}

/// Evaluasi method string untuk tipe data string SystemVerilog.
/// Mendukung: .len(), .substr(), .atoi(), .hextoi(), .bintoi(), .octtoi(),
/// .tolower(), .toupper(), .compare(), .icompare(), .getc(), .atohex(),
/// .atobin(), .atooct(), .atoreal(), .itoa(), .hextoa(), .bintoa(), .octtoa(),
/// .putc()
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
        // LRM 1800 §6.16.1 — method MUTATING: mengubah receiver string dan
        // mengembalikan string hasil. Write-back receiver ditangani di evaluator
        // (`IrExpr::Signal` is_string) via `is_mutating_string_method`.
        // `itoa(i)` → representasi desimal (signed) dari integer `i`.
        "itoa" => {
            if args.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    "itoa expects 1 argument".to_string(),
                ));
            }
            let i = args[0].to_i64();
            Ok(string_logicvec_no_term(&i.to_string()))
        }
        // `hextoa(i)` / `bintoa(i)` / `octtoa(i)` → representasi heksa/biner/
        // oktal (unsigned) dari integer `i` (tanpa prefix `0x`).
        "hextoa" => {
            if args.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    "hextoa expects 1 argument".to_string(),
                ));
            }
            Ok(string_logicvec_no_term(&format!("{:x}", args[0].to_u64())))
        }
        "bintoa" => {
            if args.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    "bintoa expects 1 argument".to_string(),
                ));
            }
            Ok(string_logicvec_no_term(&format!("{:b}", args[0].to_u64())))
        }
        "octtoa" => {
            if args.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    "octtoa expects 1 argument".to_string(),
                ));
            }
            Ok(string_logicvec_no_term(&format!("{:o}", args[0].to_u64())))
        }
        // `putc(i, c)` → ganti byte ke-i dgn `c` (ASCII); i >= len → string
        // dipanjangkan dgn NUL sampai i (LRM §6.16.1); i < 0 → no-op.
        "putc" => {
            if args.len() < 2 {
                return Err(SimError::with_diag(
                    DiagCode::DpiError,
                    "putc expects 2 arguments".to_string(),
                ));
            }
            // Indeks pakai to_i64() (bukan to_u64) agar literal negatif `-3`
            // terdeteksi sebagai negatif; to_u64() akan mengembalikan
            // two's-complement (0xFFFFFFFD) sehingga `i < 0` gagal dan
            // `bytes.resize(idx+1,..)` mencoba alokasi ~4GB → hang/OOM.
            let i = args[0].to_i64();
            let c = (args[1].to_u64() & 0xFF) as u8;
            if i < 0 {
                return Ok(string_logicvec_no_term(s));
            }
            let mut bytes = s.as_bytes().to_vec();
            let idx = i as usize;
            if idx >= bytes.len() {
                bytes.resize(idx + 1, 0);
            }
            bytes[idx] = c;
            let new_s = String::from_utf8_lossy(&bytes).into_owned();
            Ok(string_logicvec_no_term(&new_s))
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
