//! Tab Trace: event timeline perubahan nilai sinyal.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!("Trace (filter: '{}')", state.trace_filter))
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

    // Bangun event dari waveform (skip entry t=0).
    let mut events: Vec<(u64, String, String)> = Vec::new();
    for s in &state.waveform {
        for (i, (t, v)) in s.trace.iter().enumerate() {
            if i == 0 {
                continue;
            }
            if !state.trace_filter.is_empty()
                && !s
                    .name
                    .to_lowercase()
                    .contains(&state.trace_filter.to_lowercase())
            {
                continue;
            }
            events.push((*t, s.name.clone(), v.clone()));
        }
    }
    events.sort_by_key(|(t, _, _)| *t);

    if events.is_empty() {
        let p = Paragraph::new("Tidak ada event trace").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    }

    let items: Vec<ListItem> = events
        .iter()
        .rev()
        .take(inner.height as usize)
        .map(|(t, name, value)| {
            ListItem::new(format!("t={:<8} {:<30} = {}", t, name, value))
                .style(Style::default().fg(Color::Yellow))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}
