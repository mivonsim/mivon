//! Adaptor subcommand `mivon gui` — launch TUI (ratatui).
//!
//! Hanya menerjemahkan `cli::GuiArgs` menjadi konfigurasi GUI lalu panggil
//! `mivon_gui::run_with_args()`. Satu tanggung jawab: wiring CLI → GUI.

use crate::cli::GuiArgs;
use mivon_core::error::SimError;

pub(crate) fn dispatch_gui(a: &GuiArgs) -> ! {
    let targets: Vec<std::path::PathBuf> = a.targets.iter().map(std::path::PathBuf::from).collect();
    match mivon_gui::run_with_args(targets, a.max_time, a.top.as_deref()) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("GUI error: {}", e);
            std::process::exit(1);
        }
    }
}
