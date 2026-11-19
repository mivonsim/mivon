//! Tab Architecture: hierarki modul dari hasil compile.

use ratatui::{
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Architecture")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 1 {
        return;
    }

    let Some(info) = &state.compile_info else {
        let p = Paragraph::new("Compile dulu untuk melihat architecture")
            .style(Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC));
        f.render_widget(p, inner);
        return;
    };

    let mut items: Vec<ListItem> = Vec::new();
    items.push(ListItem::new("Modules").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    for m in &info.modules {
        items.push(ListItem::new(format!("  {}", m)).style(Style::default().fg(Color::White)));
    }
    if items.len() <= 1 {
        items.push(ListItem::new("(belum ada data)").style(Style::default().fg(Color::Gray)));
    }

    // ── Referensi (reverse dependency): modul yang meng-instansiasi ──
    if !info.rev_deps.is_empty() {
        items.push(ListItem::new("").style(Style::default()));
        items.push(ListItem::new("References (modul yang instantiate):").style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        for rd in &info.rev_deps {
            let parents: String = rd
                .children
                .iter()
                .map(|(p, c)| format!("{}×{}", p, c))
                .collect::<Vec<_>>()
                .join(", ");
            items.push(ListItem::new(format!("  {} ← {}", rd.module, parents)).style(
                Style::default().fg(Color::White),
            ));
        }
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
