//! Konfigurasi pipeline yang berasal dari CLI (mode elaborasi, root MICD).
//!
//! Fungsi murni atas `Cli` — dipisah dari main.rs agar bisa diuji tanpa
//! menjalankan simulasi.

use crate::cli::Cli;
use mivon_elaboration::elaborator::ElaborateMode;

/// Pilih ElaborateMode: config `[elaborate] mode` menang, lalu `--top` eksplisit
/// (StrictSimulation), lalu AnalysisRecovery (tanpa top — analisis saja).
/// Nilai config yang tidak dikenal memicu warning lalu fallback ke logika CLI.
pub(crate) fn pick_elab_mode(cli: &Cli) -> ElaborateMode {
    match cli.config_elab_mode.as_deref() {
        Some(m) if m.eq_ignore_ascii_case("analysisrecovery") => ElaborateMode::AnalysisRecovery,
        Some(m) if m.eq_ignore_ascii_case("strictsimulation") => ElaborateMode::StrictSimulation,
        Some(m) => {
            eprintln!("warning: elaborate.mode '{}' tidak dikenal (pakai StrictSimulation / AnalysisRecovery) — memakai mode default", m);
            if cli.top.is_some() {
                ElaborateMode::StrictSimulation
            } else {
                ElaborateMode::AnalysisRecovery
            }
        }
        None => {
            if cli.top.is_some() {
                ElaborateMode::StrictSimulation
            } else {
                ElaborateMode::AnalysisRecovery
            }
        }
    }
}

/// Path root database MICD, dengan support `--target-dir`.
/// `--target-dir <path>` override path default (`.mivon/database`).
pub(crate) fn micd_root_for(cli: &Cli) -> std::path::PathBuf {
    use mivon_compiler::micd::MicdDatabase;
    if let Some(ref dir) = cli.target_dir {
        std::path::PathBuf::from(dir).join("database")
    } else {
        MicdDatabase::default_root()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(argv: &[&str]) -> Cli {
        Cli::try_parse_from(argv).expect("parse")
    }

    /// Config menang atas CLI; nama mode case-insensitive.
    #[test]
    fn elab_mode_config_wins() {
        let mut cli = parse(&["mivon", "a.sv", "--top", "top"]);
        cli.config_elab_mode = Some("analysisrecovery".to_string());
        assert_eq!(pick_elab_mode(&cli), ElaborateMode::AnalysisRecovery);
        cli.config_elab_mode = Some("StrictSimulation".to_string());
        assert_eq!(pick_elab_mode(&cli), ElaborateMode::StrictSimulation);
    }

    /// Tanpa config: --top → StrictSimulation, tanpa top → AnalysisRecovery.
    #[test]
    fn elab_mode_falls_back_to_top_presence() {
        let with_top = parse(&["mivon", "a.sv", "--top", "top"]);
        assert_eq!(pick_elab_mode(&with_top), ElaborateMode::StrictSimulation);
        let without_top = parse(&["mivon", "a.sv"]);
        assert_eq!(
            pick_elab_mode(&without_top),
            ElaborateMode::AnalysisRecovery
        );
    }

    /// --target-dir menambahkan segmen "database".
    #[test]
    fn micd_root_honours_target_dir() {
        let cli = parse(&["mivon", "a.sv", "--target-dir", "/tmp/mivon-target"]);
        assert_eq!(
            micd_root_for(&cli),
            std::path::PathBuf::from("/tmp/mivon-target/database")
        );
    }
}
