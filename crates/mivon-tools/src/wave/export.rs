//! Subkomando `export` — VCD → CSV/TXT.

use mivon_core::error::SimError;

use super::data::{parse_vcd, VcdData, VcdSignal};
use super::util::{err, full_name, input_name, signal_timelines};

pub(crate) fn export(input: &str, format: &str, output: Option<&str>) -> Result<(), SimError> {
    let data = parse_vcd(input)?;
    let out_path = output
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}.{}", input.replace(".vcd", ""), format));

    match format {
        "csv" => export_csv(&data, &out_path),
        "txt" => export_txt(&data, &out_path),
        other => Err(err(format!(
            "format tidak dikenal: '{}' (pilih csv|txt)",
            other
        ))),
    }
}

pub(crate) fn export_csv(data: &VcdData, out_path: &str) -> Result<(), SimError> {
    let timelines = signal_timelines(data);
    let names: Vec<&VcdSignal> = data.signals.iter().collect();

    let mut header = String::from("time");
    for s in &names {
        header.push(',');
        header.push_str(&full_name(s));
    }

    // Semua momen waktu: 0 + semua waktu perubahan
    let mut times: Vec<u64> = vec![0];
    times.extend(data.changes.iter().map(|(t, _, _)| *t));
    times.sort_unstable();
    times.dedup();

    let mut out = String::new();
    out.push_str(&header);
    out.push('\n');
    for t in &times {
        let mut row = format!("{}", t);
        for s in &names {
            let last = timelines
                .get(&s.id)
                .and_then(|v| v.iter().rev().find(|(tt, _)| tt <= t))
                .map(|(_, v)| v.clone());
            row.push(',');
            match last {
                Some(v) => row.push_str(&v),
                None => row.push('z'),
            }
        }
        out.push_str(&row);
        out.push('\n');
    }

    std::fs::write(out_path, out).map_err(|e| err(format!("{}: {}", out_path, e)))?;
    println!(
        "  exported {} → {} (csv, {} baris)",
        input_name(data),
        out_path,
        times.len()
    );
    Ok(())
}

pub(crate) fn export_txt(data: &VcdData, out_path: &str) -> Result<(), SimError> {
    let timelines = signal_timelines(data);
    let mut out = String::new();
    for s in &data.signals {
        out.push_str(&format!("{} [{}]:\n", full_name(s), s.width));
        let ts = timelines.get(&s.id).cloned().unwrap_or_default();
        for (t, v) in ts {
            out.push_str(&format!("    #{} = {}\n", t, v));
        }
    }
    std::fs::write(out_path, out).map_err(|e| err(format!("{}: {}", out_path, e)))?;
    println!("  exported {} → {}", input_name(data), out_path);
    Ok(())
}
