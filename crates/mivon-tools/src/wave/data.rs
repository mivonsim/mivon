//! Data model VCD + parser/writer ([`parse_vcd`], [`write_vcd`]).
//! Dipakai semua subkomando mwave.

use std::collections::HashMap;

use mivon_core::error::SimError;

use super::util::err;

/// Satu sinyal VCD.
#[derive(Debug, Clone, PartialEq)]
pub struct VcdSignal {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) width: usize,
    pub(crate) scope: Vec<String>,
}

/// Hasil parse satu file VCD.
#[derive(Debug, Clone)]
pub struct VcdData {
    pub(crate) timescale: String,
    pub(crate) signals: Vec<VcdSignal>,
    /// (time, signal id, nilai mentah)
    pub(crate) changes: Vec<(u64, String, String)>,
    pub(crate) max_time: u64,
}

/// ── Parser VCD ──
pub(crate) fn parse_vcd(path: &str) -> Result<VcdData, SimError> {
    let src = std::fs::read_to_string(path).map_err(|e| err(format!("{}: {}", path, e)))?;
    let lines: Vec<&str> = src.lines().map(|l| l.trim()).collect();

    let mut signals: Vec<VcdSignal> = Vec::new();
    let mut changes: Vec<(u64, String, String)> = Vec::new();
    let mut timescale = String::new();
    let mut scope: Vec<String> = Vec::new();
    let mut max_time = 0u64;
    let mut in_header = true;
    let mut cur_time = 0u64;

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if in_header {
            if let Some(rest) = line.strip_prefix("$scope") {
                // scope module top $end
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 2 {
                    scope.push(parts[1].to_string());
                }
            } else if let Some(_rest) = line.strip_prefix("$upscope") {
                scope.pop();
            } else if let Some(rest) = line.strip_prefix("$var") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                // $var wire 1 ! clk $end
                if parts.len() >= 4 {
                    let width = parts[1].parse::<usize>().unwrap_or(1);
                    let id = parts[2].to_string();
                    let name = parts[3].to_string();
                    signals.push(VcdSignal {
                        id,
                        name,
                        width,
                        scope: scope.clone(),
                    });
                }
            } else if let Some(rest) = line.strip_prefix("$timescale") {
                timescale = rest.split_whitespace().next().unwrap_or("1ns").to_string();
            } else if line.starts_with("$enddefinitions") {
                in_header = false;
            }
            i += 1;
            continue;
        }

        // Body: #time atau value line
        if let Some(t) = line.strip_prefix('#') {
            cur_time = t.parse::<u64>().unwrap_or(0);
            if cur_time > max_time {
                max_time = cur_time;
            }
            i += 1;
            continue;
        }

        if let Some(rest) = line.strip_prefix('b') {
            // b1010 <id>
            let mut parts = rest.splitn(2, char::is_whitespace);
            let val = parts.next().unwrap_or("").to_string();
            let id = parts.next().unwrap_or("").trim().to_string();
            if !id.is_empty() {
                changes.push((cur_time, id, val));
            }
        } else if let Some(rest) = line.strip_prefix('B') {
            // B<hex> <id>
            let mut parts = rest.splitn(2, char::is_whitespace);
            let val = parts.next().unwrap_or("").to_string();
            let id = parts.next().unwrap_or("").trim().to_string();
            if !id.is_empty() {
                // Konversi hex → biner agar konsisten
                let bin = hex_to_bin(&val);
                changes.push((cur_time, id, bin));
            }
        } else if line.len() >= 2 {
            // scalar: 0<id>, 1<id>, x<id>, z<id>
            let first = line.as_bytes()[0] as char;
            if matches!(first, '0' | '1' | 'x' | 'z') {
                let id = line[1..].trim().to_string();
                if !id.is_empty() {
                    changes.push((cur_time, id, first.to_string()));
                }
            }
        }
        i += 1;
    }

    if signals.is_empty() {
        return Err(err(format!(
            "{}: bukan file VCD valid (tidak ada $var)",
            path
        )));
    }

    Ok(VcdData {
        timescale,
        signals,
        changes,
        max_time,
    })
}

pub(crate) fn hex_to_bin(hex: &str) -> String {
    let mut out = String::new();
    for c in hex.chars() {
        let v = match c.to_digit(16) {
            Some(v) => format!("{:04b}", v),
            None => {
                // x/z → biarkan sebagai 'x'/'z'
                if c == 'x' || c == 'X' {
                    "xxxx".to_string()
                } else if c == 'z' || c == 'Z' {
                    "zzzz".to_string()
                } else {
                    String::new()
                }
            }
        };
        out.push_str(&v);
    }
    out
}

/// Tulis VCD (header + changes) dengan fungsi remap id.
pub(crate) fn write_vcd(
    out_path: &str,
    timescale: &str,
    signals: &[VcdSignal],
    changes: &[(u64, String, String)],
    id_map: &HashMap<String, String>,
) -> Result<(), SimError> {
    let mut out = String::new();
    out.push_str("$date\n  mwave\n$end\n");
    out.push_str("$version\n  mivon mwave\n$end\n");
    if !timescale.is_empty() {
        out.push_str(&format!("$timescale {} $end\n", timescale));
    }
    // Kelompokkan signal per scope (urutkan stabil sesuai input)
    let mut prev_scope: Option<Vec<String>> = None;
    for sig in signals {
        if prev_scope.as_ref() != Some(&sig.scope) {
            // Tutup scope sebelumnya
            if prev_scope.is_some() {
                out.push_str("$upscope $end\n");
            }
            for s in &sig.scope {
                out.push_str(&format!("$scope module {} $end\n", s));
            }
            prev_scope = Some(sig.scope.clone());
        }
        let id = id_map
            .get(&sig.id)
            .cloned()
            .unwrap_or_else(|| sig.id.clone());
        let range = if sig.width > 1 {
            format!(" [{}:0]", sig.width - 1)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "$var wire {} {} {}{} $end\n",
            sig.width, id, sig.name, range
        ));
    }
    if prev_scope.is_some() {
        out.push_str("$upscope $end\n");
    }
    out.push_str("$enddefinitions $end\n");

    // Body: changes diurutkan per waktu
    let mut cur_t = None;
    for (t, id, val) in changes {
        let mapped = id_map.get(id).cloned().unwrap_or_else(|| id.clone());
        if cur_t != Some(*t) {
            out.push_str(&format!("#{}\n", t));
            cur_t = Some(*t);
        }
        if val.len() == 1 && matches!(val.as_bytes()[0] as char, '0' | '1' | 'x' | 'z') {
            out.push_str(&format!("{}{}\n", val, mapped));
        } else {
            out.push_str(&format!("b{} {}\n", val, mapped));
        }
    }

    std::fs::write(out_path, out).map_err(|e| err(format!("{}: {}", out_path, e)))?;
    Ok(())
}
