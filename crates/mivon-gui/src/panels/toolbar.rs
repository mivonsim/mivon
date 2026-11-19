//! Toolbar atas: brand, project name, status, tombol cepat.

use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::GuiState;
use crate::theme::Theme;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let theme = state.theme;
    let block = Block::default()
        .title("Mivon — RTL Engineering Control Center (TUI)")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.bg()).fg(theme.fg()));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 0,
        horizontal: 1,
    });
    if inner.height < 1 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(20), // brand + project
            Constraint::Length(20), // max time
            Constraint::Length(20), // run/stop
            Constraint::Length(12), // compile
            Constraint::Min(0),     // status / fill
        ])
        .split(inner);

    // Brand + project name.
    let project_text = if state.project_name.is_empty() {
        "No project".to_string()
    } else {
        format!("Project: {}", state.project_name)
    };
    let brand = Paragraph::new(format!("Mivon  |  {}", project_text))
        .style(Style::default().fg(theme.highlight()).bg(theme.bg()));
    f.render_widget(brand, chunks[0]);

    // Max time.
    let max_time =
        Paragraph::new(format!("T={}", state.max_time)).style(Style::default().fg(theme.highlight()));
    f.render_widget(max_time, chunks[1]);

    // Run / Stop / Pause indicator.
    let run_status = if state.is_paused {
        "⏸ Paused".to_string()
    } else if state.is_running {
        "⏹ Running".to_string()
    } else {
        "▶ Ready".to_string()
    };
    let run_color = if state.is_paused {
        Color::Yellow
    } else if state.is_running {
        Color::Red
    } else {
        Color::Green
    };
    let run_p = Paragraph::new(run_status).style(Style::default().fg(run_color));
    f.render_widget(run_p, chunks[2]);

    // Compile hint.
    let compile = Paragraph::new("F7 Compile").style(Style::default().fg(Color::Yellow));
    f.render_widget(compile, chunks[3]);

    // Status / hints (kanan).
    let hints = Paragraph::new("F5 Run/Resume  |  F8 Pause  |  F9 Reset  |  F10 Restart  |  F7 Compile  |  F11 Synth  |  Ctrl+O Open  |  Ctrl+S Save  |  Ctrl+G Gen  |  Ctrl+T Tema  |  Ctrl+Shift+P Palette  |  Ctrl+Q Exit")
        .style(Style::default().fg(theme.fg()).bg(theme.bg()));
    f.render_widget(hints, chunks[4]);
}
