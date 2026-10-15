//! Validasi hasil simulasi MIVON-FUZZ — buktikan kebenaran, bukan sekadar
//! "jalan dan selesai".
//!
//! Tiga pilar:
//! 1. **Determinism** — sim yang sama dijalankan 2x, hasil harus identik.
//! 2. **Differential** — 5 jalur engine (default/packed/dag/timing/mir-jit)
//!    dibandingkan berpasangan; perbedaan = bukti engine path disagreement.
//! 3. **X/Z audit** — signal yang TETAP X/Z di akhir sim dicatat; bisa wajar
//!    (undriven) atau bukti state-propagation bug — dipasok ke laporan.
#![allow(clippy::result_large_err, clippy::type_complexity)]

use mivon_api::{
    simulate_signals_with_flags_quiet, simulate_signals_with_trace_quiet, EngineFlags,
};

/// Bukti keberhasilan/keanehan satu simulasi.
#[derive(Debug, Clone)]
pub struct SimEvidence {
    /// Jumlah blok prosedural (initial/always/ff/comb) di design — bukti
    /// stimulus ada atau tidak (0 = design pasif → X wajar diharapkan).
    pub process_count: usize,
    /// Total signal hasil sim.
    pub signal_count: usize,
    /// Signal yang masih X di akhir sim.
    pub x_remain: usize,
    /// Signal yang masih Z di akhir sim.
    pub z_remain: usize,
    /// Total BIT bernilai X di akhir sim (sensus per-bit — partial-X
    /// 31/32 bit tak terlihat di `x_remain` yang hanya hitung all-X).
    pub x_bits: usize,
    /// Total bit seluruh signal (penyebut fraksi X).
    pub bit_total: usize,
    /// Signal yang pernah berubah di tengah sim (aktivitas > 0 = ada kerja).
    pub active_signals: usize,
    /// Determinism: dua run identik hasil sama.
    pub determinism_ok: bool,
    /// Differential: semua jalur engine sepakat.
    pub differential_ok: bool,
    /// Berapa pasangan jalur engine dibandingkan.
    pub paths_compared: usize,
    /// Detail perbedaan differential (nama signal + nilai tiap jalur).
    pub diff_details: Vec<String>,
}

impl Default for SimEvidence {
    fn default() -> Self {
        Self {
            process_count: 0,
            signal_count: 0,
            x_remain: 0,
            z_remain: 0,
            x_bits: 0,
            bit_total: 0,
            active_signals: 0,
            determinism_ok: true,
            differential_ok: true,
            paths_compared: 0,
            diff_details: Vec::new(),
        }
    }
}

impl SimEvidence {
    /// Ringkasan satu baris untuk detail laporan.
    pub fn summary(&self) -> String {
        format!(
            "proc={} sigs={} x={} z={} xb={}/{} active={} det={} diff={} paths={} diffs=[{}]",
            self.process_count,
            self.signal_count,
            self.x_remain,
            self.z_remain,
            self.x_bits,
            self.bit_total,
            self.active_signals,
            self.determinism_ok,
            self.differential_ok,
            self.paths_compared,
            self.diff_details.join(" | "),
        )
    }
}

/// Jalur engine yang dibandingkan (termasuk kombos — area belum tersentuh:
/// kombinasi flag parallel+packed dll, bukan hanya single-flag).
fn engine_paths() -> Vec<(&'static str, EngineFlags)> {
    let base = EngineFlags {
        use_packed_eval: false,
        use_dag_parallel: false,
        use_timing_wheel: false,
        use_mir_jit: false,
    };
    vec![
        ("default", EngineFlags { ..base }),
        (
            "packed",
            EngineFlags {
                use_packed_eval: true,
                ..base
            },
        ),
        (
            "dag-par",
            EngineFlags {
                use_dag_parallel: true,
                ..base
            },
        ),
        (
            "timing",
            EngineFlags {
                use_timing_wheel: true,
                ..base
            },
        ),
        (
            "mir-jit",
            EngineFlags {
                use_mir_jit: true,
                ..base
            },
        ),
        // ── Kombinasi (interaksi flag) ──
        (
            "packed+dag",
            EngineFlags {
                use_packed_eval: true,
                use_dag_parallel: true,
                ..base
            },
        ),
        (
            "packed+timing",
            EngineFlags {
                use_packed_eval: true,
                use_timing_wheel: true,
                ..base
            },
        ),
        (
            "dag+timing",
            EngineFlags {
                use_dag_parallel: true,
                use_timing_wheel: true,
                ..base
            },
        ),
    ]
}

/// Konversi sinyal ke peta nama → nilai (untuk perbandingan lintas jalur).
fn signal_map(sigs: &[(String, mivon_ir::LogicVec)]) -> std::collections::HashMap<String, String> {
    sigs.iter()
        .map(|(n, v)| (n.clone(), format!("{:?}", v)))
        .collect()
}

/// Sensus per-bit: (jumlah bit X, total bit). Partial-X (31/32 bit X)
/// tak terlihat di `all_x()` — celah audit X/stimulus.
fn x_bit_census(v: &mivon_ir::LogicVec) -> (usize, usize) {
    let mut x = 0usize;
    for b in &v.bits {
        if *b == mivon_ir::LogicVal::X {
            x += 1;
        }
    }
    (x, v.width)
}

/// Kumpulkan bukti simulasi untuk satu source.
///
/// Semua error di-handle: kegagalan satu jalur engine tidak menggagalkan
/// seluruh validasi — jalur itu dicatat, jalur lain tetap dibandingkan.
pub fn collect(source: &str, max_time: u64) -> SimEvidence {
    let mut ev = SimEvidence::default();

    // 1. Stimulus: jumlah blok prosedural dari IR (compile saja, bukan sim).
    match std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source)) {
        Ok(Ok(ir)) => {
            let mut n = ir.top.processes.len();
            for m in ir.modules.values() {
                n += m.processes.len();
            }
            ev.process_count = n;
        }
        _ => ev.process_count = 0,
    }

    // 2. Trace tengah sim — aktivitas signal.
    if let Ok(Ok((sigs, trace))) =
        std::panic::catch_unwind(|| simulate_signals_with_trace_quiet(source, max_time, 10))
    {
        ev.signal_count = sigs.len();
        // Trace berisi fingerprint per interval — jumlah unik signal di trace
        // = signal yang terlihat berubah.
        let mut names = std::collections::HashSet::new();
        for line in &trace {
            if let Some(idx) = line.find(':') {
                let n = line[..idx].to_string();
                if !n.is_empty() {
                    names.insert(n);
                }
            }
        }
        ev.active_signals = names.len();

        // X/Z akhir (level signal) + sensus per-bit.
        for (_, v) in &sigs {
            if v.all_x() {
                ev.x_remain += 1;
            } else if v.all_z() {
                ev.z_remain += 1;
            }
            let (xb, tb) = x_bit_census(v);
            ev.x_bits += xb;
            ev.bit_total += tb;
        }
    }

    let paths = engine_paths();
    let mut results: Vec<(&'static str, Option<Vec<(String, mivon_ir::LogicVec)>>)> = Vec::new();
    for (name, flags) in &paths {
        let r =
            std::panic::catch_unwind(|| simulate_signals_with_flags_quiet(source, max_time, flags));
        match r {
            Ok(Ok(sigs)) => results.push((name, Some(sigs))),
            _ => results.push((name, None)),
        }
    }

    // 3. Determinism — jalur default dijalankan dua kali (run kedua segar).
    {
        let run1 = results[0].1.as_ref().cloned();
        let run2 = std::panic::catch_unwind(|| {
            simulate_signals_with_flags_quiet(source, max_time, &engine_paths()[0].1)
        });
        if let (Some(r1), Ok(Ok(r2))) = (run1, run2) {
            ev.determinism_ok = signal_eq(&r1, &r2);
        } else {
            ev.determinism_ok = false;
        }
    }

    // 4. Differential — bandingkan semua pasangan jalur yang berhasil.
    let ok_paths: Vec<(&str, Vec<(String, mivon_ir::LogicVec)>)> = results
        .iter()
        .filter_map(|(n, r)| r.as_ref().map(|s| (*n, s.clone())))
        .collect();
    ev.paths_compared = ok_paths.len();
    if ok_paths.len() > 1 {
        for i in 0..ok_paths.len() {
            for j in (i + 1)..ok_paths.len() {
                let (ni, si) = &ok_paths[i];
                let (nj, sj) = &ok_paths[j];
                let ma = signal_map(si);
                let mb = signal_map(sj);
                let mut diffs: Vec<String> = Vec::new();
                for (name, va) in &ma {
                    match mb.get(name) {
                        Some(vb) if vb == va => {}
                        Some(vb) => diffs.push(format!("{name}: {ni}={va} {nj}={vb}")),
                        None => diffs.push(format!("{name}: {ni}={va} {nj}=<missing>")),
                    }
                }
                for (name, vb) in &mb {
                    if !ma.contains_key(name) {
                        diffs.push(format!("{name}: {ni}=<missing> {nj}={vb}"));
                    }
                }
                if !diffs.is_empty() {
                    ev.differential_ok = false;
                    // Batasi detail agar laporan tidak meledak.
                    for d in diffs.iter().take(6) {
                        ev.diff_details.push(d.clone());
                    }
                }
            }
        }
    }

    ev
}

/// Apakah dua hasil signal identik (nama + nilai + urutan).
fn signal_eq(a: &[(String, mivon_ir::LogicVec)], b: &[(String, mivon_ir::LogicVec)]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let ma = signal_map(a);
    let mb = signal_map(b);
    ma == mb
}

/// Audit ringan untuk verdict akhir (tanpa O4/O5 duplikat): stimulus ada?
/// (jumlah blok prosedural) + X/Z census hasil sim + aktivitas signal.
///
/// Dipakai oracle setelah determinism+differential OK: membedakan
/// "design pasif → X wajar" vs "stimulus ada tapi signal X → suspicious".
pub fn evidence_only(source: &str, max_time: u64) -> SimEvidence {
    let mut ev = SimEvidence::default();

    match std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source)) {
        Ok(Ok(ir)) => {
            let mut n = ir.top.processes.len();
            for m in ir.modules.values() {
                n += m.processes.len();
            }
            ev.process_count = n;
        }
        _ => ev.process_count = 0,
    }

    if let Ok(Ok((sigs, trace))) =
        std::panic::catch_unwind(|| simulate_signals_with_trace_quiet(source, max_time, 10))
    {
        ev.signal_count = sigs.len();
        let mut names = std::collections::HashSet::new();
        for line in &trace {
            if let Some(idx) = line.find(':') {
                let n = line[..idx].to_string();
                if !n.is_empty() {
                    names.insert(n);
                }
            }
        }
        ev.active_signals = names.len();
        for (_, v) in &sigs {
            if v.all_x() {
                ev.x_remain += 1;
            } else if v.all_z() {
                ev.z_remain += 1;
            }
            let (xb, tb) = x_bit_census(v);
            ev.x_bits += xb;
            ev.bit_total += tb;
        }
    }

    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sensus per-bit menangkap partial-X yang `all_x()` lewatkan:
    /// 31/32 bit X = 0 di `x_remain`, 31 di `x_bits`.
    #[test]
    fn x_bit_census_catches_partial_x() {
        use mivon_ir::{LogicVal, LogicVec};
        let mut v = LogicVec::fill(LogicVal::One, 32);
        for b in v.bits.iter_mut().take(31) {
            *b = LogicVal::X;
        }
        assert!(!v.all_x(), "31/32 X bukan all-X");
        let (xb, tb) = x_bit_census(&v);
        assert_eq!((xb, tb), (31, 32));
        let clean = LogicVec::fill(LogicVal::Zero, 8);
        assert_eq!(x_bit_census(&clean), (0, 8));
    }
}
