//! `mprof` — Performance Profiler.
//!
//! Ukur waktu tiap fase pipeline (discovery → preprocess → parse → index →
//! elab → simulation) dan sorot bottleneck.

use crate::{collect_targets, kv, make_session_config_with_mv, section};
use mivon_compiler::frontend::CompileSession;
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::ElaborateMode;
use mivon_simulator::simulator::SimulationEngine;
use std::time::Instant;

/// Opsi mprof.
pub struct ProfArgs<'a> {
    pub targets: &'a [String],
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub top: Option<&'a str>,
    pub max_time: u64,
    /// Baca profil build terakhir dari cache pipeline (tanpa compile).
    pub cached: bool,
}

/// Satu baris timing fase.
struct PhaseTime {
    name: &'static str,
    ms: u64,
}

/// Jalankan mprof.
pub fn run(args: &ProfArgs) -> Result<(), SimError> {
    if args.cached {
        return run_cached(args);
    }
    let files = collect_targets(args.targets)?;
    // F10: `.mv` di-transpile ke buffer inline (svh+sv) agar mprof bisa
    // memprofil pipeline Mivon HDL tanpa menulis file ke disk.
    let cfg = make_session_config_with_mv(
        files,
        args.incdirs,
        args.defines,
        args.top.map(|s| s.to_string()),
    )?;
    let mut session = CompileSession::new(cfg);

    let compile_start = Instant::now();
    // Use StrictSimulation mode for simulation profiling
    let (_design, ir, _len) =
        session.compile_and_elaborate_with_mode(args.top, ElaborateMode::StrictSimulation)?;
    let compile_ms = compile_start.elapsed().as_millis() as u64;

    // Simulation
    let mut engine = SimulationEngine::new(ir, args.max_time);
    let sim_start = Instant::now();
    engine.run()?;
    let sim_ms = sim_start.elapsed().as_millis() as u64;

    let t = &session.timing;
    let mut phases: Vec<PhaseTime> = vec![
        PhaseTime {
            name: "Discovery",
            ms: t.discovery_us / 1000,
        },
        PhaseTime {
            name: "Preprocess",
            ms: t.preprocess_us / 1000,
        },
        PhaseTime {
            name: "Parse",
            ms: t.parse_us / 1000,
        },
        PhaseTime {
            name: "Index",
            ms: t.index_us / 1000,
        },
        PhaseTime {
            name: "Elaboration",
            ms: t.elab_us / 1000,
        },
        PhaseTime {
            name: "Simulation",
            ms: sim_ms,
        },
    ];

    // Drop fase yang tidak terukur (0)
    phases.retain(|p| p.ms > 0);

    let total: u64 = phases.iter().map(|p| p.ms).sum::<u64>().max(1);

    section("Pipeline Profile");
    println!("  {:<14} {:>10} {:>8}", "Phase", "Time (ms)", "%");
    println!("  ─────────────────────────────────");
    for p in &phases {
        let bar_len = ((p.ms as f64 / total as f64) * 40.0) as usize;
        let bar = "█".repeat(bar_len);
        println!(
            "  {:<14} {:>10} {:>7.1}%  {}",
            p.name,
            p.ms,
            (p.ms as f64 / total as f64) * 100.0,
            bar
        );
    }
    println!("  ─────────────────────────────────");
    println!("  {:<14} {:>10}", "Total", total);

    // Bottleneck
    if let Some(bn) = phases.iter().max_by_key(|p| p.ms) {
        section("Bottleneck");
        kv("phase", bn.name);
        kv(
            "time",
            format!(
                "{} ms ({:.1}%)",
                bn.ms,
                (bn.ms as f64 / total as f64) * 100.0
            ),
        );
        kv("hint", bottleneck_hint(bn.name));
    }

    kv("compile (total)", format!("{} ms", compile_ms));
    kv("files", session.config.sources.len());
    kv("processed", t.processed_files);
    kv("cached", t.cached_files);

    Ok(())
}

fn bottleneck_hint(name: &str) -> &'static str {
    match name {
        "Discovery" => "file scan terlalu banyak direktori; pakai file list (-f)",
        "Preprocess" => "`include / `define berat; pakai node_cache (cache preprocess)",
        "Parse" => "banyak file besar; aktifkan --fast (FastLexer + paralel)",
        "Index" => "index module — umumnya cepat",
        "Elaboration" => "generate/parameter expansion berat; pakai --lazy",
        "Simulation" => "kecilkan max_time atau pakai --packed / --jit-body",
        "Save (node_cache)" => {
            "penulisan store node_cache lambat — cache banyak file; pakai MIVON_DEBUG_node_cache untuk detail"
        }
        "Verify" => "verifikasi per file berat; kecilkan --checks atau pakai cache verify",
        "Optimize" => "constant folding / loop unroll; sesuaikan --opt-level",
        "Lexer" => "tokenisasi berat; aktifkan FastLexer (--fast)",
        _ => "—",
    }
}

/// Baca profil build terakhir dari cache pipeline (db.md "20. profile/" —
/// "Dari sini Mivon bisa mengetahui sendiri bottleneck dan bahkan memberikan
/// rekomendasi optimasi") tanpa menjalankan compile/simulasi. Membaca entry
/// `profile/"last"` yang ditulis `save_cache` pada build sebelumnya.
pub fn run_cached(args: &ProfArgs) -> Result<(), SimError> {
    use mivon_compiler::node_cache::cache::CacheCategory;
    use mivon_compiler::node_cache::BuildProfile;

    let (mut layer, pid) = crate::open_cache_layer(args.targets, args.incdirs, args.defines)?;
    let bytes = layer.get(CacheCategory::Profile, "last").ok_or_else(|| {
        SimError::runtime("tidak ada profil cache — jalankan `mprof` (tanpa --cached) sekali dulu")
    })?;
    let prof: BuildProfile = bincode::deserialize(&bytes).map_err(|e| {
        SimError::runtime(format!("profil cache korup (versi skema berubah?): {}", e))
    })?;

    let mut phases: Vec<PhaseTime> = vec![
        PhaseTime {
            name: "Preprocess",
            ms: prof.preprocess_ms,
        },
        PhaseTime {
            name: "Lexer",
            ms: prof.lex_ms,
        },
        PhaseTime {
            name: "Parse",
            ms: prof.parse_ms,
        },
        PhaseTime {
            name: "Elaboration",
            ms: prof.elaborate_ms,
        },
        PhaseTime {
            name: "Optimize",
            ms: prof.optimize_ms,
        },
        PhaseTime {
            name: "Verify",
            ms: prof.verify_ms,
        },
        PhaseTime {
            name: "Save (node_cache)",
            ms: prof.save_ms,
        },
    ];
    phases.retain(|p| p.ms > 0);
    let total: u64 = phases.iter().map(|p| p.ms).sum::<u64>().max(1);

    section("Cached Build Profile");
    kv("project id", &pid);
    kv("build", prof.build_id);
    kv("files", prof.files);
    kv("changed", prof.changed_files);
    kv("restored (node_cache)", prof.restored_designs);
    kv(
        "peak mem",
        crate::human_bytes(prof.peak_mem_kb.saturating_mul(1024)),
    );
    println!();
    println!("  {:<14} {:>10} {:>8}", "Phase", "Time (ms)", "%");
    println!("  ─────────────────────────────────");
    for p in &phases {
        let bar_len = ((p.ms as f64 / total as f64) * 40.0) as usize;
        let bar = "█".repeat(bar_len);
        println!(
            "  {:<14} {:>10} {:>7.1}%  {}",
            p.name,
            p.ms,
            (p.ms as f64 / total as f64) * 100.0,
            bar
        );
    }
    println!("  ─────────────────────────────────");
    println!("  {:<14} {:>10}", "Total", total);

    if let Some(bn) = phases.iter().max_by_key(|p| p.ms) {
        section("Bottleneck (dari build terakhir)");
        kv("phase", bn.name);
        kv(
            "time",
            format!(
                "{} ms ({:.1}%)",
                bn.ms,
                (bn.ms as f64 / total as f64) * 100.0
            ),
        );
        kv("hint", bottleneck_hint(bn.name));
    }

    Ok(())
}
