//! Tab Signals: daftar sinyal hasil simulasi dengan nilai hex.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!("Signals ({})", state.signals.len()))
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

    if state.signals.is_empty() {
        let p = Paragraph::new("Jalankan simulasi untuk melihat signal").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    }

    let items: Vec<ListItem> = state
        .signals
        .iter()
        .take(inner.height as usize)
        .map(|s| {
            ListItem::new(format!(
                "{:<30} width={:<4} value={:<16} kind={}",
                s.name, s.width, s.value, s.kind
            ))
            .style(Style::default().fg(Color::Cyan))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}
