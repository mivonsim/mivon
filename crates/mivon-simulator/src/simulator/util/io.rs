//! Pembaca berkas memori untuk $readmemh/$readmemb (+ varian teks).
//!
//! Dipisah dari `util.rs` (grab-bag) — 1 file = 1 tanggung jawab:
//! parsing format LRM $readmemh/$readmemb: nilai per baris, `@addr`
//! heksadesimal, komentar `//` dan `#`.

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_ir::LogicVec;

pub fn read_hex_file(
    filename: &str,
    elem_width: usize,
    array_depth: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Vec<(usize, LogicVec)>, SimError> {
    let content = std::fs::read_to_string(filename).map_err(|e| {
        SimError::with_diag(
            DiagCode::IoError,
            format!("cannot read {}: {}", filename, e),
        )
    })?;
    read_hex_text(&content, elem_width, array_depth, start, end)
}

/// Parse isi teks (bukan file) format $readmemh/$sreadmemh.
pub fn read_hex_text(
    content: &str,
    elem_width: usize,
    array_depth: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Vec<(usize, LogicVec)>, SimError> {
    let start_addr = start.unwrap_or(0);
    let end_addr = end.unwrap_or(array_depth.saturating_sub(1));
    if end_addr < start_addr {
        return Ok(Vec::new());
    }
    let mut data = Vec::new();
    let mut addr = start_addr;
    'outer: for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        // LRM: beberapa nilai per baris dipisah whitespace; @addr set alamat.
        for tok in line.split_whitespace() {
            if tok.starts_with("//") || tok.starts_with('#') {
                break;
            }
            if let Some(rest) = tok.strip_prefix('@') {
                addr = usize::from_str_radix(rest, 16).map_err(|e| {
                    SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("bad address '{}': {}", rest, e),
                    )
                })?;
                continue;
            }
            let val = i64::from_str_radix(tok, 16).map_err(|e| {
                SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    format!("bad hex value '{}': {}", tok, e),
                )
            })?;
            if addr >= start_addr && addr <= end_addr {
                data.push((addr, LogicVec::from_u64(val as u64, elem_width)));
            }
            addr += 1;
            if addr > end_addr {
                break 'outer;
            }
        }
    }
    Ok(data)
}

pub fn read_bin_file(
    filename: &str,
    elem_width: usize,
    array_depth: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Vec<(usize, LogicVec)>, SimError> {
    let content = std::fs::read_to_string(filename).map_err(|e| {
        SimError::with_diag(
            DiagCode::IoError,
            format!("cannot read {}: {}", filename, e),
        )
    })?;
    read_bin_text(&content, elem_width, array_depth, start, end)
}

/// Parse isi teks (bukan file) format $readmemb/$sreadmemb.
pub fn read_bin_text(
    content: &str,
    elem_width: usize,
    array_depth: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Vec<(usize, LogicVec)>, SimError> {
    let start_addr = start.unwrap_or(0);
    let end_addr = end.unwrap_or(array_depth.saturating_sub(1));
    if end_addr < start_addr {
        return Ok(Vec::new());
    }
    let mut data = Vec::new();
    let mut addr = start_addr;
    'outer: for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        // LRM: beberapa nilai per baris dipisah whitespace; @addr set alamat
        // (alamat selalu heksadesimal, termasuk untuk readmemb).
        for tok in line.split_whitespace() {
            if tok.starts_with("//") || tok.starts_with('#') {
                break;
            }
            if let Some(rest) = tok.strip_prefix('@') {
                addr = usize::from_str_radix(rest, 16).map_err(|e| {
                    SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("bad address '{}': {}", rest, e),
                    )
                })?;
                continue;
            }
            let val = i64::from_str_radix(tok, 2).map_err(|e| {
                SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    format!("bad binary value '{}': {}", tok, e),
                )
            })?;
            if addr >= start_addr && addr <= end_addr {
                data.push((addr, LogicVec::from_u64(val as u64, elem_width)));
            }
            addr += 1;
            if addr > end_addr {
                break 'outer;
            }
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi pemisahan modul: `@addr` heksadesimal + filter rentang
    /// alamat harus tetap sama seperti versi util.rs.
    #[test]
    fn hex_text_honours_addr_directive_and_range() {
        let data = read_hex_text("@10 AB cd\n// komentar\nef", 8, 64, Some(0x10), Some(0x11))
            .expect("parse");
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].0, 0x10);
        assert_eq!(data[1].0, 0x11);
    }

    /// readmemb: nilai biner, alamat tetap heksadesimal.
    #[test]
    fn bin_text_parses_binary_values() {
        let data = read_bin_text("1010 0011", 8, 4, None, None).expect("parse");
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].1, LogicVec::from_u64(0b1010, 8));
        assert_eq!(data[1].1, LogicVec::from_u64(0b0011, 8));
    }
}
