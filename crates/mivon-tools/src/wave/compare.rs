//! Subkomando `compare` — bandingkan dua VCD (WAV-09).

use std::collections::HashMap;

use mivon_core::error::SimError;

use super::data::{parse_vcd, VcdData, VcdSignal};
use super::util::full_name;

/// Hasil perbandingan dua VCD (dipisah dari pencetakan agar bisa diuji).
pub(crate) struct CompareResult {
    pub(crate) common: usize,
    pub(crate) only_a: Vec<String>,
    pub(crate) only_b: Vec<String>,
    pub(crate) mismatches: Vec<(String, u64, String, String, usize)>,
    pub(crate) max_time_a: u64,
    pub(crate) max_time_b: u64,
}

pub(crate) fn compare(a: &str, b: &str) -> Result<(), SimError> {
    let da = parse_vcd(a)?;
    let db = parse_vcd(b)?;
    let r = compare_data(&da, &db);

    // ── Laporan ──
    println!("  compare {} vs {}", a, b);
    println!("    timescale: {} vs {}", da.timescale, db.timescale);
    println!(
        "    signals:   {} vs {} ({} sinyal umum, {} hanya di A, {} hanya di B)",
        da.signals.len(),
        db.signals.len(),
        r.common,
        r.only_a.len(),
        r.only_b.len()
    );
    println!("    max_time:  {} vs {}", r.max_time_a, r.max_time_b);
    println!("    total mismatch: {}", r.mismatches.len());
    for (name, t, fa, fb, cnt) in &r.mismatches {
        let fa_s = if fa.len() == 1 {
            fa.clone()
        } else {
            format!("b{} ", fa)
        };
        let fb_s = if fb.len() == 1 {
            fb.clone()
        } else {
            format!("b{} ", fb)
        };
        println!(
            "      {}  first t={}  a={} b={}  ({} sample berbeda)",
            name,
            t,
            fa_s.trim_end(),
            fb_s.trim_end(),
            cnt
        );
    }
    for n in &r.only_a {
        println!("      [hanya di A] {}", n);
    }
    for n in &r.only_b {
        println!("      [hanya di B] {}", n);
    }
    Ok(())
}

/// Logika perbandingan dua VCD: timeline per signal, sample per unit waktu.
pub(crate) fn compare_data(da: &VcdData, db: &VcdData) -> CompareResult {
    // Indeks signal per nama penuh (scope.name).
    let idx_a: HashMap<String, &VcdSignal> = da.signals.iter().map(|s| (full_name(s), s)).collect();
    let idx_b: HashMap<String, &VcdSignal> = db.signals.iter().map(|s| (full_name(s), s)).collect();

    // Timeline per signal: (time, value) terurut dari changes.
    let mut tl_a: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    for (t, id, val) in &da.changes {
        if let Some(sig) = da.signals.iter().find(|s| &s.id == id) {
            tl_a.entry(full_name(sig))
                .or_default()
                .push((*t, val.clone()));
        }
    }
    let mut tl_b: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    for (t, id, val) in &db.changes {
        if let Some(sig) = db.signals.iter().find(|s| &s.id == id) {
            tl_b.entry(full_name(sig))
                .or_default()
                .push((*t, val.clone()));
        }
    }

    // Nama gabungan (union) — urutan stabil: A dulu, lalu B yang tidak di A.
    let mut names: Vec<String> = idx_a.keys().cloned().collect();
    names.sort();
    for n in idx_b.keys() {
        if !idx_a.contains_key(n) {
            names.push(n.clone());
        }
    }

    let end = da.max_time.max(db.max_time);

    let mut only_a: Vec<String> = Vec::new();
    let mut only_b: Vec<String> = Vec::new();
    let mut mismatches: Vec<(String, u64, String, String, usize)> = Vec::new();

    for name in &names {
        let in_a = idx_a.contains_key(name);
        let in_b = idx_b.contains_key(name);
        match (in_a, in_b) {
            (true, false) => only_a.push(name.clone()),
            (false, true) => only_b.push(name.clone()),
            (false, false) => {} // union names — tidak mungkin
            (true, true) => {
                let ta = tl_a.get(name).cloned().unwrap_or_default();
                let tb = tl_b.get(name).cloned().unwrap_or_default();
                // Nilai pada tiap saat: value yang berlaku (last change ≤ t).
                let val_at = |tl: &[(u64, String)], t: u64| -> Option<String> {
                    tl.iter()
                        .rev()
                        .find(|(tt, _)| *tt <= t)
                        .map(|(_, v)| v.clone())
                };
                let mut first: Option<(u64, String, String)> = None;
                let mut count = 0usize;
                let mut t = 0u64;
                while t <= end {
                    let va = val_at(&ta, t);
                    let vb = val_at(&tb, t);
                    // Kedua punya nilai & berbeda → mismatch.
                    if let (Some(a), Some(b)) = (va, vb) {
                        if a != b {
                            count += 1;
                            if first.is_none() {
                                first = Some((t, a, b));
                            }
                        }
                    }
                    t += 1;
                }
                if count > 0 {
                    let (ft, fa, fb) = first.unwrap();
                    mismatches.push((name.clone(), ft, fa, fb, count));
                }
            }
        }
    }

    CompareResult {
        common: names.len() - only_a.len() - only_b.len(),
        only_a,
        only_b,
        mismatches,
        max_time_a: da.max_time,
        max_time_b: db.max_time,
    }
}
