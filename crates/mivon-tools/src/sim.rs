//! `msim` — Simulator.
//!
//! Wrapper ringkas pipeline simulasi: compile → elaborate → run engine →
//! VCD/FST + ringkasan assertion/coverage.

use crate::{kv, open_elaborated, section};
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::ElaborateMode;
use mivon_simulator::simulator::SimulationEngine;
use std::time::Instant;

/// Opsi msim.
pub struct SimArgs<'a> {
    pub files: &'a [String],
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub top: Option<&'a str>,
    pub max_time: u64,
    pub output: Option<&'a str>,
    pub fst: bool,
    pub assertions: bool,
    pub coverage: bool,
}

/// Jalankan msim.
pub fn run(args: &SimArgs) -> Result<(), SimError> {
    // Use StrictSimulation mode for simulation tools (Rule 10)
    let (_session, _design, ir) = open_elaborated(
        args.files,
        args.incdirs,
        args.defines,
        args.top,
        ElaborateMode::StrictSimulation,
    )?;
    let top_name = ir.top.name.as_str();

    let mut engine = SimulationEngine::new(ir, args.max_time);

    // VCD output
    let vcd_path = args
        .output
        .map(|o| {
            if o.ends_with(".vcd") {
                o.to_string()
            } else {
                format!("{}.vcd", o)
            }
        })
        .unwrap_or_else(|| format!("{}.vcd", top_name));
    let vcd = mivon_simulator::waveform::VcdWriter::new(&vcd_path, &engine.design)
        .map_err(|e| SimError::with_diag(mivon_core::diagnostics::DiagCode::WaveformError, e))?;
    engine.set_vcd(vcd);

    // FST output
    if args.fst {
        let fst_path = format!("{}.fst", top_name);
        match mivon_simulator::waveform::FstWaveWriter::new(&fst_path, &engine.design) {
            Ok(fst) => {
                engine.set_fst(fst);
            }
            Err(e) => {
                let diag = mivon_core::diagnostics::Diagnostic::warning(
                    mivon_core::diagnostics::DiagCode::WaveformError,
                    format!("FST: cannot create '{}': {}", fst_path, e),
                );
                let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
                let _ = emitter.emit(&diag);
            }
        }
    }

    section("Simulation");
    let sim_start = Instant::now();
    engine.run()?;
    let sim_ms = sim_start.elapsed().as_millis() as u64;

    kv("end time", format!("#{}", engine.state.time));
    kv("sim time", format!("{} ms", sim_ms));
    kv("VCD", vcd_path);

    // Diagnostics runtime
    let diags = engine.flush_diagnostics();
    let assertion_errors = diags
        .iter()
        .filter(|d| d.code == mivon_core::diagnostics::DiagCode::AssertionFailed)
        .count();
    if !diags.is_empty() {
        let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
        for d in &diags {
            let _ = emitter.emit(d);
        }
    }

    if args.assertions {
        section("Assertion Summary");
        kv("failed", assertion_errors);
        kv("passed", "—");
    }

    if args.coverage {
        section("Coverage Summary");
        let stats = engine.coverage_stats();
        for (k, v) in stats.iter().collect::<Vec<_>>().iter() {
            kv(k, v);
        }
        // Simpan ringkasan ke cache pipeline (db.md "19. coverage/") agar
        // `minspect cache` membaca hasil tanpa simulasi ulang.
        save_coverage_cache(args, &stats);
    }

    // Simpan ringkasan simulasi + index signal ke cache pipeline (db.md
    // "17. simulation/", "18. waveform/") — initial state, sensitivity list,
    // scheduler, signal index — agar tool lain membacanya tanpa simulasi ulang.
    save_sim_waveform_cache(args, &engine);

    if args.assertions && assertion_errors > 0 {
        return Err(SimError::with_diag(
            mivon_core::diagnostics::DiagCode::AssertionFailed,
            format!("{} assertion(s) gagal", assertion_errors),
        ));
    }
    // F15: $fatal menghentikan sim dengan kegagalan → exit code non-zero
    // (pola sama dengan assertion failed).
    if engine.sev_fatal_count > 0 {
        return Err(SimError::with_diag(
            mivon_core::diagnostics::DiagCode::AssertionFailed,
            format!(
                "$fatal: simulasi dihentikan ({} fatal)",
                engine.sev_fatal_count
            ),
        ));
    }
    Ok(())
}

/// Simpan ringkasan coverage ke cache pipeline (`coverage/"last"`, db.md
/// "19. coverage/"). Best-effort — kegagalan cache tidak menggagalkan msim.
fn save_coverage_cache(args: &SimArgs, stats: &std::collections::HashMap<String, f64>) {
    use mivon_compiler::node_cache::cache::pipeline::CoveragePayload;
    use mivon_compiler::node_cache::cache::CacheCategory;

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

/// Simpan ringkasan simulasi + index signal ke cache pipeline (db.md
/// "17. simulation/", "18. waveform/"). Best-effort — kegagalan cache tidak
/// menggagalkan msim. `simulation/"last"` berisi initial state + scheduler +
/// sensitivity list; `waveform/"last"` berisi signal index top (nama, lebar,
/// kind, net) agar VCD/FST lebih cepat dibuka tanpa mem-parse ulang.
fn save_sim_waveform_cache(args: &SimArgs, engine: &SimulationEngine) {
    use mivon_compiler::node_cache::cache::pipeline::{
        ProcessCounts, SimulationPayload, WaveSignal, WaveformPayload,
    };
    use mivon_compiler::node_cache::cache::CacheCategory;
    use mivon_ir::{NetType, Process, SignalKind};

    let Ok((mut layer, _pid)) = crate::open_cache_layer(args.files, args.incdirs, args.defines)
    else {
        return;
    };

    // sensitivity list + initial state dari top (post-flatten).
    let mut processes = ProcessCounts::default();
    for proc in &engine.design.top.processes {
        match proc {
            Process::Combinational { .. } => processes.combinational += 1,
            Process::CombReactive { .. } => processes.comb_reactive += 1,
            Process::Sequential { .. } => processes.sequential += 1,
            Process::Initial { .. } => processes.initial += 1,
            Process::Final { .. } => processes.final_ += 1,
            Process::AlwaysWithDelay { .. } => processes.always_with_delay += 1,
        }
    }
    let mut init_signals = 0usize;
    for s in &engine.design.top.signals {
        if s.init_val.to_u64() != 0 {
            init_signals += 1;
        }
    }
    let sim_payload = SimulationPayload {
        end_time: engine.state.time,
        events_processed: engine.sim_perf.counters.events_processed,
        signal_count: engine.design.top.signals.len(),
        init_signals,
        processes,
    };
    if let Ok(bytes) = bincode::serialize(&sim_payload) {
        let _ = layer.put(CacheCategory::Simulation, "last", &bytes);
    }

    // Signal index top → VCD/FST dibuka tanpa mem-parse ulang.
    let kind_of = |k: &SignalKind| match k {
        SignalKind::Wire => "wire",
        SignalKind::Reg => "reg",
        SignalKind::Logic => "logic",
        SignalKind::Input => "input",
        SignalKind::Output => "output",
        SignalKind::Inout => "inout",
    };
    let net_of = |n: NetType| match n {
        NetType::Wire => "wire",
        NetType::Wand => "wand",
        NetType::Wor => "wor",
        NetType::Tri => "tri",
        NetType::Tri0 => "tri0",
        NetType::Tri1 => "tri1",
        NetType::TriAnd => "triand",
        NetType::TriOr => "trior",
        NetType::Supply0 => "supply0",
        NetType::Supply1 => "supply1",
    };
    let wave = WaveformPayload {
        signals: engine
            .design
            .top
            .signals
            .iter()
            .map(|s| WaveSignal {
                name: s.name.to_string(),
                width: s.width,
                kind: kind_of(&s.kind).to_string(),
                net: net_of(s.net_type).to_string(),
                is_signed: s.is_signed,
            })
            .collect(),
    };
    if let Ok(bytes) = bincode::serialize(&wave) {
        let _ = layer.put(CacheCategory::Waveform, "last", &bytes);
    }
    let _ = layer.save();
}
