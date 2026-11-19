//! Tab Assertions: pass/fail per lokasi assertion.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!("Assertions ({})", state.assertions.len()))
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

    if state.assertions.is_empty() {
        let p = Paragraph::new("Tidak ada assertion data").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    }

    let items: Vec<ListItem> = state
        .assertions
        .iter()
        .take(inner.height as usize)
        .map(|a| {
            let total = a.pass + a.fail;
            let status = if a.fail > 0 { "FAIL" } else { "PASS" };
            let color = if a.fail > 0 { Color::Red } else { Color::Green };
            ListItem::new(format!(
                "L{}:{}  {}  pass={} fail={} total={}",
                a.line, a.col, status, a.pass, a.fail, total
            ))
            .style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}
