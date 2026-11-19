//! Panel bawah: tab Problems, Console, Signals, Waveform, Trace, Benchmark,
//! Coverage, Assertions, Terminal, Pipeline.

use ratatui::{
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

use super::super::state::{BottomTab, GuiState};
use super::{
    assertions, benchmark, console, coverage, pipeline, problems, signals, terminal, trace,
    waveform,
};
use ratatui::style::Modifier;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Bottom Panel")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::DarkGray));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 3 {
        return;
    }

    // Tab selector.
    let tabs = [
        "Problems",
        "Console",
        "Signals",
        "Waveform",
        "Trace",
        "Benchmark",
        "Coverage",
        "Assertions",
        "Terminal",
        "Pipeline",
    ];
    let tab_items: Vec<ListItem> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let active = state.bottom_tab == BottomTab::from_index(i);
            ListItem::new(format!("  {}", t)).style(if active {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect();
    let tabs_list = List::new(tab_items).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(tabs_list, inner);

    let content_area = ratatui::layout::Rect {
        x: inner.x,
        y: inner.y.saturating_add(tabs.len() as u16 + 1),
        width: inner.width,
        height: inner.height.saturating_sub(tabs.len() as u16 + 1),
    };

    match state.bottom_tab {
        BottomTab::Problems => problems::show(f, content_area, state),
        BottomTab::Console => console::show(f, content_area, state),
        BottomTab::Signals => signals::show(f, content_area, state),
        BottomTab::Waveform => waveform::show(f, content_area, state),
        BottomTab::Trace => trace::show(f, content_area, state),
        BottomTab::Benchmark => benchmark::show(f, content_area, state),
        BottomTab::Coverage => coverage::show(f, content_area, state),
        BottomTab::Assertions => assertions::show(f, content_area, state),
        BottomTab::Terminal => terminal::show(f, content_area, state),
        BottomTab::Pipeline => pipeline::show(f, content_area, state),
    }
}
