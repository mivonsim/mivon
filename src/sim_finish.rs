//! Fase pelaporan pasca-simulasi — satu implementasi untuk kedua jalur.
//!
//! `run` (pipeline legacy) dan `run_fast` (CompileSession) dulu masing-masing
//! punya salinan ~130 baris blok pelaporan. Salinan itu sudah menyimpang: jalur
//! cepat tidak mencetak `--mem`, tidak menulis UCIS (`--coverage-ucis`), dan
//! tidak menjalankan analisis CDC (`--cdc-report`) padahal justru jalur itulah
//! yang dipakai untuk `--filelist` dan file besar. Modul ini menyatukannya
//! supaya kedua jalur melaporkan hasil dengan cara yang sama.

use crate::cli::Cli;
use crate::report_glue;
use mivon_api::debugger::Debugger;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;

/// Evaluasi ambang branch coverage (CI gate). Dipanggil SETELAH simulasi
/// selesai, terlepas dari `--coverage-ucdb` — agar config `coverage.branch_threshold`
/// benar-benar berlaku (sebelumnya hanya dicek saat menyimpan UCDB, sehingga
/// gate CI dari config tidak pernah aktif).
fn check_coverage_threshold(
    engine: &mivon_api::simulator::SimulationEngine,
    threshold: f64,
    quiet: bool,
) -> Result<(), SimError> {
    let stats = engine.coverage_stats();
    let branch_pct = stats.get("branch_percent").copied().unwrap_or(0.0);
    if branch_pct < threshold {
        let msg = format!(
            "COVERAGE FAILED: branch coverage {:.1}% < threshold {:.1}%",
            branch_pct, threshold
        );
        eprintln!("warning: {}", msg);
        return Err(SimError::with_diag(DiagCode::SimulationError, msg));
    } else if !quiet {
        println!(
            "Coverage threshold: {:.1}% >= {:.1}% ✅",
            branch_pct, threshold
        );
    }
    Ok(())
}

/// Laporkan seluruh hasil simulasi (signal/timeline, VCD, coverage, checkpoint,
/// CDC), lalu ubah `$fatal` menjadi exit code non-zero.
pub(crate) fn finish_simulation(
    cli: &Cli,
    debugger: &mut Debugger,
    vcd_path: &str,
) -> Result<(), SimError> {
    // Print signals
    if cli.print_state {
        println!("\n{}", debugger.print_all_signals());
    }
    for name in &cli.print_signal {
        println!("  {}", debugger.print_signal(name));
    }
    for name in &cli.timeline {
        println!("\n{}", debugger.timeline(name, cli.timeline_len));
    }
    if cli.mem.len() == 2 {
        if let (Ok(addr), Ok(len)) = (
            u64::from_str_radix(
                cli.mem[0].trim_start_matches("0x").trim_start_matches("0X"),
                16,
            ),
            cli.mem[1].parse::<usize>(),
        ) {
            println!("\n{}", debugger.memory_inspect(addr, len));
        }
    }

    if !cli.quiet {
        println!("VCD waveform written to '{}'", vcd_path);
    }

    // ── CSV close ──
    let _ = debugger.engine.close_csv();

    report_glue::write_wave_reports(cli, &debugger.engine, vcd_path);

    // UCIS coverage export
    if let Some(ref ucis_path) = cli.coverage_ucis {
        let path = if ucis_path.is_empty() {
            format!("{}.ucis.xml", debugger.engine.design.top.name)
        } else {
            ucis_path.clone()
        };
        match debugger.engine.export_coverage_ucis(&path) {
            Ok(()) => {
                if !cli.quiet {
                    println!("UCIS coverage written to '{}'", path);
                }
            }
            Err(e) => eprintln!("UCIS export failed: {}", e),
        }
    }

    // Signal history stats
    if !cli.quiet && cli.signal_history_spill.is_some() {
        let stats = debugger.engine.signal_history.stats();
        println!(
            "Signal history: {} mem entries, {} spilled to disk",
            stats.total_memory_entries, stats.total_spilled_entries
        );
    }

    // Save coverage database if requested
    if let Some(ref covdb_path) = cli.coverage_ucdb {
        let mut covdb = mivon_api::simulator::coverage_db::CoverageDatabase::with_path(covdb_path);
        covdb.merge_from_engine(&debugger.engine);
        if let Err(e) = covdb.save() {
            eprintln!("warning: coverage DB save failed: {}", e);
        } else if !cli.quiet {
            println!("Coverage database saved to '{}'", covdb_path);
        }
    }

    // Coverage threshold check (CI gate) — berlaku walau tanpa --coverage-ucdb,
    // sehingga config coverage.branch_threshold benar-benar dievaluasi.
    if let Some(threshold) = cli.coverage_threshold {
        check_coverage_threshold(&debugger.engine, threshold, cli.quiet)?;
    }

    // Export HTML coverage report
    if let Some(ref html_path) = cli.coverage_html {
        // String kosong → nama top (konsisten dengan UCIS).
        let path = if html_path.is_empty() {
            format!("{}.coverage.html", debugger.engine.design.top.name)
        } else {
            html_path.clone()
        };
        let mut covdb = mivon_api::simulator::coverage_db::CoverageDatabase::new();
        covdb.merge_from_engine(&debugger.engine);
        if let Err(e) = covdb.export_html(&path) {
            eprintln!("warning: HTML coverage report failed: {}", e);
        } else if !cli.quiet {
            println!("HTML coverage report written to '{}'", path);
        }
    }

    // Save checkpoint to file if requested
    if let Some(save_path) = &cli.save {
        let _ = debugger.engine.signal_history.flush();
        let path = std::path::Path::new(save_path);
        debugger.engine.save_checkpoint(path).map_err(|e| {
            SimError::with_diag(DiagCode::IoError, format!("checkpoint save failed: {}", e))
        })?;
        if !cli.quiet {
            println!("Checkpoint saved to '{}'", save_path);
        }
    }

    // CDC (Clock-Domain Crossing) analysis
    if let Some(ref cdc_path) = cli.cdc_report {
        if !cli.quiet {
            println!("Running CDC analysis...");
        }
        let cdc_analysis = mivon_api::scheduler::cdc::CdcAnalysis::analyze(&debugger.engine.design);

        // Print summary to console
        if !cli.quiet {
            println!(
                "CDC: {} crossings — {} unsynchronized, {} single-flop, {} OK (2+ flops)",
                cdc_analysis.total_crossings,
                cdc_analysis.unsynchronized_count,
                cdc_analysis.single_flop_count,
                cdc_analysis.sync_ok_count,
            );
            if cdc_analysis.unsynchronized_count > 0 {
                eprintln!(
                    "  ⚠️ {} unsynchronized crossing(s) detected — check CDC report for details",
                    cdc_analysis.unsynchronized_count
                );
            }
        }

        // Write detailed report
        let path = if cdc_path.is_empty() {
            format!(
                "{}_cdc_report.txt",
                debugger.engine.design.top.name.as_str()
            )
        } else {
            cdc_path.clone()
        };
        match cdc_analysis.write_report(&path) {
            Ok(()) => {
                if !cli.quiet {
                    println!("CDC report written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: CDC report write failed: {}", e),
        }
    }

    // F15: $fatal menghentikan sim dengan kegagalan → exit code non-zero.
    if debugger.engine.sev_fatal_count > 0 {
        return Err(SimError::with_diag(
            DiagCode::AssertionFailed,
            format!(
                "$fatal: simulasi dihentikan ({})",
                debugger.engine.sev_fatal_count
            ),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_coverage_threshold;

    /// Gate CI: branch coverage 0% harus GAGAL untuk threshold > 0 dan LOLOS
    /// untuk threshold 0. Dulu fungsi ini hanya dipanggil dari jalur legacy;
    /// sekarang dipakai bersama oleh `run` dan `run_fast`.
    #[test]
    fn threshold_gate_rejects_unmet_target() {
        let ir = mivon_api::compile_str_quiet("module top; initial begin end endmodule")
            .expect("desain minimal harus compile");
        let engine = mivon_api::simulator::SimulationEngine::new(ir, 100);

        let err = check_coverage_threshold(&engine, 100.0, true)
            .expect_err("coverage 0% harus gagal untuk threshold 100%");
        match err {
            mivon_core::error::SimError::Diagnostic(d) => {
                assert_eq!(d.code, mivon_core::diagnostics::DiagCode::SimulationError);
            }
            other => panic!("harus Diagnostic, dapat {:?}", other),
        }

        check_coverage_threshold(&engine, 0.0, true).expect("threshold 0% harus lolos");
    }
}
