//! Tab Synthesis: tampilkan laporan sintesis (SYN check, utilisasi, FSM,
//! static timing, area) yang dihasilkan dalam GUI melalui `mivon synth`.
//! Logika sintesis tinggal di crate `mivon-synth` / `mivon-timing`; GUI
//! hanya menjalankan pipeline dan menampilkan laporan teksnya.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Synthesis")
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

    let Some(rep) = &state.synth_report else {
        let p = Paragraph::new(
            "Belum ada laporan sintesis (tekan F11 untuk menjalankan sintesis pada design ter-elaborasi)",
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
        "Sintesis: {}  |  {}\n",
        if rep.ok { "BERHASIL" } else { "ADA MASALAH" },
        if rep.ok { "design sintesizable" } else { "periksa laporan SYN di bawah" }
    ))
    .style(Style::default().fg(status_color).add_modifier(Modifier::BOLD)));

    // SYN check.
    items.push(ListItem::new("── SYN Check ──").style(
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    for line in rep.syn_check.lines().take(12) {
        items.push(ListItem::new(format!("  {}", line)).style(Style::default().fg(Color::White)));
    }
    items.push(ListItem::new("").style(Style::default()));

    // Utilisasi.
    items.push(ListItem::new("── Utilisasi ──").style(
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    for line in rep.util.lines().take(12) {
        items.push(ListItem::new(format!("  {}", line)).style(Style::default().fg(Color::White)));
    }
    items.push(ListItem::new("").style(Style::default()));

    // FSM.
    items.push(ListItem::new("── FSM ──").style(
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    for line in rep.fsm.lines().take(10) {
        items.push(ListItem::new(format!("  {}", line)).style(Style::default().fg(Color::White)));
    }
    items.push(ListItem::new("").style(Style::default()));

    // Timing.
    items.push(ListItem::new("── Static Timing ──").style(
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    for line in rep.timing.lines().take(10) {
        items.push(ListItem::new(format!("  {}", line)).style(Style::default().fg(Color::White)));
    }
    items.push(ListItem::new("").style(Style::default()));

    // Area.
    items.push(ListItem::new("── Area ──").style(
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    for line in rep.area.lines().take(10) {
        items.push(ListItem::new(format!("  {}", line)).style(Style::default().fg(Color::White)));
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
