//! Design-for-debug manifest (FEAT-15, Medium).
//!
//! Menghasilkan artefak machine-readable yang memetakan seluruh sinyal yang
//! dapat di-probe dari hasil simulasi (VCD), berguna sebagai input alat
//! debug pasca-silikon (logic analyzer, trace sinkronisasi). Ini adalah
//! subset "design-for-debug infrastructure": probe-point map + estimasi
//! hot-spot debug dari kepadatan perubahan nilai per sinyal.
//!
//! Satu file = satu tanggung jawab. Dipakai oleh subkomando `mwave debug`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use mivon_core::error::SimError;

use super::data::VcdData;
use super::util::err;

/// Satu titik probe (sinyal) dalam manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProbePoint {
    pub name: String,
    pub scope: Vec<String>,
    pub width: usize,
    /// Jumlah transisi nilai (proxy kepadatan aktivitas untuk prioritas probe).
    pub transitions: usize,
}

/// Manifest design-for-debug: daftar probe + metadata waktu.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebugManifest {
    pub timescale: String,
    pub max_time: u64,
    pub total_changes: usize,
    pub probes: Vec<ProbePoint>,
}

impl DebugManifest {
    /// Bangun manifest dari data VCD yang sudah di-parse.
    pub fn from_vcd(data: &VcdData) -> DebugManifest {
        let mut transitions: HashMap<String, usize> = HashMap::new();
        for (_, id, _) in &data.changes {
            *transitions.entry(id.clone()).or_insert(0) += 1;
        }
        let mut probes: Vec<ProbePoint> = data
            .signals
            .iter()
            .map(|s| ProbePoint {
                name: s.name.clone(),
                scope: s.scope.clone(),
                width: s.width,
                transitions: *transitions.get(&s.id).unwrap_or(&0),
            })
            .collect();
        probes.sort_by(|a, b| {
            a.scope
                .cmp(&b.scope)
                .then_with(|| a.name.cmp(&b.name))
        });
        DebugManifest {
            timescale: data.timescale.clone(),
            max_time: data.max_time,
            total_changes: data.changes.len(),
            probes,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn write_json(&self, path: &str) -> Result<(), SimError> {
        std::fs::write(path, self.to_json()).map_err(|e| err(format!("{}: {}", path, e)))
    }

    pub fn from_json(s: &str) -> Result<DebugManifest, SimError> {
        serde_json::from_str(s).map_err(|e| err(format!("manifest json: {}", e)))
    }
}

/// Subkomando `mwave debug`: VCD -> manifest JSON.
pub(crate) fn debug(input: &str, output: Option<&str>) -> Result<(), SimError> {
    let data = super::data::parse_vcd(input)?;
    let manifest = DebugManifest::from_vcd(&data);
    let out_path = output
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}.debug.json", input.replace(".vcd", "")));
    manifest.write_json(&out_path)?;
    println!(
        "  debug manifest -> {} ({} probe, {} transisi)",
        out_path,
        manifest.probes.len(),
        manifest.total_changes
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wave::data::VcdSignal;

    fn sample() -> VcdData {
        let signals = vec![
            VcdSignal {
                id: "c".to_string(),
                name: "clk".to_string(),
                width: 1,
                scope: vec!["top".to_string()],
            },
            VcdSignal {
                id: "D".to_string(),
                name: "data".to_string(),
                width: 8,
                scope: vec!["top".to_string(), "alu".to_string()],
            },
        ];
        let changes = vec![
            (0u64, "c".to_string(), "1".to_string()),
            (5, "c".to_string(), "0".to_string()),
            (10, "c".to_string(), "1".to_string()),
            (15, "D".to_string(), "10101010".to_string()),
        ];
        VcdData {
            timescale: "1ns".to_string(),
            signals,
            changes,
            max_time: 15,
        }
    }

    #[test]
    fn builds_probe_points_with_transitions() {
        let m = DebugManifest::from_vcd(&sample());
        assert_eq!(m.probes.len(), 2);
        let clk = m.probes.iter().find(|p| p.name == "clk").unwrap();
        assert_eq!(clk.width, 1);
        assert_eq!(clk.transitions, 3);
        assert_eq!(clk.scope, vec!["top"]);
        let data = m.probes.iter().find(|p| p.name == "data").unwrap();
        assert_eq!(data.width, 8);
        assert_eq!(data.transitions, 1);
        assert_eq!(m.total_changes, 4);
        assert_eq!(m.max_time, 15);
        assert_eq!(m.timescale, "1ns");
    }

    #[test]
    fn probes_sorted_by_scope_then_name() {
        let m = DebugManifest::from_vcd(&sample());
        // clk (scope ["top"]) sebelum data (scope ["top","alu"]).
        assert_eq!(m.probes[0].name, "clk");
        assert_eq!(m.probes[1].name, "data");
    }

    #[test]
    fn json_roundtrip_is_exact() {
        let m = DebugManifest::from_vcd(&sample());
        let json = m.to_json();
        let back = DebugManifest::from_json(&json).unwrap();
        assert_eq!(back, m);
        assert!(json.contains("probes"));
        assert!(json.contains("transitions"));
        assert!(json.contains("alu"));
    }

    #[test]
    fn from_json_rejects_garbage() {
        assert!(DebugManifest::from_json("not json").is_err());
    }
}
