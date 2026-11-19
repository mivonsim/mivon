//! Tab Regression: tampilkan database regression (`.mivon/regression.json`,
//! CLI `mregression`) — run terbaru, pass/fail/skip/flaky, tren, dan daftar
//! tes flaky. Logika analisis regression tetap di CLI; GUI hanya menampilkan.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;
use super::super::regression::TestStatus;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Regression")
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

    let Some(db) = &state.regression_db else {
        let p = Paragraph::new("Belum ada database regression (jalankan `mregression` CLI)\nContoh: mivon mregression run config.toml")
            .style(
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            );
        f.render_widget(p, inner);
        return;
    };

    let mut items: Vec<ListItem> = Vec::new();

    // Ringkasan run terbaru.
    if let Some(r) = db.latest() {
        let skip = r
            .results
            .iter()
            .filter(|x| x.status == TestStatus::Skip)
            .count();
        let flaky = r
            .results
            .iter()
            .filter(|x| x.status == TestStatus::Flaky)
            .count();
        items.push(ListItem::new(format!(
            "Run terbaru: {}  |  commit {}  |  {} tes",
            r.id, r.commit, r.results.len()
        ))
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
        items.push(ListItem::new(format!(
            "  pass {}  |  fail {}  |  skip {}  |  flaky {}  |  pass rate {:.1}%",
            r.pass_count(),
            r.fail_count(),
            skip,
            flaky,
            r.pass_rate()
        ))
        .style(Style::default().fg(Color::White)));
        items.push(ListItem::new(format!(
            "  total durasi: {} ms  |  total run: {}",
            r.total_duration_ms,
            db.run_count()
        ))
        .style(Style::default().fg(Color::Gray)));
        items.push(ListItem::new("").style(Style::default()));
    } else {
        items.push(ListItem::new("Belum ada run").style(Style::default().fg(Color::Gray)));
        items.push(ListItem::new("").style(Style::default()));
    }

    // Tes flaky.
    let flaky = db.flaky_tests();
    if !flaky.is_empty() {
        items.push(ListItem::new("Tes flaky:").style(
            Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD),
        ));
        for (name, rate) in flaky.iter().take(inner.height.saturating_sub(4) as usize) {
            items.push(ListItem::new(format!("  {}  ({:.1}%)", name, rate)).style(
                Style::default().fg(Color::LightRed),
            ));
        }
        items.push(ListItem::new("").style(Style::default()));
    }

    // Detail hasil run terbaru (nama + status).
    if let Some(r) = db.latest() {
        items.push(ListItem::new("Hasil run terbaru:").style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        let remaining = inner.height.saturating_sub(items.len() as u16) as usize;
        for res in r.results.iter().take(remaining.saturating_sub(1)) {
            let (tag, color) = match res.status {
                TestStatus::Pass => ("✔", Color::Green),
                TestStatus::Fail => ("✖", Color::Red),
                TestStatus::Skip => ("○", Color::Gray),
                TestStatus::Flaky => ("~", Color::Yellow),
            };
            let line = format!("  {} {}  ({} ms)", tag, res.name, res.duration_ms);
            items.push(ListItem::new(line).style(Style::default().fg(color)));
            if let Some(e) = &res.error_msg {
                items.push(ListItem::new(format!("      {}", e)).style(
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
