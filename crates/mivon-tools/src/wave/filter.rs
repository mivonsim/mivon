//! Subkomando `filter` — pertahankan subset sinyal, id di-remap berurutan.

use std::collections::HashMap;

use mivon_core::error::SimError;

use super::data::{parse_vcd, write_vcd, VcdSignal};
use super::util::{err, full_name};

pub(crate) fn filter(input: &str, keep: &[String], output: Option<&str>) -> Result<(), SimError> {
    let data = parse_vcd(input)?;
    let keep_set: std::collections::HashSet<String> = keep
        .iter()
        .flat_map(|s| s.split([',', ' ', ';']))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    let keep_ids: std::collections::HashSet<String> = data
        .signals
        .iter()
        .filter(|s| keep_set.contains(&s.name) || keep_set.contains(&full_name(s)))
        .map(|s| s.id.clone())
        .collect();

    let signals: Vec<VcdSignal> = data
        .signals
        .iter()
        .filter(|s| keep_ids.contains(&s.id))
        .cloned()
        .collect();
    let changes: Vec<(u64, String, String)> = data
        .changes
        .iter()
        .filter(|(_, id, _)| keep_ids.contains(id))
        .cloned()
        .collect();

    if signals.is_empty() {
        return Err(err(format!(
            "tidak ada sinyal yang cocok — sinyal yang ada: {}",
            data.signals
                .iter()
                .map(full_name)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    // Remap id berurutan agar ringkas
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut counter = 33u8;
    for sig in signals.iter() {
        let new_id = format!("{}", char::from_u32(counter as u32).unwrap_or('!'));
        id_map.insert(sig.id.clone(), new_id);
        counter = (counter + 1).max(127);
    }

    let out_path = output.map(|s| s.to_string()).unwrap_or_else(|| {
        let base = input.rsplit('/').next().unwrap_or(input);
        format!("filtered_{}", base)
    });
    write_vcd(&out_path, &data.timescale, &signals, &changes, &id_map)?;
    println!(
        "  filtered {} → {} ({} sinyal dipertahankan)",
        input,
        out_path,
        signals.len()
    );
    Ok(())
}
