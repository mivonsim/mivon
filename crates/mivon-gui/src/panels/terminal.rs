//! Tab Terminal: output terminal perintah terakhir.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!("Terminal ({} lines)", state.term_lines.len()))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 2 {
        return;
    }

    if state.term_lines.is_empty() {
        let p = Paragraph::new("Terminal kosong").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    }

    let items: Vec<ListItem> = state
        .term_lines
        .iter()
        .rev()
        .take(inner.height.saturating_sub(1) as usize)
        .map(|line| {
            let color = if line.is_err {
                Color::Red
            } else {
                Color::White
            };
            ListItem::new(line.text.as_str()).style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);

    // Input bar di bawah.
    let input =
        Paragraph::new(format!("> {}", state.term_input)).style(Style::default().fg(Color::Cyan));
    f.render_widget(
        input,
        ratatui::layout::Rect {
            x: inner.x,
            y: inner.y.saturating_add(inner.height.saturating_sub(1)),
            width: inner.width,
            height: 1,
        },
    );
}
