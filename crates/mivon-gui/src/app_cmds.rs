//! Fungsi trigger aksi GUI (dipindah dari app.rs agar app.rs ringkas).
//! Semua memutar state GuiState dan memanggil backend via channel.

use std::sync::atomic::Ordering;

use super::backend::{spawn_compile, spawn_sim, spawn_term, DebugConfig};
use super::state::GuiState;

/// Buka folder project (paksa direktori kerja saat ini sebagai project).
pub fn trigger_open_project(state: &mut GuiState) {
    let dir = std::env::current_dir().unwrap_or_default();
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Project".to_string());
    let name_clone = name.clone();
    let dir_clone = dir.clone();
    let files = super::backend::scan_tree(&dir);
    state.project_name = name;
    state.project_root = Some(dir_clone.clone());
    state.files = files;
    state.compile_info = None;
    state.signals.clear();
    state.diagnostics.clear();
    state.log(format!("📂 Project dibuka: {}", name_clone));
    super::workspace::push_recent(&dir_clone);
    super::workspace::save_workspace(state);
}

/// Compile + elaborate semua file .sv/.svh di pohon project.
pub fn trigger_compile(state: &mut GuiState) {
    if state.project_root.is_none() {
        state.log("⚠ Buka project dulu (Ctrl+O)");
        return;
    }
    let paths = state.collect_sv_files();
    if paths.is_empty() {
        state.log("⚠ Tidak ada file .sv/.svh");
        return;
    }
    state.log(format!("🔨 Compile {} file...", paths.len()));
    state.diagnostics.clear();
    let tx = state.tx.clone();
    let project_root = state.project_root.clone();
    spawn_compile(tx, paths, project_root);
}

/// Jalankan simulasi pada design ter-compile.
pub fn trigger_run(state: &mut GuiState) {
    // Resume dari Pause: lepas pause_flag pada engine yang sama yang sedang
    // menunggu di worker. Jangan spawn worker baru (race dengan yang lama).
    if state.is_paused {
        state.pause_flag.store(false, Ordering::Relaxed);
        state.is_running = true;
        state.is_paused = false;
        state.log("▶ Resume simulasi...");
        return;
    }
    if state.is_running {
        return;
    }
    // Pastikan tidak ada sisa flag pause/cancel dari run sebelumnya.
    state.pause_flag.store(false, Ordering::Relaxed);
    state.cancel_flag.store(false, Ordering::Relaxed);

    let Some(design) = state.design.clone() else {
        state.log("⚠ Compile dulu sebelum run");
        return;
    };
    state.prev_waveform = state.waveform.clone();
    if !state.prev_waveform.is_empty() {
        state.log(format!(
            "⟲ Snapshot {} signal tersimpan (baseline compare)",
            state.prev_waveform.len()
        ));
    }
    state.log(format!(
        "▶ Simulasi (T={})...", state.max_time));
    state.is_running = true;
    state.is_paused = false;
    let max_time = state.max_time;
    let tx = state.tx.clone();
    let cancel = state.cancel_flag.clone();
    let pause = state.pause_flag.clone();
    let debug = DebugConfig {
        breakpoints: state.breakpoints.clone(),
        watchpoints: state.watchpoints.clone(),
        enabled: state.debug_mode,
    };
    spawn_sim(tx, design, max_time, cancel, pause, debug);
}

/// Jeda simulasi yang berjalan (Pause). Engine menetukan `paused = true` dan
/// worker menunggu hingga `pause_flag` false (Resume via F5/F8).
pub fn trigger_pause(state: &mut GuiState) {
    if !state.is_running || state.is_paused {
        return;
    }
    state.pause_flag.store(true, Ordering::Relaxed);
    state.is_paused = true;
    state.log("⏸ Simulasi dijeda (Pause). Tekan F5/F8 untuk resume.");
}

/// Hentikan simulasi yang berjalan.
pub fn trigger_stop(state: &mut GuiState) {
    if !state.is_running {
        return;
    }
    state.cancel_flag.store(true, Ordering::Relaxed);
    state.pause_flag.store(false, Ordering::Relaxed); // lepas blokir Pause bila aktif
    state.is_running = false;
    state.is_paused = false;
}

/// Jalankan perintah terminal (input bar). Perintah debugger (`bp`/`wp`/`watch`/
/// `debug`) dialihkan ke state (tidak spawn shell) agar breakpoint nyata.
pub fn trigger_terminal(state: &mut GuiState, cmd: String) {
    if cmd.trim().is_empty() {
        return;
    }
    let lower = cmd.trim().to_lowercase();
    if lower == "clear" {
        state.clear_debug();
        return;
    }
    if lower == "debug on" || lower == "debug off" {
        state.debug_mode = lower.ends_with("on");
        state.log(format!("🐞 debug mode {}", if state.debug_mode { "ON" } else { "OFF" }));
        return;
    }
    if lower.starts_with("bp ") || lower.starts_with("break ") {
        let spec = cmd.split_once(' ').map(|(_, r)| r).unwrap_or("").to_string();
        state.add_breakpoint(&spec);
        return;
    }
    if lower.starts_with("wp ") || lower.starts_with("watch ") {
        let name = cmd.split_once(' ').map(|(_, r)| r).unwrap_or("").to_string();
        state.add_watchpoint(&name);
        return;
    }
    state.term_history.push(cmd.clone());
    state.term_hist_idx = usize::MAX;
    let cwd = state.project_root.clone();
    spawn_term(state.tx.clone(), cmd, cwd);
}

/// Generate .sv/.svh dari file .mv/.mvh aktif.
pub fn trigger_generate(state: &mut GuiState) {
    let Some(idx) = state.active_file else {
        state.log("⚠ Generate: tidak ada file aktif");
        return;
    };
    let Some(of) = state.open_files.get(idx) else {
        return;
    };
    let ext = of.path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "mv" && ext != "mvh" {
        state.log("⚠ Generate hanya untuk file .mv/.mvh (Mivon HDL)");
        return;
    }
    let is_header = ext == "mvh";
    let content = of.content.clone();
    let base = of
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("design")
        .to_string();
    let sv_path = of.path.with_extension("sv");
    let svh_path = of.path.with_extension("svh");
    state.log(format!(
        "⚙ Generate {} → {} ...",
        of.name,
        if is_header { ".svh" } else { ".sv/.svh" }
    ));
    let tr = if is_header {
        mivon_mv::transpile_header(&content, &base)
    } else {
        mivon_mv::transpile(&content, &base)
    };
    match tr {
        Ok(r) => {
            let sv_lines = r.sv.lines().count();
            let svh_lines = r.svh.lines().count();
            let wsv = if is_header || r.sv.is_empty() {
                Ok(())
            } else {
                std::fs::write(&sv_path, &r.sv)
            };
            let wsvh = if r.svh.is_empty() {
                Ok(())
            } else {
                std::fs::write(&svh_path, &r.svh)
            };
            if let Err(e) = wsv {
                state.log(format!(
                    "❌ Generate: gagal menulis {}: {}",
                    sv_path.display(),
                    e
                ));
                return;
            }
            if let Err(e) = wsvh {
                state.log(format!(
                    "❌ Generate: gagal menulis {}: {}",
                    svh_path.display(),
                    e
                ));
                return;
            }
            if is_header {
                if r.svh.is_empty() {
                    state.log("⚠ Generate: .mvh tidak punya output");
                } else {
                    state.log(format!(
                        "✅ Generate: {} ({} baris)",
                        svh_path.display(),
                        svh_lines
                    ));
                    state.open_file(svh_path);
                }
                return;
            }
            if r.sv.is_empty() {
                state.log("⚠ Generate: tidak ada module — .sv tidak ditulis");
            } else {
                state.log(format!(
                    "✅ Generate: {} ({} baris)",
                    sv_path.display(),
                    sv_lines
                ));
            }
            if !r.svh.is_empty() {
                state.log(format!("   + {} ({} baris)", svh_path.display(), svh_lines));
            }
            if !r.sv.is_empty() {
                state.open_file(sv_path);
            }
        }
        Err(e) => {
            state.log(format!("❌ Generate: {}", e.format()));
        }
    }
}
