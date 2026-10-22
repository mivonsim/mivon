//! Subkomando `merge` — gabungkan beberapa VCD jadi satu (offset waktu
//! kumulatif, id sinyal diprefix per input).

use std::collections::HashMap;

use mivon_core::error::SimError;

use super::data::{parse_vcd, write_vcd, VcdSignal};
use super::util::err;

pub(crate) fn merge(inputs: &[String], output: Option<&str>) -> Result<(), SimError> {
    if inputs.len() < 2 {
        return Err(err("merge membutuhkan minimal 2 VCD input"));
    }
    let mut all_signals: Vec<VcdSignal> = Vec::new();
    let mut all_changes: Vec<(u64, String, String)> = Vec::new();
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut offset = 0u64;
    let mut timescale = String::new();

    for (idx, path) in inputs.iter().enumerate() {
        let data = parse_vcd(path)?;
        let prefix = format!("{}", char::from_u32(0x41 + idx as u32).unwrap_or('A'));
        for sig in &data.signals {
            let new_id = format!("{}{}", prefix, sig.id);
            id_map.insert(sig.id.clone(), new_id.clone());
            all_signals.push(VcdSignal {
                id: new_id,
                name: sig.name.clone(),
                width: sig.width,
                scope: sig.scope.clone(),
            });
        }
        for (t, id, val) in &data.changes {
            all_changes.push((offset + t, id.clone(), val.clone()));
        }
        offset += data.max_time;
        if timescale.is_empty() {
            timescale = data.timescale.clone();
        }
    }

    all_changes.sort_by_key(|(t, _, _)| *t);
    let out_path = output
        .map(|s| s.to_string())
        .unwrap_or_else(|| "merged.vcd".to_string());
    write_vcd(&out_path, &timescale, &all_signals, &all_changes, &id_map)?;
    println!(
        "  merged {} file → {} ({} signals, {} perubahan, max time {})",
        inputs.len(),
        out_path,
        all_signals.len(),
        all_changes.len(),
        offset
    );
    Ok(())
}
