//! `MivonApp` — loop utama ratatui TUI.
//!
//! Layout:
//!   ┌─────────────────────────────────────────────┐
//!   │ Toolbar (Mivon | project | Run/Stop/Compile) │
//!   ├──────────┬──────────────────────────────────┤
//!   │ Sidebar  │        Editor (RTL)             │
//!   │          │                                   │
//!   ├──────────┴──────────────────────────────────┤
//!   │ Bottom panel: Problems/Console/Signals/…   │
//!   └─────────────────────────────────────────────┘
//!
//! Semua operasi berat di worker thread; hasil dipoll per-frame lewat channel.

use std::io;
use std::sync::mpsc::channel;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame, Terminal,
};

use super::app_cmds;
use super::backend::scan_tree;
use super::panels::{bottom_tabs, editor, sidebar, toolbar};
use super::syntax::is_ident_char;
use super::state::{
    BottomTab, DiagEntry, DiagLevel, EditorMode, GuiEvent, GuiState, LspPendingKind, SidebarTab,
    STAGE_SIMULATOR,
};
use super::workspace::{restore_workspace, save_workspace};

pub struct MivonApp {
    pub state: GuiState,
    pub exit: bool,
    pub pending_open: Option<std::path::PathBuf>,
    pub pending_top: Option<String>,
}

impl Default for MivonApp {
    fn default() -> Self {
        Self::new()
    }
}

impl MivonApp {
    pub fn new() -> Self {
        let (tx, rx) = channel::<GuiEvent>();
        Self {
            state: GuiState::new(tx, rx),
            exit: false,
            pending_open: None,
            pending_top: None,
        }
    }

    pub fn open_project_dir(&mut self, dir: &std::path::Path) {
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Project".to_string());
        let name_clone = name.clone();
        let dir_clone = dir.to_path_buf();
        let files = scan_tree(dir);
        self.state.project_name = name;
        self.state.project_root = Some(dir_clone.clone());
        self.state.files = files;
        self.state.compile_info = None;
        self.state.signals.clear();
        self.state.diagnostics.clear();
        self.state.log(format!("📂 Project dibuka: {}", name_clone));
        super::workspace::push_recent(&dir_clone);
        save_workspace(&self.state);
    }

    pub fn restore_workspace(&mut self) {
        restore_workspace(&mut self.state);
    }

    fn poll_events(&mut self) {
        while let Ok(ev) = self.state.rx.try_recv() {
            match ev {
                GuiEvent::CompileDone(result) => match result {
                    Ok((info, design)) => {
                        let pending_top = self.pending_top.take();
                        self.state.design = Some(design);
                        self.state.compile_info = Some(info);
                        self.state.dep_graph = None;
                        let (lint, modules_len) = match &self.state.compile_info {
                            Some(ci) => (ci.lint.clone(), ci.modules.len()),
                            None => (Vec::new(), 0),
                        };
                        if !lint.is_empty() {
                            self.state
                                .log(format!("🔍 Lint: {} warning(s)", lint.len()));
                            self.state.diagnostics.extend(lint);
                        }
                        // Tentukan top module: prioritas pending_top (dari --top),
                        // lalu selected_top yang sudah ada, lalu module pertama
                        // hasil compile. Dipakai saat Run bila tidak ada pemilih
                        // lain.
                        if let Some(top) = pending_top {
                            self.state.selected_top = Some(top.clone());
                            self.state.log(format!("▶ Top module: {}", top));
                        } else if self.state.selected_top.is_none() {
                            if let Some(ci) = &self.state.compile_info {
                                if let Some(first) = ci.modules.first() {
                                    self.state.selected_top = Some(first.clone());
                                    self.state.log(format!(
                                        "📌 Top module default: {}", first
                                    ));
                                }
                            }
                        }
                        self.state
                            .log(format!("✅ Compile selesai ({} module)", modules_len));
                    }
                    Err(diags) => {
                        for d in &diags {
                            let loc = if d.file.is_empty() {
                                String::new()
                            } else {
                                format!("{}:{}", d.file, d.line)
                            };
                            self.state.log(format!("❌ [{}] {}", loc, d.message));
                        }
                        self.state.diagnostics.extend(diags);
                    }
                },
                GuiEvent::SimDone(result) => {
                    self.state.is_running = false;
                    self.state.is_paused = false;
                    match result {
                        Ok(info) => {
                            if let Some(ci) = self.state.compile_info.as_mut() {
                                if let Some(stage) =
                                    ci.pipeline.iter_mut().find(|s| s.name == STAGE_SIMULATOR)
                                {
                                    stage.status = "ok".into();
                                    stage.ms = info.sim_time_ms as u64;
                                }
                            }
                            self.state.signals = info.signals;
                            self.state.waveform = info.waveform.clone();
                            self.state.cycles = info.cycles;
                            self.state.sim_time_ms = info.sim_time_ms;
                            self.state.coverage = info.coverage;
                            self.state.assertions = info.assertions;
                            self.state.debug_events = info.debug_events.clone();
                            if !self.state.debug_events.is_empty() {
                                self.state.log(format!(
                                    "🐞 {} debug event (breakpoint/watchpoint)",
                                    self.state.debug_events.len()
                                ));
                            }
                            self.state.delta_cycles = info.delta_cycles;
                            self.state.events_processed = info.events_processed;
                            self.state.log(format!(
                                "✅ Simulasi selesai — t={} ({} signal, {:.2}ms)",
                                info.cycles,
                                self.state.signals.len(),
                                info.sim_time_ms
                            ));
                        }
                        Err(e) => {
                            if !e.starts_with("Simulasi dihentikan") {
                                self.state.diagnostics.push(DiagEntry {
                                    file: String::new(),
                                    line: 0,
                                    message: format!("Simulation error: {}", e),
                                    level: DiagLevel::Error,
                                    fix: None,
                                });
                                self.state.log(format!("❌ Simulasi gagal: {}", e));
                            } else {
                                self.state.log(format!("⏹ {}", e));
                            }
                        }
                    }
                }
                GuiEvent::SimPaused(info) => {
                    self.state.is_running = true;
                    self.state.is_paused = true;
                    let n_signal = info.signals.len();
                    self.state.signals = info.signals;
                    self.state.cycles = info.cycles;
                    self.state.sim_time_ms = info.sim_time_ms;
                    self.state.debug_events = info.debug_events.clone();
                    self.state.log(format!(
                        "⏸ Simulasi dijeda — t={} ({} signal)", info.cycles, n_signal
                    ));
                }
                GuiEvent::TermOutput(text, is_err) => {
                    self.state
                        .term_lines
                        .push(super::state::TermLine { text, is_err });
                    if self.state.term_lines.len() > super::state::MAX_TERM_LINES {
                        let overflow = self.state.term_lines.len() - super::state::MAX_TERM_LINES;
                        self.state.term_lines.drain(0..overflow);
                    }
                }
                GuiEvent::TermExit(code) => {
                    self.state.term_running = false;
                    let status = if code == 0 {
                        "✅ selesai".to_string()
                    } else {
                        format!("❌ exit code {}", code)
                    };
                    self.state.term_lines.push(super::state::TermLine {
                        text: status,
                        is_err: code != 0,
                    });
                }
                GuiEvent::LspStarted(_)
                | GuiEvent::LspStopped(_)
                | GuiEvent::LspDiagnostics(_, _)
                | GuiEvent::LspReply { .. } => {}
            }
        }
        if let Some(lsp) = &self.state.lsp {
            let mut evs: Vec<GuiEvent> = Vec::new();
            while let Ok(ev) = lsp.rx.try_recv() {
                evs.push(ev);
            }
            for ev in evs {
                self.handle_lsp_event(ev);
            }
        }
    }

    fn handle_lsp_event(&mut self, ev: GuiEvent) {
        match ev {
            GuiEvent::LspStarted(detail) => {
                self.state.lsp_ready = true;
                self.state.log(format!("🔌 LSP siap ({})", detail));
            }
            GuiEvent::LspStopped(reason) => {
                self.state.lsp_ready = false;
                self.state.lsp = None;
                self.state.lsp_opened.clear();
                self.state.lsp_pending.clear();
                self.state.lsp_completion_req.clear();
                self.state.lsp_auto_restart_blocked = true;
                self.state.log(format!("⛔ LSP: {}", reason));
            }
            GuiEvent::LspDiagnostics(file, diags) => {
                let n = diags.len();
                self.state
                    .lsp_diags
                    .retain(|d| !super::state::diag_matches_file(&d.file, &file));
                if !diags.is_empty() {
                    self.state.lsp_diags.extend(diags);
                    self.state
                        .log(format!("🔍 LSP diagnostics {}: {} baris", file, n));
                }
            }
            GuiEvent::LspReply { id, value } => {
                let kind = self.state.lsp_pending.remove(&id);
                match kind {
                    Some(LspPendingKind::HoverRequest) => {
                        if let Some(v) = value {
                            if let Ok(Some(hover)) =
                                serde_json::from_value::<Option<lsp_types::Hover>>(v)
                            {
                                if let Some(text) = super::lsp_client::hover_text(&hover) {
                                    let name = self
                                        .state
                                        .lsp_last_hover
                                        .clone()
                                        .unwrap_or_else(|| "symbol".to_string());
                                    self.state.lsp_hover = Some((name, text));
                                }
                            }
                        }
                    }
                    Some(LspPendingKind::GotoRequest) => {
                        if let Some(v) = value {
                            if let Some((path, line)) = super::lsp_client::definition_target(&v) {
                                if !self.state.open_files.iter().any(|of| of.path == path) {
                                    self.state.open_file(path.clone());
                                }
                                if let Some(idx) =
                                    self.state.open_files.iter().position(|of| of.path == path)
                                {
                                    self.state.active_file = Some(idx);
                                    self.state.open_files[idx].pending_goto = Some(line);
                                    // Set kursor nyata + auto-scroll (fix: goto dulu hanya
                                    // menyimpan pending_goto, editor_scroll tetap 0).
                                    self.state.editor_set_cursor(line.saturating_sub(1), 0);
                                    self.state.log(format!(
                                        "⌗ LSP definition: {}:{}",
                                        path.display(),
                                        line
                                    ));
                                }
                            } else {
                                self.state.log("⚠ LSP: definition tidak ditemukan");
                            }
                        }
                    }
                    Some(LspPendingKind::CompletionRequest) => {
                        if let Some(v) = value {
                            let items: Vec<String> =
                                super::lsp_client::completion_items(&v);
                            // Ambil prefix yang disimpan saat request dikirim.
                            let prefix = &self.state.lsp_completion_prefix;
                            let filtered: Vec<String> = items
                                .into_iter()
                                .filter(|s| {
                                    prefix.is_empty()
                                        || s.get(..prefix.len())
                                            .map(|h| h.eq_ignore_ascii_case(prefix))
                                            .unwrap_or(false)
                                })
                                .collect();
                            let n_items = filtered.len();
                            self.state.lsp_completion_items = filtered;
                            self.state.lsp_completion_selected = 0;
                            self.state.lsp_completion_show = n_items > 0;
                            if n_items > 0 {
                                self.state.log(format!(
                                    "🔤 Completion: {} kandidat",
                                    n_items
                                ));
                            }
                        }
                        self.state.lsp_completion_req.remove(&id);
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl && key.code == KeyCode::Char('q') {
            self.exit = true;
            return;
        }
        if ctrl && key.code == KeyCode::Char('s') {
            if self.state.save_active_file() {
                self.state.log("💾 File disimpan");
            }
            return;
        }
        if ctrl && key.code == KeyCode::Char('o') {
            app_cmds::trigger_open_project(&mut self.state);
            return;
        }
        if ctrl && key.code == KeyCode::Char('g') {
            app_cmds::trigger_generate(&mut self.state);
            return;
        }
        if ctrl && shift && key.code == KeyCode::Char('p') {
            self.state.palette_open = !self.state.palette_open;
            if self.state.palette_open {
                self.state.palette_filter.clear();
                self.state.palette_selected = 0;
            }
            return;
        }
        if ctrl && key.code == KeyCode::Char('b') {
            self.state.show_sidebar = !self.state.show_sidebar;
            return;
        }

        if self.state.palette_open {
            self.handle_palette_key(key);
            return;
        }

        // Navigasi kursor waveform (, = mundur, . = maju) saat tab Waveform aktif.
        if self.state.bottom_tab == BottomTab::Waveform && !self.state.lsp_completion_show {
            match key.code {
                KeyCode::Char(',') => {
                    self.state.wave_cursor = self.state.wave_cursor.saturating_sub(10);
                    return;
                }
                KeyCode::Char('.') => {
                    let t_max = self
                        .state
                        .waveform
                        .iter()
                        .flat_map(|s| s.trace.iter())
                        .map(|(t, _)| *t)
                        .max()
                        .unwrap_or(0);
                    self.state.wave_cursor = (self.state.wave_cursor + 10).min(t_max);
                    return;
                }
                _ => {}
            }
        }

        // Navigasi / insert popup completion LSP.
        if self.state.lsp_completion_show {
            match key.code {
                KeyCode::Esc => {
                    self.state.lsp_completion_show = false;
                    return;
                }
                KeyCode::Up => {
                    self.state.lsp_completion_selected =
                        self.state.lsp_completion_selected.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    self.state.lsp_completion_selected = self
                        .state
                        .lsp_completion_selected
                        .saturating_add(1)
                        .min(self.state.lsp_completion_items.len().saturating_sub(1));
                    return;
                }
                KeyCode::Enter => {
                    self.insert_completion();
                    return;
                }
                _ => {
                    // Lainnya: tutup popup, lanjut ke penanganan biasa.
                    self.state.lsp_completion_show = false;
                }
            }
        }

        // F5/F7 berlaku global — termasuk saat editor fokus (fix bug lama).
        match key.code {
            KeyCode::F(5) => {
                if self.state.is_running {
                    app_cmds::trigger_stop(&mut self.state);
                } else {
                    app_cmds::trigger_run(&mut self.state);
                }
                return;
            }
            KeyCode::F(7) => {
                app_cmds::trigger_compile(&mut self.state);
                return;
            }
            KeyCode::F(8) => {
                // Pause / Resume simulasi.
                if self.state.is_running && !self.state.is_paused {
                    app_cmds::trigger_pause(&mut self.state);
                } else if self.state.is_paused {
                    app_cmds::trigger_run(&mut self.state); // resume
                }
                return;
            }
            KeyCode::F(9) => {
                app_cmds::trigger_reset(&mut self.state);
                return;
            }
            KeyCode::F(10) => {
                app_cmds::trigger_restart(&mut self.state);
                return;
            }
            KeyCode::Char(' ') if ctrl => {
                // Ctrl+Space: minta completion LSP pada posisi kursor editor.
                let Some(idx) = self.state.active_file else {
                    return;
                };
                let Some(f) = self.state.open_files.get(idx) else {
                    return;
                };
                let line_idx = self.state.editor_cursor_line;
                let col = self.state.editor_cursor_col;
                let line = f.content.lines().nth(line_idx).unwrap_or("");
                let f_path = f.path.clone();
                let _ = f;
                // Prefix = kata identifier sebelum kursor (untuk filter sisi klien).
                let chars: Vec<char> = line.chars().collect();
                let n = chars.len().min(col);
                let mut start = n;
                while start > 0 && is_ident_char(chars[start - 1]) {
                    start -= 1;
                }
                let prefix: String = chars[start..n].iter().collect();
                self.state.lsp_completion_prefix = prefix.clone();
                let id = self.state.lsp_alloc_id(LspPendingKind::CompletionRequest);
                let _ = self.state.lsp_send(super::lsp_client::LspCmd::Completion {
                    id,
                    path: f_path.clone(),
                    line: line_idx as u32,
                    character: col as u32,
                });
                self.state
                    .lsp_completion_req
                    .insert(id, (f_path, prefix.clone()));
                self.state.lsp_completion_show = false;
                self.state.log(format!("🔤 Completion (prefix='{}')", prefix));
                return;
            }
            _ => {}
        }

        // Input debugger panel (bp/wp/clear/debug on/off) saat tab Debugger aktif.
        if self.state.bottom_tab == BottomTab::Debugger && !self.state.lsp_completion_show {
            match key.code {
                KeyCode::Esc => {
                    self.state.debug_input.clear();
                    return;
                }
                KeyCode::Enter => {
                    let cmd = self.state.debug_input.clone();
                    self.state.debug_input.clear();
                    app_cmds::trigger_debug_cmd(&mut self.state, cmd);
                    return;
                }
                KeyCode::Backspace => {
                    self.state.debug_input.pop();
                    return;
                }
                KeyCode::Char(c) => {
                    self.state.debug_input.push(c);
                    return;
                }
                _ => {}
            }
        }

        // Editor interaktif: saat file aktif, keyboard mengedit teks nyata.
        if self.state.active_file.is_some() && self.handle_editor_key(key) {
            return;
        }

        if key.code == KeyCode::Tab {
            self.state.sidebar_tab = match self.state.sidebar_tab {
                SidebarTab::Project => SidebarTab::Symbols,
                SidebarTab::Symbols => SidebarTab::Architecture,
                SidebarTab::Architecture => SidebarTab::Dependency,
                SidebarTab::Dependency => SidebarTab::Search,
                SidebarTab::Search => SidebarTab::Project,
            };
            return;
        }

        if key.code == KeyCode::BackTab {
            self.state.bottom_tab = match self.state.bottom_tab {
                BottomTab::Problems => BottomTab::Pipeline,
                BottomTab::Console => BottomTab::Problems,
                BottomTab::Signals => BottomTab::Console,
                BottomTab::Waveform => BottomTab::Signals,
                BottomTab::Trace => BottomTab::Waveform,
                BottomTab::Benchmark => BottomTab::Trace,
                BottomTab::Coverage => BottomTab::Benchmark,
                BottomTab::Assertions => BottomTab::Coverage,
                BottomTab::Terminal => BottomTab::Assertions,
                BottomTab::Pipeline => BottomTab::Debugger,
                BottomTab::Debugger => BottomTab::Regression,
                BottomTab::Regression => BottomTab::Terminal,
            };
            return;
        }

        // Navigasi sidebar project tab (↑/↓) dan buka file terpilih (Enter).
        if self.state.show_sidebar && self.state.sidebar_tab == SidebarTab::Project {
            match key.code {
                KeyCode::Up => sidebar::sidebar_select_prev(&mut self.state),
                KeyCode::Down => sidebar::sidebar_select_next(&mut self.state),
                KeyCode::Enter => {
                    if self.state.active_file.is_none() {
                        sidebar::sidebar_open_selected(&mut self.state);
                    }
                }
                _ => {}
            }
            return;
        }

        if let Some(idx) = self.state.active_file {
            if key.code == KeyCode::Enter {
                let pending = self.state.open_files.get_mut(idx).and_then(|f| {
                    f.pending_goto
                        .take()
                        .map(|l| (f.path.display().to_string(), l))
                });
                if let Some((p, l)) = pending {
                    self.state.log(format!("→ {}:{}", p, l));
                }
            }
        }
    }

    /// Tangani keyboard saat editor fokus. Mengembalikan true bila key
    /// dikonsumsi editor (UI tidak boleh memproses lebih lanjut).
    fn handle_editor_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let view_h = 20usize; // perkiraan tinggi area editor; clamp di render

        // Ctrl+Z / Ctrl+Y: undo/redo (berlaku di dua mode).
        if ctrl && key.code == KeyCode::Char('z') {
            self.state.editor_undo();
            return true;
        }
        if ctrl && key.code == KeyCode::Char('y') {
            self.state.editor_redo();
            return true;
        }

        match self.state.editor_mode {
            EditorMode::Normal => match key.code {
                KeyCode::Char('i') => {
                    self.state.editor_mode = EditorMode::Insert;
                    true
                }
                KeyCode::Char('a') => {
                    self.state.editor_move_cursor(0, 1);
                    self.state.editor_mode = EditorMode::Insert;
                    true
                }
                KeyCode::Char('o') => {
                    self.state.editor_insert_newline();
                    self.state.editor_mode = EditorMode::Insert;
                    true
                }
                KeyCode::Char('x') => {
                    self.state.editor_delete_char();
                    true
                }
                KeyCode::Char('u') => {
                    self.state.editor_undo();
                    true
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.state.editor_redo();
                    true
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    self.state.editor_move_cursor(0, -1);
                    true
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.state.editor_move_cursor(1, 0);
                    self.state.editor_ensure_visible(view_h);
                    true
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.state.editor_move_cursor(-1, 0);
                    self.state.editor_ensure_visible(view_h);
                    true
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    self.state.editor_move_cursor(0, 1);
                    true
                }
                KeyCode::Home => {
                    self.state.editor_cursor_col = 0;
                    true
                }
                KeyCode::End => {
                    self.state.editor_clamp_cursor();
                    let lines = self.state.editor_lines();
                    if let Some(l) = lines.get(self.state.editor_cursor_line) {
                        self.state.editor_cursor_col = l.chars().count();
                    }
                    true
                }
                KeyCode::PageUp => {
                    self.state.editor_move_cursor(-(view_h as i64), 0);
                    self.state.editor_ensure_visible(view_h);
                    true
                }
                KeyCode::PageDown => {
                    self.state.editor_move_cursor(view_h as i64, 0);
                    self.state.editor_ensure_visible(view_h);
                    true
                }
                _ => false,
            },
            EditorMode::Insert => match key.code {
                KeyCode::Esc => {
                    self.state.editor_mode = EditorMode::Normal;
                    true
                }
                KeyCode::Char(c) if !ctrl => {
                    self.state.editor_insert_char(c);
                    true
                }
                KeyCode::Backspace => {
                    self.state.editor_backspace();
                    true
                }
                KeyCode::Delete => {
                    self.state.editor_delete_char();
                    true
                }
                KeyCode::Enter => {
                    self.state.editor_insert_newline();
                    true
                }
                KeyCode::Tab => {
                    for _ in 0..4 {
                        self.state.editor_insert_char(' ');
                    }
                    true
                }
                KeyCode::Left => {
                    self.state.editor_move_cursor(0, -1);
                    true
                }
                KeyCode::Right => {
                    self.state.editor_move_cursor(0, 1);
                    true
                }
                KeyCode::Up => {
                    self.state.editor_move_cursor(-1, 0);
                    self.state.editor_ensure_visible(view_h);
                    true
                }
                KeyCode::Down => {
                    self.state.editor_move_cursor(1, 0);
                    self.state.editor_ensure_visible(view_h);
                    true
                }
                _ => false,
            },
        }
    }

    fn handle_palette_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.palette_open = false;
            }
            KeyCode::Enter => {
                self.state.palette_open = false;
                let filter = self.state.palette_filter.trim().to_lowercase();
                if filter.starts_with("run") || filter.starts_with("simulasi") {
                    app_cmds::trigger_run(&mut self.state);
                } else if filter.starts_with("compile") {
                    app_cmds::trigger_compile(&mut self.state);
                } else if filter.starts_with("open") {
                    app_cmds::trigger_open_project(&mut self.state);
                } else if filter.starts_with("stop") {
                    app_cmds::trigger_stop(&mut self.state);
                } else if filter.starts_with("exit") || filter.starts_with("keluar") {
                    self.exit = true;
                } else {
                    self.state
                        .log(format!("Palette: '{}'", self.state.palette_filter));
                }
            }
            KeyCode::Backspace => {
                self.state.palette_filter.pop();
            }
            KeyCode::Char(c) => {
                self.state.palette_filter.push(c);
            }
            KeyCode::Up => {
                self.state.palette_selected = self.state.palette_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                self.state.palette_selected = self.state.palette_selected.saturating_add(1);
            }
            _ => {}
        }
    }

    /// Insert kandidat completion yang disorot, mengganti prefix sebelum kursor.
    /// Operasi berbasis char agar aman untuk multibyte (CJK/emoji).
    fn insert_completion(&mut self) {
        let item = match self.state.lsp_completion_items.get(self.state.lsp_completion_selected) {
            Some(i) => i.clone(),
            None => return,
        };
        self.state.lsp_completion_show = false;
        let Some(idx) = self.state.active_file else {
            return;
        };
        let line_idx = self.state.editor_cursor_line;
        let col = self.state.editor_cursor_col;
        let prefix = &self.state.lsp_completion_prefix;
        let Some(f) = self.state.open_files.get_mut(idx) else {
            return;
        };
        let line = f.content.lines().nth(line_idx).unwrap_or("");
        let mut chars: Vec<char> = line.chars().collect();
        let start = col.saturating_sub(prefix.chars().count()).min(chars.len());
        let end = col.min(chars.len());
        chars.splice(start..end, item.chars());
        let new_line: String = chars.into_iter().collect();
        let mut new_lines: Vec<String> = f.content.lines().map(|l| l.to_string()).collect();
        if line_idx < new_lines.len() {
            new_lines[line_idx] = new_line;
        }
        f.content = new_lines.join("\n");
        f.dirty = true;
        self.state.editor_cursor_col = start + item.chars().count();
        self.state.editor_cursor_line = line_idx;
        self.state.log(format!("✅ Insert '{}'", item));
    }

    pub fn run(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            self.poll_events();
            terminal.draw(|f| self.draw(f))?;

            if crossterm::event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = crossterm::event::read()? {
                    self.handle_key(key);
                }
            }

            if self.exit {
                break;
            }
        }

        save_workspace(&self.state);
        let _ = self.state.lsp_send(super::lsp_client::LspCmd::Stop);

        Ok(())
    }

    fn draw(&mut self, f: &mut Frame) {
        let size = f.size();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(10),
            ])
            .split(size);

        toolbar::show(f, chunks[0], &mut self.state);

        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(chunks[1]);

        if self.state.show_sidebar {
            sidebar::show(f, main_chunks[0], &mut self.state);
        }

        editor::show(f, main_chunks[1], &mut self.state);

        bottom_tabs::show(f, chunks[2], &mut self.state);

        if self.state.palette_open {
            self.draw_palette(f);
        }

        if let Some((name, text)) = self.state.lsp_hover.as_ref() {
            self.draw_hover(f, name, text);
        }
    }

    fn draw_palette(&self, f: &mut Frame) {
        let area = centered_rect(50, 30, f.size());
        f.render_widget(Clear, area);
        let block = Block::default()
            .title("Command Palette (Ctrl+Shift+P)")
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::DarkGray));
        f.render_widget(block, area);

        let inner = area.inner(&ratatui::layout::Margin {
            vertical: 1u16,
            horizontal: 1u16,
        });
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(inner);

        let input = Paragraph::new(format!("> {}", self.state.palette_filter))
            .style(Style::default().fg(Color::White));
        f.render_widget(input, chunks[0]);

        let items = [
            "Run simulation (F5)",
            "Compile + elaborate (F7)",
            "Stop simulation",
            "Open project (Ctrl+O)",
            "Save file (Ctrl+S)",
            "Generate SV/SVH (Ctrl+G)",
            "Exit (Ctrl+Q)",
        ];
        let list: List = items
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let selected = i == self.state.palette_selected;
                ListItem::new(format!("  {}", s)).style(if selected {
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                })
            })
            .collect();
        f.render_widget(list, chunks[1]);
    }

    fn draw_hover(&self, f: &mut Frame, name: &str, text: &str) {
        let area = centered_rect(60, 20, f.size());
        f.render_widget(Clear, area);
        let block = Block::default()
            .title(format!("◉ {}", name))
            .borders(Borders::ALL);
        let para = Paragraph::new(text.to_string())
            .block(block)
            .style(Style::default().fg(Color::White))
            .wrap(ratatui::widgets::Wrap { trim: false });
        f.render_widget(para, area);
    }
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    let popup_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1]);
    popup_layout[1]
}
