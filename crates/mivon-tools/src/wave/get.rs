//! Subkomando `get` — random access nilai sinyal dari VCD (WAV-07).

use mivon_core::error::SimError;

use super::data::{parse_vcd, VcdData};
use super::util::{err, format_bits, full_name, sample_at, timeline_of, wildcard_match};

/// Satu entri hasil query: sinyal + daftar (waktu, nilai mentah bits).
#[derive(Debug, Clone)]
pub(crate) struct GetEntry {
    pub(crate) name: String,
    pub(crate) width: usize,
    pub(crate) values: Vec<(u64, String)>,
}

pub(crate) fn get(
    input: &str,
    patterns: &[String],
    at: Option<u64>,
    range: Option<(u64, u64)>,
) -> Result<(), SimError> {
    let data = parse_vcd(input)?;
    let entries = get_data(&data, patterns, at, range)?;

    println!(
        "  get: {} ({} sinyal cocok, timescale={})",
        input,
        entries.len(),
        data.timescale
    );
    for e in &entries {
        match (at, range) {
            (Some(t), _) => {
                let v = e.values.first().map(|(_, v)| v.as_str()).unwrap_or("x");
                println!(
                    "  {} [{}:0] @ {} = {}",
                    e.name,
                    e.width - 1,
                    t,
                    format_bits(v)
                );
            }
            (None, Some((lo, hi))) => {
                println!("  {} [{}:0] [{}:{}]", e.name, e.width - 1, lo, hi);
                for (t, v) in &e.values {
                    println!("    {:>10}  {}", t, format_bits(v));
                }
            }
            (None, None) => {
                println!(
                    "  {} [{}:0] ({} perubahan)",
                    e.name,
                    e.width - 1,
                    e.values.len()
                );
                for (t, v) in &e.values {
                    println!("    {:>10}  {}", t, format_bits(v));
                }
            }
        }
    }
    Ok(())
}

/// Core query (murni, bisa diuji): resolve pola → entri nilai.
/// Mode `at`: satu sample per sinyal. Mode range/full: perubahan dalam window.
/// Tiga mode: `--at T` (sample di waktu T), `--range a:b` (perubahan dalam
/// window), tanpa flag (timeline penuh).
pub(crate) fn get_data(
    data: &VcdData,
    patterns: &[String],
    at: Option<u64>,
    range: Option<(u64, u64)>,
) -> Result<Vec<GetEntry>, SimError> {
    let pats: Vec<&str> = patterns
        .iter()
        .flat_map(|p| p.split([',', ' ', ';']))
        .filter(|p| !p.is_empty())
        .collect();
    if pats.is_empty() {
        return Err(err(
            "get membutuhkan minimal 1 pola sinyal (dukung * dan ?)",
        ));
    }

    let mut entries: Vec<GetEntry> = Vec::new();
    for sig in &data.signals {
        let full = full_name(sig);
        let hit = pats
            .iter()
            .any(|p| wildcard_match(p, &sig.name) || wildcard_match(p, &full));
        if !hit {
            continue;
        }
        let tl = timeline_of(data, sig);
        let values = match at {
            Some(t) => vec![(t, sample_at(&tl, t))],
            None => match range {
                Some((lo, hi)) => tl
                    .into_iter()
                    .filter(|(t, _)| *t >= lo && *t <= hi)
                    .collect(),
                None => tl,
            },
        };
        entries.push(GetEntry {
            name: full,
            width: sig.width,
            values,
        });
    }

    if entries.is_empty() {
        return Err(err(format!(
            "tidak ada sinyal yang cocok dengan: {}",
            pats.join(", ")
        )));
    }
    Ok(entries)
}
