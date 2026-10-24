//! Post-silicon validation alignment (FEAT-18, Medium).
//!
//! Mengkorelasikan trace pre-silicon (simulasi, VCD) dengan capture
//! post-silicon (logic analyzer/emulator, VCD) menggunakan sinyal trigger
//! sebagai jangkar waktu. Menghasilkan laporan alignment per-cycle yang
//! menunjukkan siklus pertama yang berbeda (jika ada), berguna untuk
//! memvalidasi bahwa RTL berperilaku sama dengan silikon.
//!
//! Satu file = satu tanggung jawab. Dipakai oleh subkomando mwave align.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use mivon_core::error::SimError;

use super::data::VcdData;
use super::util::err;

/// Satu ketidakcocokan pada offset alignment tertentu.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mismatch {
    pub offset: u64,
    pub signal: String,
    pub pre_value: String,
    pub post_value: String,
}

/// Laporan alignment pre-silicon vs post-silicon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlignReport {
    pub trigger: String,
    pub pre_anchor_time: u64,
    pub post_anchor_time: u64,
    pub shared_signals: Vec<String>,
    pub aligned_cycles: u64,
    pub first_mismatch: Option<Mismatch>,
    pub pre_only_signals: Vec<String>,
    pub post_only_signals: Vec<String>,
}

/// Bangun name -> id dari daftar sinyal.
fn name_to_id(data: &VcdData) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for s in &data.signals {
        m.insert(s.name.clone(), s.id.clone());
    }
    m
}

/// Bangun timeline per id: Vec<(time, value)> terurut naik.
fn build_timelines(data: &VcdData) -> HashMap<String, Vec<(u64, String)>> {
    let mut m: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    for (t, id, v) in &data.changes {
        m.entry(id.clone()).or_default().push((*t, v.clone()));
    }
    m
}

/// Nilai sinyal pada waktu t (nilai terakhir <= t, atau x bila belum ada).
fn value_at(timeline: &[(u64, String)], t: u64) -> String {
    let mut last = "x".to_string();
    for (tt, v) in timeline {
        if *tt <= t {
            last = v.clone();
        } else {
            break;
        }
    }
    last
}

/// Cari jangkar: waktu pertama trigger bernilai 1, else 0.
fn find_anchor(timeline: &[(u64, String)]) -> u64 {
    for (t, v) in timeline {
        if v == "1" {
            return *t;
        }
    }
    0
}

/// Core alignment: bandingkan sinyal bersama dari jangkar trigger.
pub fn align_signals(
    pre: &VcdData,
    post: &VcdData,
    trigger: &str,
) -> Result<AlignReport, SimError> {
    let pre_ids = name_to_id(pre);
    let post_ids = name_to_id(post);
    let pre_trig = pre_ids
        .get(trigger)
        .ok_or_else(|| err(format!(r#"trigger {} not in pre-silicon VCD"#, trigger)))?;
    let post_trig = post_ids
        .get(trigger)
        .ok_or_else(|| err(format!(r#"trigger {} not in post-silicon VCD"#, trigger)))?;

    let pre_tl = build_timelines(pre);
    let post_tl = build_timelines(post);

    let pre_anchor = find_anchor(pre_tl.get(pre_trig).map(|v| v.as_slice()).unwrap_or(&[]));
    let post_anchor = find_anchor(post_tl.get(post_trig).map(|v| v.as_slice()).unwrap_or(&[]));

    let mut shared: Vec<String> = pre_ids
        .keys()
        .filter(|n| post_ids.contains_key(*n))
        .cloned()
        .collect();
    shared.sort();

    let pre_only: Vec<String> = pre_ids
        .keys()
        .filter(|n| !post_ids.contains_key(*n))
        .cloned()
        .collect();
    let post_only: Vec<String> = post_ids
        .keys()
        .filter(|n| !pre_ids.contains_key(*n))
        .cloned()
        .collect();

    let max_off = std::cmp::min(
        pre.max_time.saturating_sub(pre_anchor),
        post.max_time.saturating_sub(post_anchor),
    );

    let mut first_mismatch: Option<Mismatch> = None;
    let mut aligned_cycles: u64 = 0;
    for i in 0..=max_off {
        let pt = pre_anchor + i;
        let pot = post_anchor + i;
        for name in &shared {
            let pid = &pre_ids[name];
            let qid = &post_ids[name];
            let pv = value_at(pre_tl.get(pid).map(|v| v.as_slice()).unwrap_or(&[]), pt);
            let qv = value_at(post_tl.get(qid).map(|v| v.as_slice()).unwrap_or(&[]), pot);
            if pv != qv && first_mismatch.is_none() {
                first_mismatch = Some(Mismatch {
                    offset: i,
                    signal: name.clone(),
                    pre_value: pv,
                    post_value: qv,
                });
            }
        }
        aligned_cycles += 1;
    }

    Ok(AlignReport {
        trigger: trigger.to_string(),
        pre_anchor_time: pre_anchor,
        post_anchor_time: post_anchor,
        shared_signals: shared,
        aligned_cycles,
        first_mismatch,
        pre_only_signals: pre_only,
        post_only_signals: post_only,
    })
}

/// Subkomando mwave align: dua VCD -> laporan JSON.
pub(crate) fn align(
    pre_path: &str,
    post_path: &str,
    trigger: &str,
    output: Option<&str>,
) -> Result<(), SimError> {
    let pre = super::data::parse_vcd(pre_path)?;
    let post = super::data::parse_vcd(post_path)?;
    let report = align_signals(&pre, &post, trigger)?;
    let json = serde_json::to_string_pretty(&report).unwrap_or_default();
    match output {
        Some(p) => {
            std::fs::write(p, &json).map_err(|e| err(format!(r#"{}: {}"#, p, e)))?;
            println!(
                r#"  align -> {} (shared={}, aligned_cycles={}, mismatch={})"#,
                p,
                report.shared_signals.len(),
                report.aligned_cycles,
                report.first_mismatch.is_some()
            );
        }
        None => {
            println!("{}", json);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wave::data::VcdSignal;

    fn base(pre: bool) -> VcdData {
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
                width: 4,
                scope: vec!["top".to_string()],
            },
            VcdSignal {
                id: "T".to_string(),
                name: "trig".to_string(),
                width: 1,
                scope: vec!["top".to_string()],
            },
        ];
        let (trig_t, data_off) = if pre {
            (5u64, "0010".to_string())
        } else {
            (7u64, "1111".to_string())
        };
        let changes = vec![
            (0u64, "T".to_string(), "0".to_string()),
            (trig_t, "T".to_string(), "1".to_string()),
            (trig_t, "c".to_string(), "1".to_string()),
            (trig_t, "D".to_string(), "0001".to_string()),
            (trig_t + 5, "D".to_string(), data_off),
        ];
        let max_time = trig_t + 5;
        VcdData {
            timescale: "1ns".to_string(),
            signals,
            changes,
            max_time,
        }
    }

    #[test]
    fn align_finds_first_mismatch_at_offset() {
        let pre = base(true);
        let post = base(false);
        let r = align_signals(&pre, &post, "trig").unwrap();
        assert_eq!(r.pre_anchor_time, 5);
        assert_eq!(r.post_anchor_time, 7);
        assert_eq!(r.shared_signals, vec!["clk", "data", "trig"]);
        assert_eq!(r.aligned_cycles, 6);
        let mm = r.first_mismatch.expect("should mismatch");
        assert_eq!(mm.offset, 5);
        assert_eq!(mm.signal, "data");
        assert_eq!(mm.pre_value, "0010");
        assert_eq!(mm.post_value, "1111");
    }

    #[test]
    fn align_identical_has_no_mismatch() {
        let pre = base(true);
        let post = base(true);
        let r = align_signals(&pre, &post, "trig").unwrap();
        assert!(r.first_mismatch.is_none());
        assert_eq!(r.aligned_cycles, 6);
    }

    #[test]
    fn align_reports_only_side_signals() {
        let mut pre = base(true);
        pre.signals.push(VcdSignal {
            id: "X".to_string(),
            name: "extra".to_string(),
            width: 1,
            scope: vec!["top".to_string()],
        });
        let post = base(true);
        let r = align_signals(&pre, &post, "trig").unwrap();
        assert_eq!(r.pre_only_signals, vec!["extra"]);
        assert!(r.post_only_signals.is_empty());
    }
}
#[test]
fn align_handles_constant_and_no_trigger_assert() {
    fn mk() -> VcdData {
        VcdData {
            timescale: "1ns".to_string(),
            signals: vec![
                crate::wave::data::VcdSignal {
                    id: "T".to_string(),
                    name: "trig".to_string(),
                    width: 1,
                    scope: vec!["top".to_string()],
                },
                crate::wave::data::VcdSignal {
                    id: "D".to_string(),
                    name: "data".to_string(),
                    width: 4,
                    scope: vec!["top".to_string()],
                },
            ],
            changes: vec![(0u64, "T".to_string(), "0".to_string())],
            max_time: 10,
        }
    }
    let pre = mk();
    let post = mk();
    let r = align_signals(&pre, &post, "trig").unwrap();
    assert_eq!(r.pre_anchor_time, 0);
    assert_eq!(r.post_anchor_time, 0);
    assert_eq!(r.aligned_cycles, 11);
    assert!(r.first_mismatch.is_none());
}
