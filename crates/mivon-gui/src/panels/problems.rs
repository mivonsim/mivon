//! Tab Problems: diagnostic + LSP diagnostics + Quick Fix.

use ratatui::{
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::{DiagEntry, DiagLevel, GuiState};
use ratatui::style::Modifier;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let all: Vec<&DiagEntry> = state
        .diagnostics
        .iter()
        .chain(state.lsp_diags.iter())
        .collect();

    let block = Block::default()
        .title(format!("Problems ({})", all.len()))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 1 {
        return;
    }

    if all.is_empty() {
        let p = Paragraph::new("No problems detected").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    }

    let items: Vec<ListItem> = all
        .iter()
        .take(inner.height as usize)
        .map(|d| {
            let (icon, color) = match d.level {
                DiagLevel::Error | DiagLevel::Bug | DiagLevel::Fatal => ("✖", Color::Red),
                DiagLevel::Warning => ("⚠", Color::Yellow),
                _ => ("ℹ", Color::Blue),
            };
            let loc = if d.file.is_empty() {
                "global".to_string()
            } else {
                format!("{}:{}", d.file, d.line)
            };
            let fix = if d.fix.is_some() { " 💡" } else { "" };
            ListItem::new(format!("{} {} {} — {}", icon, loc, d.message, fix))
                .style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);

    // Hint.
    if !all.is_empty() {
        let hint =
            Paragraph::new("Klik baris → buka file & lompat ke baris  |  💡 = Quick Fix tersedia")
                .style(Style::default().fg(Color::DarkGray));
        f.render_widget(
            hint,
            ratatui::layout::Rect {
                x: inner.x,
                y: inner.y.saturating_add(inner.height.saturating_sub(1)),
                width: inner.width,
                height: 1,
            },
        );
    }
}
