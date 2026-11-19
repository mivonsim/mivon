//! Workspace persistence — pulihkan sesi terakhir saat aplikasi dibuka kembali
//! (prinsip desain: "Workspace terakhir dipulihkan saat aplikasi dibuka").
//!
//! Per proyek: `<root>/.mivon/workspace.json` menyimpan state UI (file terbuka,
//! tab aktif, toggles panel, zoom waveform, signal tersembunyi, max_time).
//! Pointer global `<config>/mivon/last_workspace.json` mencatat root proyek
//! terakhir agar bisa dibuka otomatis saat startup.
//!
//! Konvensi `.mivon/` konsisten dengan MICD (`<root>/.mivon/database`).
//! Semua I/O best-effort — kegagalan tidak menggagalkan aplikasi.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::state::{BottomTab, GuiState, SidebarTab};

/// State workspace yang dipulihkan (JSON — `.mivon/workspace.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// Path file terbuka — relatif terhadap root proyek (portabel).
    pub open_files: Vec<String>,
    pub active_file: Option<usize>,
    pub sidebar_tab: SidebarTab,
    pub bottom_tab: BottomTab,
    pub show_sidebar: bool,
    pub show_bottom: bool,
    /// Tinggi Bottom Panel (px) — ukuran panel bawah dipulihkan saat workspace
    /// di-restore. `#[serde(default)]` agar workspace lama (tanpa field ini)
    /// tetap bisa di-restore.
    #[serde(default)]
    pub bottom_height: f32,
    pub show_outline: bool,
    pub wave_zoom: f32,
    /// Signal yang disembunyikan di waveform.
    pub wave_hidden: Vec<String>,
    pub max_time: u64,
    /// Path file yang di-bookmark (relatif terhadap root — portabel).
    /// `#[serde(default)]` — workspace lama (tanpa field ini) tetap bisa
    /// di-restore; hanya bookmark yang hilang, bukan seluruh state.
    #[serde(default)]
    pub bookmarks: Vec<String>,
}

/// Pointer global ke root proyek terakhir (`<config>/mivon/last_workspace.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LastWorkspace {
    pub project_root: String,
}

/// Daftar proyek terakhir dibuka (`<config>/mivon/recent.json`) — paling baru
/// di depan, maks `RECENT_MAX`. Dipakai welcome screen "Recent projects".
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecentProjects {
    pub projects: Vec<String>,
}

/// Maksimal proyek yang diingat di daftar recent.
pub const RECENT_MAX: usize = 8;

/// Path daftar proyek recent.
fn recent_projects_path() -> PathBuf {
    config_dir().join("mivon").join("recent.json")
}

/// Baca daftar proyek recent (hanya yang masih ada di disk).
pub fn load_recents() -> Vec<PathBuf> {
    let Ok(json) = std::fs::read_to_string(recent_projects_path()) else {
        return Vec::new();
    };
    let Ok(rp) = serde_json::from_str::<RecentProjects>(&json) else {
        return Vec::new();
    };
    rp.projects
        .iter()
        .filter_map(|p| {
            let path = PathBuf::from(p);
            if path.is_dir() {
                Some(path)
            } else {
                None
            }
        })
        .collect()
}

/// Tambah proyek ke daftar recent (paling depan, dedupe, cap `RECENT_MAX`).
pub fn push_recent(root: &Path) {
    let mut list: Vec<String> = load_recents()
        .into_iter()
        .map(|p| p.display().to_string())
        .collect();
    list.retain(|p| p != &root.display().to_string());
    list.insert(0, root.display().to_string());
    list.truncate(RECENT_MAX);
    let rp = RecentProjects { projects: list };
    if let Ok(json) = serde_json::to_string_pretty(&rp) {
        let p = recent_projects_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, json);
    }
}

/// Direktori konfigurasi pengguna ($XDG_CONFIG_HOME atau ~/.config).
fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config");
    }
    std::env::temp_dir()
}

fn last_workspace_path() -> PathBuf {
    config_dir().join("mivon").join("last_workspace.json")
}

fn workspace_file(root: &Path) -> PathBuf {
    root.join(".mivon").join("workspace.json")
}

/// Simpan state workspace proyek saat ini (best-effort). Dipanggil saat proyek
/// dibuka dan saat window ditutup (frame terakhir via close_requested).
pub fn save_workspace(state: &GuiState) {
    let Some(root) = &state.project_root else {
        return;
    };
    let ws = WorkspaceState {
        open_files: state
            .open_files
            .iter()
            .map(|f| rel_or_abs(root, &f.path))
            .collect(),
        active_file: state.active_file,
        sidebar_tab: state.sidebar_tab,
        bottom_tab: state.bottom_tab,
        show_sidebar: state.show_sidebar,
        show_bottom: state.show_bottom,
        bottom_height: state.bottom_height,
        show_outline: state.show_outline,
        wave_zoom: state.wave_zoom,
        wave_hidden: state.wave_hidden.iter().cloned().collect(),
        max_time: state.max_time,
        bookmarks: state
            .bookmarks
            .iter()
            .map(|p| rel_or_abs(root, p))
            .collect(),
    };
    let json = match serde_json::to_string_pretty(&ws) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[workspace] serialize gagal: {}", e);
            return;
        }
    };
    let file = workspace_file(root);
    if std::fs::create_dir_all(root.join(".mivon")).is_err() {
        eprintln!("[workspace] gagal buat {}", root.join(".mivon").display());
        return;
    }
    if let Err(e) = std::fs::write(&file, json) {
        eprintln!("[workspace] gagal menulis {}: {}", file.display(), e);
        return;
    }

    // Pointer global ke proyek terakhir (untuk restore saat startup berikutnya)
    let last = LastWorkspace {
        project_root: root.display().to_string(),
    };
    if let Ok(j) = serde_json::to_string_pretty(&last) {
        let p = last_workspace_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, j);
    }
}

/// Pulihkan proyek terakhir — dipanggil sekali di frame pertama. Tidak ada
/// efek apa pun jika belum pernah ada workspace tersimpan.
pub fn restore_workspace(state: &mut GuiState) {
    let Some(root) = read_last_project_root() else {
        return; // belum pernah ada workspace — normal, bukan error
    };
    let ws_path = workspace_file(&root);
    // Pointer ada tapi file workspace korup/hilang (mis. write terpotong saat
    // crash) — beri tahu user supaya tidak bingung kenapa tidak dipulihkan.
    let json = match std::fs::read_to_string(&ws_path) {
        Ok(j) => j,
        Err(_) => {
            state.log(format!(
                "⚠ Workspace tidak dapat dipulihkan: {} tidak dapat dibaca",
                ws_path.display()
            ));
            return;
        }
    };
    let ws = match serde_json::from_str::<WorkspaceState>(&json) {
        Ok(w) => w,
        Err(_) => {
            state.log(format!(
                "⚠ Workspace tidak dapat dipulihkan: {} korup",
                ws_path.display()
            ));
            return;
        }
    };
    apply(state, root, ws);
}

/// Baca root proyek terakhir dari pointer global (valid bila direktori masih ada).
fn read_last_project_root() -> Option<PathBuf> {
    let json = std::fs::read_to_string(last_workspace_path()).ok()?;
    let last: LastWorkspace = serde_json::from_str(&json).ok()?;
    let root = PathBuf::from(&last.project_root);
    if root.is_dir() {
        Some(root)
    } else {
        None
    }
}

/// Terapkan state workspace ke GuiState: buka proyek, restore file + tab.
fn apply(state: &mut GuiState, root: PathBuf, ws: WorkspaceState) {
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Project".to_string());
    let files = crate::backend::scan_tree(&root);
    state.project_name = name;
    state.project_root = Some(root.clone());
    state.files = files;
    state.compile_info = None;
    state.signals.clear();
    state.diagnostics.clear();
    crate::diagnostics::log(format!(
        "restore: project '{}' — {} file di pohon",
        state.project_name,
        state.files.len()
    ));

    // Buka file tersimpan (path relatif di-resolve terhadap root)
    for rel in &ws.open_files {
        let p = PathBuf::from(rel);
        let abs = if p.is_absolute() { p } else { root.join(p) };
        state.open_file(abs);
    }
    if let Some(a) = ws.active_file {
        if a < state.open_files.len() {
            state.active_file = Some(a);
        }
    }
    state.sidebar_tab = ws.sidebar_tab;
    state.bottom_tab = ws.bottom_tab;
    state.show_sidebar = ws.show_sidebar;
    state.show_bottom = ws.show_bottom;
    // Tinggi panel bawah di-restore; clamp minimal agar tab bar tidak hilang
    // (clamp maksimal dilakukan per-frame di app.rs terhadap tinggi window).
    state.bottom_height = ws.bottom_height.max(100.0);
    state.show_outline = ws.show_outline;
    state.wave_zoom = ws.wave_zoom.max(0.5);
    state.wave_hidden = ws.wave_hidden.into_iter().collect();
    state.max_time = ws.max_time.max(1);
    // Bookmark (path relatif di-resolve terhadap root).
    for rel in ws.bookmarks {
        let p = PathBuf::from(rel);
        let abs = if p.is_absolute() { p } else { root.join(p) };
        if abs.exists() {
            state.bookmarks.insert(abs);
        }
    }

    state.log(format!(
        "↩ Workspace dipulihkan: {} · {} file terbuka",
        state.project_name,
        state.open_files.len()
    ));
}

/// Path relatif terhadap root bila memungkinkan (portabel), else absolut.
fn rel_or_abs(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_or_abs_prefers_relative() {
        let root = Path::new("/proj");
        assert_eq!(rel_or_abs(root, Path::new("/proj/src/a.sv")), "src/a.sv");
        assert_eq!(rel_or_abs(root, Path::new("/other/x.sv")), "/other/x.sv");
    }

    #[test]
    fn workspace_roundtrip_via_serde() {
        let ws = WorkspaceState {
            open_files: vec!["a.sv".into(), "tb/b.sv".into()],
            active_file: Some(1),
            sidebar_tab: SidebarTab::Architecture,
            bottom_tab: BottomTab::Waveform,
            show_sidebar: false,
            show_bottom: true,
            bottom_height: 320.0,
            show_outline: false,
            wave_zoom: 8.0,
            wave_hidden: vec!["clk".into()],
            max_time: 5000,
            bookmarks: vec!["core/cache.sv".into()],
        };
        let json = serde_json::to_string(&ws).expect("serialize");
        let back: WorkspaceState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.open_files, ws.open_files);
        assert_eq!(back.active_file, Some(1));
        assert_eq!(back.sidebar_tab, SidebarTab::Architecture);
        assert_eq!(back.bottom_tab, BottomTab::Waveform);
        assert!(!back.show_sidebar);
        assert_eq!(back.bottom_height, 320.0);
        assert_eq!(back.wave_zoom, 8.0);
        assert_eq!(back.max_time, 5000);
        assert_eq!(back.bookmarks, vec!["core/cache.sv".to_string()]);
    }

    #[test]
    fn recent_projects_roundtrip_dedupe_cap() {
        // Isolasi config dir ke temp — jangan sentuh config user asli.
        let cfg = std::env::temp_dir().join(format!("mivon_ws_test_{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &cfg);
        let a = cfg.join("proj_a");
        let b = cfg.join("proj_b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        push_recent(&a);
        push_recent(&b);
        push_recent(&a); // dedupe → pindah ke depan, bukan duplikat
        let recents = load_recents();
        assert_eq!(recents.len(), 2);
        assert_eq!(recents[0], a);
        assert_eq!(recents[1], b);

        // Isi > cap → tertimpa yang paling lama.
        for i in 0..(RECENT_MAX + 3) {
            let p = cfg.join(format!("proj_{}", i));
            if !p.exists() {
                std::fs::create_dir_all(&p).unwrap();
            }
            push_recent(&p);
        }
        let capped = load_recents();
        assert!(capped.len() <= RECENT_MAX, "cap recent");

        std::fs::remove_dir_all(&cfg).ok();
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
