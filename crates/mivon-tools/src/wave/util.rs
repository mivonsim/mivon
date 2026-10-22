//! Helper dan util bersama di seluruh mwave: error builder, nama sinyal
//! penuh, timeline, sample pada waktu T, wildcard, dan util bits.

use std::collections::HashMap;

use mivon_core::error::SimError;

use super::data::{VcdData, VcdSignal};

pub(crate) fn err(msg: impl Into<String>) -> SimError {
    SimError::with_diag(mivon_core::diagnostics::DiagCode::WaveformError, msg)
}

/// Nama penuhi sinyal: `scope1.scope2.name`.
pub(crate) fn full_name(s: &VcdSignal) -> String {
    let mut name = s.scope.join(".");
    if !name.is_empty() {
        name.push('.');
    }
    name.push_str(&s.name);
    name
}

/// Label nama file input (ringkas, untuk pesan).
pub(crate) fn input_name(data: &VcdData) -> String {
    data.signals
        .first()
        .map(|_| "vcd".to_string())
        .unwrap_or_default()
}

/// Bangun peta sinyal: id → (name, list perubahan (time, value)).
pub(crate) fn signal_timelines(data: &VcdData) -> HashMap<String, Vec<(u64, String)>> {
    let mut map: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    for sig in &data.signals {
        map.entry(sig.id.clone()).or_default();
    }
    for (t, id, val) in &data.changes {
        if let Some(v) = map.get_mut(id) {
            v.push((*t, val.clone()));
        }
    }
    for v in map.values_mut() {
        v.sort_by_key(|(t, _)| *t);
    }
    map
}

/// Wildcard match sederhana: `*` = any run (termasuk kosong), `?` = satu
/// karakter, sisanya literal. Case-sensitive (nama signal SV case-sensitive).
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // DP: dp[i][j] = pattern[..i] cocok text[..j].
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=p.len() {
        for j in 1..=t.len() {
            dp[i][j] = match p[i - 1] {
                '*' => dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == t[j - 1],
            };
        }
    }
    dp[p.len()][t.len()]
}

/// Format nilai untuk tampilan: bits mentah + desimal bila biner murni.
pub(crate) fn format_bits(v: &str) -> String {
    if !v.is_empty() && v.chars().all(|c| c == '0' || c == '1') {
        format!("{} ({})", v, bits_val(v))
    } else {
        v.to_string()
    }
}

/// Cari sinyal by nama komponen terakhir (case-sensitive dulu, lalu case-insensitive).
pub(crate) fn find_signal<'a>(data: &'a VcdData, name: &str) -> Option<&'a VcdSignal> {
    let last = |s: &VcdSignal| full_name(s).rsplit('.').next().unwrap_or("").to_string();
    data.signals.iter().find(|s| last(s) == name).or_else(|| {
        data.signals
            .iter()
            .find(|s| last(s).eq_ignore_ascii_case(name))
    })
}

/// Timeline (time, nilai) untuk satu sinyal, sorted by time.
pub(crate) fn timeline_of(data: &VcdData, sig: &VcdSignal) -> Vec<(u64, String)> {
    let mut tl: Vec<(u64, String)> = data
        .changes
        .iter()
        .filter(|(_, id, _)| id == &sig.id)
        .map(|(t, _, v)| (*t, v.clone()))
        .collect();
    tl.sort_by_key(|(t, _)| *t);
    tl
}

/// Nilai sinyal pada waktu t (perubahan terakhir ≤ t; "x" bila belum ada).
pub(crate) fn sample_at(tl: &[(u64, String)], t: u64) -> String {
    let mut v = String::from("x");
    for (tt, val) in tl {
        if *tt <= t {
            v = val.clone();
        } else {
            break;
        }
    }
    v
}

/// Konversi string bit ("0101") → u64 (bit x/z dianggap 0).
pub(crate) fn bits_val(bits: &str) -> u64 {
    let mut v = 0u64;
    for c in bits.chars() {
        v = (v << 1) | u64::from(c == '1');
    }
    v
}

/// Union sorted-unik dari waktu perubahan beberapa timeline.
pub(crate) fn union_times(tls: &[&[(u64, String)]]) -> Vec<u64> {
    let mut ts: Vec<u64> = tls
        .iter()
        .flat_map(|tl| tl.iter().map(|(t, _)| *t))
        .collect();
    ts.sort_unstable();
    ts.dedup();
    ts
}

#[allow(dead_code)]
pub(crate) fn _pathbuf(s: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(s)
}
