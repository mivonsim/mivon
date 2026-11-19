//! Tab Benchmark: metrik performa simulasi terakhir.

use ratatui::{
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

use super::super::state::GuiState;
use ratatui::style::Modifier;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Benchmark")
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
        ListItem::new("Performance (last run)").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    items.push(ListItem::new(format!("  Cycles: {}", state.cycles)));
    items.push(ListItem::new(format!(
        "  Sim time: {:.2} ms",
        state.sim_time_ms
    )));
    items.push(ListItem::new(format!(
        "  Delta cycles: {}",
        state.delta_cycles
    )));
    items.push(ListItem::new(format!(
        "  Events processed: {}",
        state.events_processed
    )));
    items.push(ListItem::new(format!(
        "  Processes evaluated: {}",
        state.processes_evaluated
    )));
    items.push(ListItem::new(format!(
        "  NBA commits: {}",
        state.nba_commits
    )));
    items.push(ListItem::new(format!(
        "  Events/delta: {:.2}",
        state.events_per_delta
    )));
    items.push(ListItem::new(""));
    let list = List::new(items);
    f.render_widget(list, inner);
}
