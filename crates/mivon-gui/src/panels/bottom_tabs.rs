//! Panel bawah: tab Problems, Console, Signals, Waveform, Trace, Benchmark,
//! Coverage, Assertions, Terminal, Pipeline.

use ratatui::{
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

use super::super::state::{BottomTab, GuiState};
use super::{
    assertions, benchmark, console, coverage, debugger, pipeline, problems, regression, signals,
    synthesis, terminal, trace, waveform,
};
use crate::theme::Theme;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let theme: Theme = state.theme;
    let block = Block::default()
        .title("Bottom Panel")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.bg()).fg(theme.fg()));
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
        "Debugger",
        "Regression",
        "Synthesis",
    ];
    let tab_items: Vec<ListItem> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let active = state.bottom_tab == BottomTab::from_index(i);
            ListItem::new(format!("  {}", t)).style(if active {
                Style::default()
                    .bg(theme.active_bg())
                    .fg(theme.active_fg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg())
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
        BottomTab::Debugger => debugger::show(f, content_area, state),
        BottomTab::Regression => regression::show(f, content_area, state),
        BottomTab::Synthesis => synthesis::show(f, content_area, state),
    }
}
