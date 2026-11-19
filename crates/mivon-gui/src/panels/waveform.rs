//! Tab Waveform: daftar signal hasil simulasi (trace dari VCD).

use ratatui::{
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
use ratatui::style::Modifier;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!("Waveform ({} signal)", state.waveform.len()))
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

    if state.waveform.is_empty() {
        let p = Paragraph::new("Tidak ada waveform — jalankan simulasi dulu").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    }

    let items: Vec<ListItem> = state
        .waveform
        .iter()
        .take(inner.height as usize)
        .map(|s| {
            let last = s.trace.last().map(|(_, v)| v.as_str()).unwrap_or("-");
            ListItem::new(format!("{:<30} width={:<4} last={}", s.name, s.width, last))
                .style(Style::default().fg(Color::Green))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}
