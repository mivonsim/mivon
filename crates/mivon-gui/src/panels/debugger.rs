//! Tab Debugger: daftar breakpoint, watchpoint, dan event debug.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!(
            "Debugger  {}",
            if state.debug_mode { "[DEBUG ON]" } else { "[DEBUG OFF]" }
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 3 {
        return;
    }

    // Layout: breakpoints (atas), watchpoints (tengah), events (bawah).
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(6),
            ratatui::layout::Constraint::Length(6),
            ratatui::layout::Constraint::Min(1),
        ])
        .split(inner);

    // ── Breakpoints ──
    let bp_items: Vec<ListItem> = if state.breakpoints.is_empty() {
        vec![ListItem::new("(tidak ada breakpoint)").style(
            Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
        )]
    } else {
        state
            .breakpoints
            .iter()
            .map(|bp| {
                ListItem::new(format!("● {}", bp)).style(
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )
            })
            .collect()
    };
    let bp_list = List::new(bp_items).block(
        Block::default()
            .title("Breakpoints")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Red)),
    );
    f.render_widget(bp_list, chunks[0]);

    // ── Watchpoints ──
    let wp_items: Vec<ListItem> = if state.watchpoints.is_empty() {
        vec![ListItem::new("(tidak ada watchpoint)").style(
            Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
        )]
    } else {
        state
            .watchpoints
            .iter()
            .map(|wp| {
                ListItem::new(format!("◐ {}", wp)).style(
                    Style::default().fg(Color::Yellow),
                )
            })
            .collect()
    };
    let wp_list = List::new(wp_items).block(
        Block::default()
            .title("Watchpoints")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(wp_list, chunks[1]);

    // ── Debug events ──
    let ev_items: Vec<ListItem> = state
        .debug_events
        .iter()
        .rev()
        .take(chunks[2].height as usize)
        .map(|ev| {
            ListItem::new(format!("> {}", ev)).style(Style::default().fg(Color::Cyan))
        })
        .collect();
    let ev_list = List::new(ev_items).block(
        Block::default()
            .title("Debug Events")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan)),
    );
    f.render_widget(ev_list, chunks[2]);

    // Hint.
    let hint = Paragraph::new("Tambah breakpoint: 'break cycle N' / 'break change <sinyal>'  |  Watch: '<sinyal>'")
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
