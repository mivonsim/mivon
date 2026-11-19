//! Tab Pipeline: timing per tahap compile + MICD stats.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Compile Pipeline")
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
        ListItem::new("Pipeline stages").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );

    let stages = match &state.compile_info {
        Some(ci) => &ci.pipeline,
        None => &Vec::new(),
    };
    for s in stages {
        let color = match s.status.as_str() {
            "ok" => Color::Green,
            "running" => Color::Cyan,
            _ => Color::Gray,
        };
        items.push(
            ListItem::new(format!("  {:<14} {:>6} ms  [{}]", s.name, s.ms, s.status))
                .style(Style::default().fg(color)),
        );
    }

    items.push(ListItem::new(""));
    items.push(
        ListItem::new("node_cache (incremental compile cache)").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    match &state.compile_info {
        Some(ci) => match &ci.micd {
            Some(m) => {
                items.push(ListItem::new(format!(
                    "  Present: {}  Files: {}",
                    m.present, m.files
                )));
                items.push(ListItem::new(format!(
                    "  Restored AST: {}  Changed: {}",
                    m.restored_ast, m.changed_files
                )));
                items.push(ListItem::new(format!(
                    "  Verify hits: {}  Misses: {}",
                    m.verify_hits, m.verify_misses
                )));
                items.push(ListItem::new(format!(
                    "  Snapshots: {}  DB bytes: {}",
                    m.snapshots, m.db_bytes
                )));
            }
            None => {
                items.push(
                    ListItem::new("  (no MICD data)").style(Style::default().fg(Color::Gray)),
                );
            }
        },
        None => {
            items.push(ListItem::new("  (compile dulu)").style(Style::default().fg(Color::Gray)));
        }
    }

    if items.len() <= 1 {
        items.push(ListItem::new("(kosong)").style(Style::default().fg(Color::Gray)));
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
