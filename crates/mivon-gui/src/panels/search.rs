//! Tab Search: pencarian simbol / teks di project.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Search")
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

    let mut items: Vec<ListItem> = Vec::new();
    items.push(
        ListItem::new("Search categories").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    items.push(ListItem::new(format!("  Active: {:?}", state.search_cat)));
    items.push(ListItem::new(format!("  Filter: {}", state.search_filter)));
    items.push(ListItem::new(format!(
        "  Text hits: {}",
        state.text_hits.len()
    )));
    for hit in &state.text_hits {
        items.push(ListItem::new(format!(
            "  {}:{} — {}",
            hit.file.display(),
            hit.line,
            hit.text
        )));
    }
    if items.len() <= 1 {
        items.push(ListItem::new("(kosong)").style(Style::default().fg(Color::Gray)));
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
