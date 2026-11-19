//! Tab Coverage: ringkasan coverage simulasi.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let c = &state.coverage;

    let block = Block::default()
        .title("Coverage")
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
        ListItem::new("Coverage summary").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    items.push(ListItem::new(format!(
        "  Line items: {} / {}",
        c.line_hits, c.line_items
    )));
    items.push(ListItem::new(format!(
        "  Toggle signals: {}",
        c.toggle_signals
    )));
    items.push(ListItem::new(format!(
        "  Toggle transitions: {}",
        c.toggle_transitions
    )));
    items.push(ListItem::new(format!(
        "  Branch: {} / {} ({:.1}%)",
        c.branch_covered, c.branch_total, c.branch_percent
    )));
    items.push(ListItem::new(format!(
        "  FSM signals: {} / states: {}",
        c.fsm_signals, c.fsm_states
    )));
    items.push(ListItem::new(format!(
        "  Covergroups: {}",
        c.covergroups.len()
    )));
    for cg in &c.covergroups {
        items.push(ListItem::new(format!(
            "    {} — {}/{}",
            cg.name, cg.hits, cg.total
        )));
    }
    if items.len() <= 1 {
        items.push(
            ListItem::new("Jalankan simulasi untuk melihat coverage").style(
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            ),
        );
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
