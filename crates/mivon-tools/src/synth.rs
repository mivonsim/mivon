//! `synth` (alias `msynth`) — Mivon Synthesis (SYNTHESIS.md §15).
//!
//! RTL → SIR → netlist gate-level:
//! - `--check-only`: analisis sintesizability (SYN-1..9) tanpa netlist.
//! - `--dump-sir`: dump SIR node-based (fase RTL→SIR, `mivon-sir`).
//! - default: SYN check + inferensi netlist pra-map + emit `.mvnet` + report
//!   utilisasi (estimasi S1).

use std::path::PathBuf;

use crate::{kv, section};
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::ElaborateMode;
use mivon_synth::prelude::*;
use mivon_synth::DeviceKind;

/// Opsi `synth`.
pub struct SynthArgs<'a> {
    pub targets: &'a [String],
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub top: Option<&'a str>,
    /// Output prefix (default: nama top). Menghasilkan `<prefix>.mvnet` dst.
    pub output: Option<String>,
    /// Hanya SYN subset check.
    pub check_only: bool,
    /// Device target: "fpga-x7" (default) atau "generic".
    pub device: String,
    /// Preset pipeline: generic | fpga | asic | custom.
    pub preset: String,
    /// Emisi `.mvnet`.
    pub emit_mvnet: bool,
    /// Dump SIR (node-based) ke stdout.
    pub dump_sir: bool,
    /// Dump SIR setelah pass optimizer.
    pub dump_sir_opt: bool,
    /// Dump netlist generik (Verilog + .mvnet) ke stdout.
    pub dump_netlist: bool,
    /// Emisi netlist ke file: <prefix>.netlist.v + .json + .mvnet.
    pub emit_netlist: bool,
    /// Tech mapping (phase 4): LUT cut + AIG dekomposisi + carry chain →
    /// <prefix>.tech.v/.json/.mvnet + report LUT/CARRY4/FF.
    pub tech_map: bool,
    /// Tulis report utilisasi ke file (opsional; tanpa ini ke stdout).
    pub report_util: Option<String>,
    /// File constraint `.mcs` (phase 5) — dipakai `--timing`.
    pub constraint: Option<String>,
    /// Static timing + area analysis (phase 5): WNS/TNS/critical path + area
    /// → <prefix>.timing.rpt / <prefix>.area.rpt.
    pub timing: bool,
    /// Laporan FSM extraction (COMP-11 tahap 2): deteksi state register +
    /// transisi dari proses Sequential.
    pub fsm_report: bool,
    pub quiet: bool,
}

/// Jalankan `synth`.
pub fn run(args: &SynthArgs) -> Result<(), SimError> {
    let (session, _design, ir) = crate::open_elaborated(
        args.targets,
        args.incdirs,
        args.defines,
        args.top,
        ElaborateMode::AnalysisRecovery,
    )?;
    let top_name = ir.top.name.as_str().to_string();

    // ── Device ──
    let device = match args.device.as_str() {
        "fpga-x7" => DeviceKind::FpgaX7,
        "generic" => DeviceKind::Generic,
        other => {
            return Err(SimError::with_diag(
                mivon_core::diagnostics::DiagCode::InvalidSyntax,
                format!(
                    "device '{}' tidak dikenal — pakai 'fpga-x7' (default) atau 'generic' (ASIC menyusul S4)",
                    other
                ),
            ));
        }
    };

    // ── SYN check ──
    let check = synth_check(&ir);
    let syn_report = render_syn_report(&check);
    if !args.quiet {
        print!("{}", syn_report);
    }

    // ── FSM extraction report (COMP-11 tahap 2) ──
    if args.fsm_report {
        let fsms = mivon_synth::fsm::extract_fsms(&ir);
        let report = mivon_synth::fsm::render_fsm_report(&fsms);
        if !args.quiet {
            print!("{}", report);
        }
    }

    // ── SIR: lowering + optimizer (Phase 2, SYNTHESIS.md §4/§6) ──
    let need_sir = !args.check_only
        || args.dump_sir
        || args.dump_sir_opt
        || args.dump_netlist
        || args.emit_netlist
        || args.tech_map;
    if need_sir {
        let sir = mivon_sir::lower(&ir);
        if args.dump_sir && !args.quiet {
            section("SIR (sebelum optimasi)");
            print!("{}", mivon_sir::render_sir(&sir.module));
            if !sir.skipped.is_empty() {
                println!(
                    "  [skipped {} konstruk yang belum didukung SIR fase 1]",
                    sir.skipped.len()
                );
                for s in &sir.skipped {
                    println!("    - {s}");
                }
            }
            println!();
        }
        // Pass manager + preset.
        let mut pipeline = mivon_synth::SynthPipeline::with_preset(&args.preset)?;
        let (sir_opt, results) = pipeline.run(sir.module)?;
        if args.dump_sir_opt && !args.quiet {
            section("SIR (setelah optimasi)");
            print!("{}", mivon_sir::render_sir(&sir_opt));
            println!();
        }
        section(&format!("Optimization (preset: {})", args.preset));
        for r in &results {
            kv(
                r.name,
                format!(
                    "{} → {} node ({} rewrite)",
                    r.nodes_before, r.nodes_after, r.changed
                ),
            );
        }

        // ── Netlist: SIR → generic netlist (Phase 3, SYNTHESIS.md §11/§13) ──
        if args.dump_netlist || args.emit_netlist {
            let nl = mivon_netlist::lower_module(&sir_opt);
            section("Netlist (generic, SIR → gate-level)");
            print!("{}", mivon_netlist::emit_summary(&nl));
            let check = mivon_netlist::verify_dag(&nl);
            if !check.ok {
                if !check.double_drivers.is_empty() {
                    println!("  ⚠ double driver: {}", check.double_drivers.join(", "));
                }
                if !check.floating.is_empty() {
                    println!("  ⚠ floating net: {}", check.floating.join(", "));
                }
            }
            if args.dump_netlist && !args.quiet {
                section("netlist.v");
                print!("{}", mivon_netlist::emit_verilog(&nl));
                section(".mvnet");
                print!("{}", mivon_netlist::emit_mvnet(&nl));
            }
            if args.emit_netlist {
                let prefix = args.output.clone().unwrap_or_else(|| top_name.clone());
                let v = mivon_netlist::emit_verilog(&nl);
                let json = mivon_netlist::emit_json(&nl);
                let mvnet = mivon_netlist::emit_mvnet(&nl);
                for (suffix, content) in [
                    ("netlist.v", v),
                    ("netlist.json", json),
                    ("netlist.mvnet", mvnet),
                ] {
                    let path = PathBuf::from(format!("{}.{}", prefix, suffix));
                    std::fs::write(&path, content).map_err(|e| {
                        SimError::with_diag(
                            mivon_core::diagnostics::DiagCode::IoError,
                            format!("{}: {}", path.display(), e),
                        )
                    })?;
                    if !args.quiet {
                        println!("  netlist → {}", path.display());
                    }
                }
            }
        }

        // ── Tech mapping (Phase 4, SYNTHESIS.md §5/§12) ──
        // LUT cut (≤K input, init nyata) + AIG dekomposisi (>K input) +
        // carry chain (CARRY4) + FF per-bit. Emisi <prefix>.tech.v/.json/.mvnet
        // + report LUT/CARRY4/FF. `--device` memilih arsitektur (generic/fpga-x7).
        let mut tech_netlist: Option<mivon_netlist::Netlist> = None;
        if args.tech_map {
            let arch = mivon_tech::arch_for(match args.device.as_str() {
                "fpga-x7" => "fpga",
                other => other,
            })
            .expect("device generic/fpga punya back-end");
            let res = mivon_synth::tech_map(&sir_opt, arch.as_ref());
            section(&format!("Tech Mapping (phase 4 — {})", arch.name()));
            for s in &res.skipped {
                println!("  [skipped] {s}");
            }
            kv("LUT", res.lut_count.to_string());
            kv("CARRY4", res.carry4_count.to_string());
            kv("FF", res.ff_count.to_string());
            let dag = mivon_netlist::verify_dag(&res.netlist);
            kv("DAG", if dag.ok { "ok" } else { "violation!" });
            let prefix = args.output.clone().unwrap_or_else(|| top_name.clone());
            for (suffix, content) in [
                ("tech.v", mivon_netlist::emit_verilog(&res.netlist)),
                ("tech.json", mivon_netlist::emit_json(&res.netlist)),
                ("tech.mvnet", mivon_netlist::emit_mvnet(&res.netlist)),
            ] {
                let path = PathBuf::from(format!("{}.{}", prefix, suffix));
                std::fs::write(&path, content).map_err(|e| {
                    SimError::with_diag(
                        mivon_core::diagnostics::DiagCode::IoError,
                        format!("{}: {}", path.display(), e),
                    )
                })?;
                if !args.quiet {
                    println!("  tech netlist → {}", path.display());
                }
            }
            tech_netlist = Some(res.netlist);
        }

        // ── Timing & Area (Phase 5, SYNTHESIS.md §15-17) ──
        // STA atas netlist: arrival/required/slack → WNS/TNS + critical path
        // + estimasi area. Netlist: tech (bila `--tech-map`) else generic.
        // Constraint `.mcs` opsional (default: period 10ns, delay 0).
        if args.timing {
            let nl = match &tech_netlist {
                Some(n) => n,
                None => {
                    let g = mivon_netlist::lower_module(&sir_opt);
                    tech_netlist.insert(g)
                }
            };
            let (constraint, cname) = match &args.constraint {
                Some(p) => {
                    let path = PathBuf::from(p);
                    let c = mivon_timing::load_constraints(&path).map_err(|e| {
                        SimError::with_diag(
                            mivon_core::diagnostics::DiagCode::IoError,
                            format!("{}: {}", path.display(), e),
                        )
                    })?;
                    (c, p.clone())
                }
                None => (
                    mivon_timing::Constraint::default(),
                    "default (10ns)".to_string(),
                ),
            };
            let rep =
                mivon_timing::analyze(nl, &constraint, &mivon_timing::TimingOptions::default());
            let area = mivon_timing::estimate_area(nl);
            let timing_rpt = mivon_timing::render_timing_report(&rep, &cname);
            let area_rpt = mivon_timing::render_area_report(&area);
            section("Timing (phase 5 — STA)");
            print!("{}", timing_rpt);
            section("Area (phase 5 — estimate)");
            print!("{}", area_rpt);
            let prefix = args.output.clone().unwrap_or_else(|| top_name.clone());
            for (suffix, content) in [("timing.rpt", timing_rpt), ("area.rpt", area_rpt)] {
                let path = PathBuf::from(format!("{}.{}", prefix, suffix));
                std::fs::write(&path, content).map_err(|e| {
                    SimError::with_diag(
                        mivon_core::diagnostics::DiagCode::IoError,
                        format!("{}: {}", path.display(), e),
                    )
                })?;
                if !args.quiet {
                    println!("  report → {}", path.display());
                }
            }
        }
    }

    if args.check_only {
        // `--check-only`: berhenti di sini; exit non-zero bila ada SYN error.
        if check.error_count() > 0 {
            let first = mivon_synth::report::first_error(&check).unwrap_or_default();
            return Err(SimError::with_diag(
                mivon_core::diagnostics::DiagCode::InvalidSyntax,
                format!(
                    "synthesis check FAILED: {} error(s) — {}",
                    check.error_count(),
                    first
                ),
            ));
        }
        println!(
            "\n✅ synthesis check OK — design sintesizable (skor {:.1}/100)",
            check.overall_score()
        );
        return Ok(());
    }

    // ── Inferensi netlist ──
    let opts = mivon_synth::SynthOpts { device };
    let out = synthesize(&ir, &opts);
    let nl = &out.netlist;

    section("Synthesis Result");
    print!("{}", emit_summary(nl));
    kv("check score", format!("{:.1}/100", check.overall_score()));
    kv("elab time", format!("{} µs", session.timing.elab_us));

    // ── Output ──
    let prefix = args.output.clone().unwrap_or_else(|| top_name.to_string());
    if args.emit_mvnet {
        let mvnet = emit_mvnet(nl, mivon_synth::VERSION);
        let path = PathBuf::from(format!("{}.mvnet", prefix));
        std::fs::write(&path, mvnet).map_err(|e| {
            SimError::with_diag(
                mivon_core::diagnostics::DiagCode::IoError,
                format!("{}: {}", path.display(), e),
            )
        })?;
        if !args.quiet {
            println!("  .mvnet → {}", path.display());
        }
    }

    // ── Report utilisasi ──
    let cap = match nl.device {
        DeviceKind::FpgaX7 => mivon_synth::DeviceCapacity::fpga_x7(),
        DeviceKind::Generic => mivon_synth::DeviceCapacity::generic(),
    };
    let util = render_util_report(nl, &cap);
    if let Some(path_str) = &args.report_util {
        let path = PathBuf::from(path_str);
        std::fs::write(&path, &util).map_err(|e| {
            SimError::with_diag(
                mivon_core::diagnostics::DiagCode::IoError,
                format!("{}: {}", path.display(), e),
            )
        })?;
        if !args.quiet {
            println!("  util report → {}", path.display());
        }
    } else if !args.quiet {
        print!("{}", util);
    }

    Ok(())
}
