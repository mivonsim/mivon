//! Command Palette (Ctrl+Shift+P) — akses cepat semua aksi nyata GUI.
//!
//! Modal filter teks di tengah layar: ↑/↓ memilih, Enter menjalankan ITEM
//! YANG DIPILIH (bukan menebak dari teks filter), Esc menutup.
//! Daftar aksi dan eksekusi hidup di sini (bukan hardcoded di app.rs) supaya
//! bisa di-unit-test tanpa terminal.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::super::app_cmds;
use super::super::backend::signals_to_vcd;
use super::super::state::{BottomTab, GuiState, SidebarTab};

/// Aksi yang bisa dijalankan palette. Hanya berisi aksi yang benar-benar
/// punya implementasi — tidak ada placeholder/wizard mati.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    OpenProject,
    Compile,
    Run,
    Stop,
    Save,
    Format,
    Generate,
    ToggleSidebar,
    ToggleTheme,
    Synth,
    Formal,
    ToggleBottom,
    ShowProblems,
    ShowConsole,
    ShowSignals,
    ShowWaveform,
    ShowTrace,
    ShowBenchmark,
    ShowCoverage,
    ShowAssertions,
    ShowDebugger,
    ShowTerminal,
    ShowArchitecture,
    ShowDependency,
    ShowSearch,
    ClearConsole,
    ExportVcd,
    Exit,
}

/// (label, shortcut, aksi). Urutan = urutan tampil di palette.
pub type PaletteEntry = (&'static str, &'static str, PaletteAction);

pub const ACTIONS: &[PaletteEntry] = &[
    ("Run Simulation", "F5", PaletteAction::Run),
    ("Compile + Elaborate", "F7", PaletteAction::Compile),
    ("Stop Simulation", "F5", PaletteAction::Stop),
    ("Open Project…", "Ctrl+O", PaletteAction::OpenProject),
    ("Save File", "Ctrl+S", PaletteAction::Save),
    ("Format File Aktif (mfmt)", "Ctrl+Shift+F", PaletteAction::Format),
    ("Generate .sv dari .mv", "Ctrl+G", PaletteAction::Generate),
    ("Tema (gelap/terang/tinggi)", "Ctrl+T", PaletteAction::ToggleTheme),
    ("Sintesis (SYN/timing/area)", "F11", PaletteAction::Synth),
    ("Formal Verification (BMC)", "F12", PaletteAction::Formal),
    ("Toggle Sidebar", "Ctrl+B", PaletteAction::ToggleSidebar),
    ("Toggle Bottom Panel", "Ctrl+`", PaletteAction::ToggleBottom),
    ("Open Problems", "", PaletteAction::ShowProblems),
    ("Open Console", "", PaletteAction::ShowConsole),
    ("Open Signals", "", PaletteAction::ShowSignals),
    ("Open Waveform", "", PaletteAction::ShowWaveform),
    ("Open Trace", "", PaletteAction::ShowTrace),
    ("Open Benchmark", "", PaletteAction::ShowBenchmark),
    ("Open Coverage", "", PaletteAction::ShowCoverage),
    ("Open Assertions", "", PaletteAction::ShowAssertions),
    ("Open Debugger", "", PaletteAction::ShowDebugger),
    ("Open Terminal", "", PaletteAction::ShowTerminal),
    ("Open Architecture", "", PaletteAction::ShowArchitecture),
    ("Open Dependency", "", PaletteAction::ShowDependency),
    ("Open Search", "", PaletteAction::ShowSearch),
    ("Clear Console", "", PaletteAction::ClearConsole),
    ("Export Waveform (VCD)", "", PaletteAction::ExportVcd),
    ("Exit", "Ctrl+Q", PaletteAction::Exit),
];

/// Filter case-insensitive berdasarkan label. Indicator kosong = semua aksi.
pub fn filtered_actions(filter: &str) -> Vec<&'static PaletteEntry> {
    let q = filter.trim().to_lowercase();
    ACTIONS
        .iter()
        .filter(|(name, _, _)| q.is_empty() || name.to_lowercase().contains(&q))
        .collect()
}

/// Jalankan SATU aksi. Mengembalikan `true` bila aplikasi diminta keluar.
pub fn execute(state: &mut GuiState, action: PaletteAction) -> bool {
    match action {
        PaletteAction::OpenProject => app_cmds::trigger_open_project(state),
        PaletteAction::Compile => app_cmds::trigger_compile(state),
        PaletteAction::Run => app_cmds::trigger_run(state),
        PaletteAction::Stop => app_cmds::trigger_stop(state),
        PaletteAction::Save => {
            if state.save_active_file() {
                state.log("💾 File disimpan");
            }
        }
        PaletteAction::Format => app_cmds::trigger_format(state),
        PaletteAction::Generate => app_cmds::trigger_generate(state),
        PaletteAction::ToggleTheme => state.theme = state.theme.next(),
        PaletteAction::Synth => app_cmds::trigger_synth(state),
        PaletteAction::Formal => app_cmds::trigger_formal(state),
        PaletteAction::ToggleSidebar => state.show_sidebar = !state.show_sidebar,
        PaletteAction::ToggleBottom => state.show_bottom = !state.show_bottom,
        PaletteAction::ShowProblems => show_bottom(state, BottomTab::Problems),
        PaletteAction::ShowConsole => show_bottom(state, BottomTab::Console),
        PaletteAction::ShowSignals => show_bottom(state, BottomTab::Signals),
        PaletteAction::ShowWaveform => show_bottom(state, BottomTab::Waveform),
        PaletteAction::ShowTrace => show_bottom(state, BottomTab::Trace),
        PaletteAction::ShowBenchmark => show_bottom(state, BottomTab::Benchmark),
        PaletteAction::ShowCoverage => show_bottom(state, BottomTab::Coverage),
        PaletteAction::ShowAssertions => show_bottom(state, BottomTab::Assertions),
        PaletteAction::ShowDebugger => show_bottom(state, BottomTab::Debugger),
        PaletteAction::ShowTerminal => show_bottom(state, BottomTab::Terminal),
        PaletteAction::ShowArchitecture => show_sidebar(state, SidebarTab::Architecture),
        PaletteAction::ShowDependency => show_sidebar(state, SidebarTab::Dependency),
        PaletteAction::ShowSearch => show_sidebar(state, SidebarTab::Search),
        PaletteAction::ClearConsole => state.console.clear(),
        PaletteAction::ExportVcd => export_vcd(state),
        PaletteAction::Exit => return true,
    }
    false
}

/// Jalankan item ke-`selected` DARI HASIL FILTER. Ini kontrak utama palette:
/// Enter selalu menjalankan baris yang disorot, bukan `starts_with` filter.
pub fn execute_selected(state: &mut GuiState, filter: &str, selected: usize) -> bool {
    let items = filtered_actions(filter);
    if items.is_empty() {
        return false;
    }
    let idx = selected.min(items.len() - 1);
    execute(state, items[idx].2)
}

fn show_bottom(state: &mut GuiState, tab: BottomTab) {
    state.show_bottom = true;
    state.bottom_tab = tab;
}

fn show_sidebar(state: &mut GuiState, tab: SidebarTab) {
    state.show_sidebar = true;
    state.sidebar_tab = tab;
}

/// Tulis waveform hasil simulasi terakhir ke `<project>/mivon_wave.vcd`.
/// Fallback ke temp_dir bila project belum dibuka — tetap menghasilkan file
/// nyata yang bisa dibuka viewer VCD mana pun.
fn export_vcd(state: &mut GuiState) {
    if state.waveform.is_empty() {
        state.log("⚠ Ekspor VCD: belum ada waveform (jalankan simulasi dulu)");
        return;
    }
    let root = state
        .project_root
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let path = root.join("mivon_wave.vcd");
    match std::fs::write(&path, signals_to_vcd(&state.waveform)) {
        Ok(()) => state.log(format!("💾 VCD diekspor: {}", path.display())),
        Err(e) => state.log(format!("❌ Ekspor VCD gagal: {}", e)),
    }
}

/// Render modal palette. `state.palette_selected` adalah indeks DALAM hasil
/// filter (bukan indeks ACTIONS) — sama dengan yang dipakai `execute_selected`.
pub fn show(f: &mut Frame, area: Rect, state: &GuiState) {
    f.render_widget(Clear, area);
    let block = Block::default()
        .title("Command Palette (Ctrl+Shift+P)")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::DarkGray));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1,
        horizontal: 1,
    });
    if inner.height < 3 {
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let input = Paragraph::new(format!("> {}▏", state.palette_filter))
        .style(Style::default().fg(Color::White));
    f.render_widget(input, chunks[0]);

    let items = filtered_actions(&state.palette_filter);
    let visible = chunks[1].height.saturating_sub(2) as usize;
    let start = state.palette_selected.saturating_sub(visible.saturating_sub(1) / 2);
    let list: Vec<ListItem> = items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(row, (name, key, _))| {
            let label = if key.is_empty() {
                format!("  {}", name)
            } else {
                format!("  {:<28} {}", name, key)
            };
            let style = if row == state.palette_selected {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(label).style(style)
        })
        .collect();
    let empty: Vec<ListItem> = if items.is_empty() {
        vec![ListItem::new("  (tidak ada perintah yang cocok)")
            .style(Style::default().fg(Color::DarkGray))]
    } else {
        Vec::new()
    };
    let widget = List::new(if list.is_empty() { empty } else { list }).block(
        Block::default()
            .title(format!("{}/{}", items.len().min(state.palette_selected + 1), items.len()))
            .borders(Borders::TOP),
    );
    f.render_widget(widget, chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{GuiEvent, WaveformSignal};
    use std::sync::mpsc::channel;

    fn state() -> GuiState {
        let (tx, rx) = channel::<GuiEvent>();
        GuiState::new(tx, rx)
    }

    #[test]
    fn filter_matches_label_case_insensitive() {
        let got = filtered_actions("cons");
        assert_eq!(got.len(), 2, "Open Console + Clear Console");
        assert_eq!(got[0].0, "Open Console");
        assert_eq!(got[0].2, PaletteAction::ShowConsole);
    }

    #[test]
    fn empty_filter_lists_all_actions() {
        assert_eq!(filtered_actions("").len(), ACTIONS.len());
    }

    #[test]
    fn execute_selected_runs_the_highlighted_row_not_the_first_one() {
        // Filter "Open " menghasilkan banyak baris. Pilih indeks 3
        // (Open Waveform) — harus membuka Waveform, BUKAN Open Project.
        let items = filtered_actions("Open ");
        let pos = items
            .iter()
            .position(|e| e.2 == PaletteAction::ShowWaveform)
            .expect("Open Waveform ada di filter");
        assert!(pos > 0, "uji hanya bermakna bukan baris pertama");

        let mut s = state();
        s.bottom_tab = BottomTab::Problems;
        let exit = execute_selected(&mut s, "Open ", pos);
        assert!(!exit);
        assert_eq!(s.bottom_tab, BottomTab::Waveform);
        assert!(s.show_bottom);
    }

    #[test]
    fn execute_selected_clamps_out_of_range_index() {
        let mut s = state();
        s.bottom_tab = BottomTab::Problems;
        let items = filtered_actions("Open Console");
        let last = items.len() - 1;
        let exit = execute_selected(&mut s, "Open Console", last + 99);
        assert!(!exit);
        assert_eq!(s.bottom_tab, BottomTab::Console);
    }

    #[test]
    fn exit_action_reports_exit_request() {
        let mut s = state();
        assert!(execute_selected(&mut s, "Exit", 0));
    }

    #[test]
    fn export_vcd_writes_real_file() {
        let mut s = state();
        let dir = std::env::temp_dir().join(format!("mivon_palette_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        s.project_root = Some(dir.clone());
        s.waveform = vec![WaveformSignal {
            name: "clk".into(),
            width: 1,
            trace: vec![(0, "0".into()), (5, "1".into())],
        }];
        let exit = execute_selected(&mut s, "Export Waveform", 0);
        assert!(!exit);
        let out = dir.join("mivon_wave.vcd");
        let text = std::fs::read_to_string(&out).expect("file VCD nyata harus ada");
        assert!(text.contains("$var wire 1 s0 clk $end"));
        assert!(text.contains("#5"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
