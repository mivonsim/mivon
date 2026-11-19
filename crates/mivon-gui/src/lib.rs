//! GUI terminal (ratatui TUI) — Engineering Control Center untuk RTL.
//!
//! Dipanggil via `mivon gui` (subcommand CLI), tidak ada biner terpisah,
//! tidak butuh `--features gui`, tidak butuh GPU. Semua operasi berat
//! (compile/sim) dijalankan di worker thread; hasil dipoll per-frame lewat
//! channel. Renderer: terminal (crossterm + ratatui).

use std::io;

pub mod app;
pub mod app_cmds;
pub mod backend;
pub mod diagnostics;
pub mod lsp_client;
pub mod panels;
pub mod regression;
pub mod syntax;
pub mod state;
pub mod workspace;

pub use app::MivonApp;

/// Jalankan aplikasi GUI terminal (blocking sampai keluar).
///
/// `targets`: file/direktori yang dibuka saat startup (opsional).
/// `max_time`: batas waktu simulasi default (ns).
/// `top`: top module name (opsional).
pub fn run_with_args(
    targets: Vec<std::path::PathBuf>,
    max_time: Option<u64>,
    top: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    diagnostics::install_panic_hook();
    diagnostics::log("mivon-gui (ratatui) start");

    // Rayon global pool dengan stack BESAR (16MB/worker) — cermin CLI.
    rayon::ThreadPoolBuilder::new()
        .stack_size(16 * 1024 * 1024)
        .thread_name(|i| format!("rayon-parse-{}", i))
        .build_global()
        .ok();
    diagnostics::log("rayon pool 16MB siap");

    // Setup terminal crossterm + ratatui.
    let mut terminal = setup_terminal()?;

    let mut app = MivonApp::new();

    // Buka project bila targets diberikan.
    if !targets.is_empty() {
        let dir = targets[0].clone();
        if dir.is_dir() {
            app.open_project_dir(&dir);
        } else {
            // File tunggal: buka direktori induk.
            if let Some(parent) = dir.parent() {
                app.open_project_dir(parent);
            }
            // Buka file tersebut sebagai active file.
            app.state.open_file(dir);
        }
        if let Some(mt) = max_time {
            app.state.max_time = mt;
        }
        if let Some(t) = top {
            app.state.pending_top = Some(t.to_string());
        }
    } else {
        // Restore workspace terakhir (last workspace).
        app.restore_workspace();
    }

    let result = app.run(&mut terminal);

    // Restore terminal.
    restore_terminal(terminal)?;
    result
}

/// Jalankan GUI dengan argumen default (kosong — restore workspace terakhir).
pub fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_with_args(Vec::new(), None, None)
}

fn setup_terminal() -> Result<
    ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
    use crossterm::ExecutableCommand;
    io::stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let backend = ratatui::backend::CrosstermBackend::new(io::stdout());
    let terminal = ratatui::Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(
    mut terminal: ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    use crossterm::ExecutableCommand;
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    Ok(())
}
