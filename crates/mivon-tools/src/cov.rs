//! `mcov` — Coverage Analyzer.
//!
//! Menghasilkan `coverage.json` + `coverage.html` dari hasil simulasi.
//! Jenis coverage: line, toggle, branch, FSM, covergroup, assertion.

use crate::{kv, open_elaborated, section};
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::ElaborateMode;
use mivon_simulator::simulator::SimulationEngine;
use std::time::Instant;

/// Opsi mcov.
pub struct CovArgs<'a> {
    pub files: &'a [String],
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub top: Option<&'a str>,
    pub max_time: u64,
    pub output: Option<&'a str>,
    pub json: bool,
    pub html: bool,
    pub threshold: Option<f64>,
}

/// Jalankan mcov.
pub fn run(args: &CovArgs) -> Result<(), SimError> {
    // Use StrictSimulation mode for coverage (requires simulation)
    let (_session, _design, ir) = open_elaborated(
        args.files,
        args.incdirs,
        args.defines,
        args.top,
        ElaborateMode::StrictSimulation,
    )?;
    let top_name = ir.top.name.as_str();

    let mut engine = SimulationEngine::new(ir, args.max_time);

    section("Coverage Simulation");
    let sim_start = Instant::now();
    engine.run()?;
    kv(
        "sim time",
        format!("{} ms", sim_start.elapsed().as_millis()),
    );

    let stats = engine.coverage_stats();

    // Simpan ringkasan ke cache pipeline (db.md "19. coverage/") agar
    // `minspect cache` membaca hasil tanpa simulasi ulang.
    save_coverage_cache(args, &stats);

    let prefix = args
        .output
        .map(|o| {
            o.trim_end_matches(".coverage.json")
                .trim_end_matches(".coverage.html")
                .to_string()
        })
        .unwrap_or_else(|| top_name.to_string());

    // ── Ringkasan ──
    section("Coverage Summary");
    let branch_pct = stats.get("branch_percent").copied().unwrap_or(0.0);
    let branch_total = stats.get("branch_total").copied().unwrap_or(0.0) as u64;
    let branch_covered = stats.get("branch_covered").copied().unwrap_or(0.0) as u64;
    let line_items = stats.get("line_items").copied().unwrap_or(0.0) as u64;
    let line_hits = stats.get("line_total_hits").copied().unwrap_or(0.0) as u64;
    let toggle_signals = stats.get("toggle_signals").copied().unwrap_or(0.0) as u64;
    let toggle_transitions = stats.get("toggle_transitions").copied().unwrap_or(0.0) as u64;
    let fsm_signals = stats.get("fsm_signals").copied().unwrap_or(0.0) as u64;
    let fsm_states = stats.get("fsm_states").copied().unwrap_or(0.0) as u64;

    kv("line", format!("{}/{}", line_hits, line_items));
    kv(
        "branch",
        format!("{}/{} ({:.1}%)", branch_covered, branch_total, branch_pct),
    );
    kv(
        "toggle",
        format!(
            "{} signals, {} transitions",
            toggle_signals, toggle_transitions
        ),
    );
    kv(
        "fsm",
        format!("{} signals, {} states", fsm_signals, fsm_states),
    );
    // Covergroup (VERIF-28) — dulu tak pernah tampil padahal datanya ada
    // di engine/CoverageDatabase (bug: report tampak lengkap, covergroup
    // kosong senyap).
    let cg_points = stats.get("covergroup_points").copied().unwrap_or(0.0) as u64;
    let cg_covered = stats.get("covergroup_covered").copied().unwrap_or(0.0) as u64;
    let cg_pct = stats.get("covergroup_percent").copied().unwrap_or(0.0);
    kv(
        "covergroup",
        format!("{}/{} ({:.1}%)", cg_covered, cg_points, cg_pct),
    );

    // ── coverage.json ──
    if args.json {
        let json_path = format!("{}.coverage.json", prefix);
        let obj = serde_json::json!({
            "tool": "mivon mcov",
            "top_module": top_name,
            "max_time": args.max_time,
            "coverage": {
                "line": { "items": line_items, "hits": line_hits,
                          "percent": if line_items > 0 { line_hits as f64 / line_items as f64 * 100.0 } else { 0.0 } },
                "branch": { "total": branch_total, "covered": branch_covered, "percent": branch_pct },
                "toggle": { "signals": toggle_signals, "transitions": toggle_transitions },
                "fsm": { "signals": fsm_signals, "states": fsm_states },
                "covergroup": { "points": cg_points, "covered": cg_covered, "percent": cg_pct },
            }
        });
        let json = serde_json::to_string_pretty(&obj).map_err(|e| {
            SimError::with_diag(
                mivon_core::diagnostics::DiagCode::InternalError,
                format!("json: {}", e),
            )
        })?;
        std::fs::write(&json_path, json).map_err(|e| {
            SimError::with_diag(
                mivon_core::diagnostics::DiagCode::IoError,
                format!("{}: {}", json_path, e),
            )
        })?;
        println!("  coverage.json → {}", json_path);
    }

    // ── coverage.html ──
    if args.html {
        let html_path = format!("{}.coverage.html", prefix);
        let mut covdb = mivon_simulator::simulator::coverage_db::CoverageDatabase::new();
        covdb.merge_from_engine(&engine);
        covdb.export_html(&html_path).map_err(|e| {
            SimError::with_diag(
                mivon_core::diagnostics::DiagCode::IoError,
                format!("html: {}", e),
            )
        })?;
        println!("  coverage.html → {}", html_path);
    }

    // ── Threshold ──
    if let Some(threshold) = args.threshold {
        if branch_pct < threshold {
            return Err(SimError::with_diag(
                mivon_core::diagnostics::DiagCode::AssertionFailed,
                format!(
                    "COVERAGE FAILED: branch {:.1}% < threshold {:.1}%",
                    branch_pct, threshold
                ),
            ));
        }
        println!(
            "  ✓ branch coverage {:.1}% >= threshold {:.1}%",
            branch_pct, threshold
        );
    }

    Ok(())
}

/// Simpan ringkasan coverage ke cache pipeline (`coverage/"last"`, db.md
/// "19. coverage/"). Best-effort — kegagalan cache tidak menggagalkan mcov.
fn save_coverage_cache(args: &CovArgs, stats: &std::collections::HashMap<String, f64>) {
    use mivon_compiler::micd::cache::pipeline::CoveragePayload;
    use mivon_compiler::micd::cache::CacheCategory;

    let Ok((mut layer, _pid)) = crate::open_cache_layer(args.files, args.incdirs, args.defines)
    else {
        return;
    };
    let get = |k: &str| stats.get(k).copied().unwrap_or(0.0) as u64;
    let payload = CoveragePayload {
        line_items: get("line_items"),
        line_hits: get("line_total_hits"),
        branch_total: get("branch_total"),
        branch_covered: get("branch_covered"),
        toggle_signals: get("toggle_signals"),
        toggle_transitions: get("toggle_transitions"),
        fsm_signals: get("fsm_signals"),
        fsm_states: get("fsm_states"),
    };
    if let Ok(bytes) = bincode::serialize(&payload) {
        let _ = layer.put(CacheCategory::Coverage, "last", &bytes);
        let _ = layer.save();
    }
}
