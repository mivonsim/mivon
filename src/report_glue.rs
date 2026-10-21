//! Laporan pasca-simulasi: signal statistics, GTKWave, HTML viewer.
//!
//! Dipakai KEDUA jalur eksekusi (legacy `run` dan `run_fast`) supaya
//! blok ~50 baris yang dulu diduplikasi tidak melenceng lagi.

use crate::cli::Cli;
use mivon_api::simulator::SimulationEngine;

/// Path laporan: flag kosong → `<top>.<ext>`, selain itu pakai nilai flag.
pub(crate) fn report_path(flag: &str, ext: &str, top: &str) -> String {
    if flag.is_empty() {
        format!("{}.{}", top, ext)
    } else {
        flag.to_string()
    }
}

/// Tulis laporan waveform opsional (stats/GTKWave/HTML) sesuai flag CLI.
pub(crate) fn write_wave_reports(cli: &Cli, engine: &SimulationEngine, vcd_path: &str) {
    // ── Signal statistics ──
    if let Some(ref stats_path) = cli.signal_stats {
        let path = report_path(stats_path, "stats.txt", engine.design.top.name.as_str());
        if let Some(ref stats) = engine.signal_stats {
            if let Err(e) = stats.write_to_file(&path) {
                eprintln!("warning: signal stats write failed: {}", e);
            } else if !cli.quiet {
                println!("Signal statistics written to '{}'", path);
            }
        }
    }

    // ── GTKWave save file ──
    if let Some(ref gtkw_path) = cli.gtkw {
        let path = report_path(gtkw_path, "gtkw", engine.design.top.name.as_str());
        match mivon_api::waveform::save_gtkw(&path, vcd_path, &engine.design) {
            Ok(()) => {
                if !cli.quiet {
                    println!("GTKWave save file written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: GTKW save failed: {}", e),
        }
    }

    // ── HTML waveform viewer ──
    if let Some(ref html_path) = cli.waveform_html_viewer {
        let csv_ref = cli.waveform_csv.as_deref().unwrap_or("output.csv");
        let path = report_path(html_path, "html", engine.design.top.name.as_str());
        match mivon_api::waveform::save_html_viewer(&path, csv_ref, &engine.design) {
            Ok(()) => {
                if !cli.quiet {
                    println!("HTML waveform viewer written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: HTML viewer failed: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::report_path;

    /// Flag kosong → default `<top>.<ext>`; flag berisi → dipakai apa adanya.
    #[test]
    fn report_path_defaults_to_top_name() {
        assert_eq!(report_path("", "stats.txt", "counter"), "counter.stats.txt");
        assert_eq!(report_path("", "html", "counter"), "counter.html");
        assert_eq!(report_path("out.gtkw", "gtkw", "counter"), "out.gtkw");
    }
}
