//! Subkomando `stats` — statistik aktivitas per sinyal (WAV-17).

use mivon_core::error::SimError;

use super::data::{parse_vcd, VcdData};
use super::util::full_name;

/// Satu baris statistik per sinyal.
pub(crate) struct SignalStats {
    pub(crate) name: String,
    pub(crate) width: usize,
    pub(crate) toggles: usize,
    pub(crate) changes: usize,
    pub(crate) first: u64,
    pub(crate) last: u64,
    pub(crate) activity: u64,
    pub(crate) duration: u64,
}

pub(crate) fn stats(input: &str) -> Result<(), SimError> {
    let data = parse_vcd(input)?;
    let r = stats_data(&data);

    // ── Laporan ──
    println!(
        "  stats: {} ({} sinyal, max_time={}, timescale={})",
        input,
        r.len(),
        data.max_time,
        data.timescale
    );
    println!(
        "  {:<22} {:<5} {:<7} {:<7} {:<10} {:<7} activity%",
        "signal", "width", "toggle", "change", "first", "last"
    );
    let mut total_toggle = 0usize;
    let mut stuck = 0usize;
    for s in &r {
        total_toggle += s.toggles;
        if s.toggles == 0 {
            stuck += 1;
        }
        let act = s
            .activity
            .checked_mul(100)
            .and_then(|a| a.checked_div(s.duration))
            .unwrap_or(0);
        println!(
            "  {:<22} {:<5} {:<7} {:<7} {:<10} {:<7} {}%",
            s.name, s.width, s.toggles, s.changes, s.first, s.last, act
        );
    }
    println!(
        "  total toggle: {} ({} sinyal stuck / 0 aktivitas)",
        total_toggle, stuck
    );
    Ok(())
}

/// Logika statistik per sinyal (dipisah dari pencetakan agar bisa diuji).
/// Toggle = transisi nilai antar perubahan berurutan (bitwise berbeda),
/// activity = jumlah unit waktu di mana nilai != nilai commit pertama.
pub(crate) fn stats_data(data: &VcdData) -> Vec<SignalStats> {
    let mut out: Vec<SignalStats> = Vec::new();
    let end = data.max_time;

    for sig in &data.signals {
        let full = full_name(sig);
        let mut tl: Vec<(u64, String)> = data
            .changes
            .iter()
            .filter(|(_, id, _)| id == &sig.id)
            .map(|(t, _, v)| (*t, v.clone()))
            .collect();
        tl.sort_by_key(|(t, _)| *t);

        if tl.is_empty() {
            out.push(SignalStats {
                name: full,
                width: sig.width,
                toggles: 0,
                changes: 0,
                first: 0,
                last: 0,
                activity: 0,
                duration: end,
            });
            continue;
        }

        let first_t = tl[0].0;
        let last_t = tl[tl.len() - 1].0;
        let mut toggles = 0usize;
        let mut prev = tl[0].1.clone();
        for (_, v) in tl.iter().skip(1) {
            if v != &prev {
                toggles += 1;
            }
            prev = v.clone();
        }
        // Aktivitas: waktu di mana nilai berlaku != nilai commit pertama.
        let baseline = tl[0].1.clone();
        let mut activity = 0u64;
        for w in tl.windows(2) {
            let (t0, v0) = &w[0];
            let (t1, _) = &w[1];
            if *v0 != baseline {
                activity += t1 - t0;
            }
        }
        if let Some((t_last, v_last)) = tl.last() {
            if *v_last != baseline {
                activity += end.saturating_sub(*t_last);
            }
        }
        out.push(SignalStats {
            name: full,
            width: sig.width,
            toggles,
            changes: tl.len(),
            first: first_t,
            last: last_t,
            activity,
            duration: end,
        });
    }
    out
}
