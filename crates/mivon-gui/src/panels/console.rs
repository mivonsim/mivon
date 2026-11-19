//! Tab Console: log console.

use ratatui::{
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
use ratatui::style::Modifier;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!("Console ({})", state.console.len()))
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

    if state.console.is_empty() {
        let p = Paragraph::new("Console kosong").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    }

    let items: Vec<ListItem> = state
        .console
        .iter()
        .rev()
        .take(inner.height as usize)
        .map(|line| {
            ListItem::new(format!("{} {}", line.time, line.msg))
                .style(Style::default().fg(Color::White))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}
