//! Tab Formal Verification: tampilkan laporan BMC (assertion checking) dan
//! konektivitas yang dihasilkan melalui `mivon --firmal` di dalam GUI.
//! Logika formal tinggal di crate `mivon-formal`; GUI hanya menjalankan
//! engine dan menampilkan laporan teksnya.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Formal Verification")
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

    let Some(rep) = &state.formal_report else {
        let p = Paragraph::new(
            "Belum ada laporan formal verification (tekan F12 untuk menjalankan BMC pada design ter-elaborasi)",
        )
        .style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    };

    let mut items: Vec<ListItem> = Vec::new();

    // Ringkasan status.
    let status_color = if rep.ok { Color::Green } else { Color::LightRed };
    items.push(ListItem::new(format!(
        "Formal (BMC): {}  |  {}/{} assertion lulus  |  {} total",
        if rep.ok { "LULUS" } else { "ADA GAGAL" },
        rep.passed,
        rep.total,
        rep.total,
    ))
    .style(Style::default().fg(status_color).add_modifier(Modifier::BOLD)));
    items.push(ListItem::new("").style(Style::default()));

    // Detail per assertion.
    items.push(ListItem::new("Assertion Results:").style(
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    let max = inner.height.saturating_sub(8) as usize;
    for (name, status) in rep.results.iter().take(max) {
        let (tag, color) = if status.starts_with("PASS") || status.starts_with("INDUCTIVE") {
            ("✓", Color::Green)
        } else if status.starts_with("FAIL") {
            ("✗", Color::LightRed)
        } else if status.starts_with("UNKNOWN") {
            ("?", Color::Yellow)
        } else {
            ("!", Color::Red)
        };
        items.push(ListItem::new(format!("  {} {} — {}", tag, name, status)).style(
            Style::default().fg(color),
        ));
    }
    if rep.results.len() > max {
        items.push(ListItem::new(format!("  … dan {} lagi", rep.results.len() - max)).style(
            Style::default().fg(Color::DarkGray),
        ));
    }
    items.push(ListItem::new("").style(Style::default()));

    // Konektivitas.
    if !rep.connectivity.is_empty() {
        items.push(ListItem::new("Connectivity Check:").style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        let maxc = inner.height.saturating_sub(items.len() as u16) as usize;
        for (src, dst, status) in rep.connectivity.iter().take(maxc) {
            let tag = if status.contains("CONNECTED") { "✓" } else { "✗" };
            let color = if status.contains("CONNECTED") { Color::Green } else { Color::LightRed };
            items.push(ListItem::new(format!("  {} {} → {} ({})", tag, src, dst, status)).style(
                Style::default().fg(color),
            ));
        }
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
