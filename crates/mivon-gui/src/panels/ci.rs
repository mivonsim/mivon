//! Tab CI/CD: tampilkan status CI/CD dari file JSON lokal
//! (`.mivon/ci-status.json`) yang dihasilkan oleh CI eksternal.
//! GUI hanya menampilkan status; tidak berkomunikasi langsung dengan CI server.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::ci::CiJobStatus;
use super::super::state::GuiState;

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("CI/CD")
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

    let Some(db) = &state.ci_database else {
        let p = Paragraph::new(
            "Belum ada data CI/CD (jalankan CI eksternal dan simpan hasil ke\n.mivon/ci-status.json, atau muat via command palette)",
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

    // Ringkasan run terbaru.
    if let Some(r) = db.latest() {
        let status_color = match r.status {
            CiJobStatus::Success => Color::Green,
            CiJobStatus::Failure => Color::LightRed,
            CiJobStatus::Running => Color::Yellow,
            CiJobStatus::Pending => Color::Gray,
            CiJobStatus::Skipped => Color::DarkGray,
            CiJobStatus::Canceled => Color::Gray,
        };
        let status_str = match r.status {
            CiJobStatus::Success => "SUKSES",
            CiJobStatus::Failure => "GAGAL",
            CiJobStatus::Running => "BERJALAN",
            CiJobStatus::Pending => "TERTUNDA",
            CiJobStatus::Skipped => "DILEWATI",
            CiJobStatus::Canceled => "DIBATALKAN",
        };
        items.push(ListItem::new(format!(
            "Run: {}  |  {}  |  commit {}  |  branch {}",
            r.id, status_str, r.commit, r.branch
        ))
        .style(Style::default().fg(status_color).add_modifier(Modifier::BOLD)));
        items.push(ListItem::new(format!(
            "Workflow: {}  |  total job: {}",
            r.workflow,
            r.jobs.len()
        ))
        .style(Style::default().fg(Color::White)));
        items.push(ListItem::new("").style(Style::default()));
    } else {
        items.push(ListItem::new("Belum ada run CI").style(Style::default().fg(Color::Gray)));
        items.push(ListItem::new("").style(Style::default()));
    }

    // Status counts.
    let counts = db.status_counts();
    if !counts.is_empty() {
        items.push(ListItem::new("Status Jobs:").style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        let mut status_list: Vec<_> = counts.iter().collect();
        status_list.sort_by_key(|(_, &c)| std::cmp::Reverse(c));
        for (status, count) in status_list.iter().take(6) {
            let color = match status {
                CiJobStatus::Success => Color::Green,
                CiJobStatus::Failure => Color::LightRed,
                CiJobStatus::Running => Color::Yellow,
                _ => Color::Gray,
            };
            items.push(ListItem::new(format!("  {:?}: {}", status, count)).style(
                Style::default().fg(color),
            ));
        }
        items.push(ListItem::new("").style(Style::default()));
    }

    // Job gagal terbaru.
    let failed = db.failed_jobs();
    if !failed.is_empty() {
        items.push(ListItem::new("Job Gagal (terbaru):").style(
            Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD),
        ));
        let max = inner.height.saturating_sub(items.len() as u16) as usize;
        for job in failed.iter().take(max) {
            items.push(ListItem::new(format!("  ✖ {}", job.name)).style(
                Style::default().fg(Color::LightRed),
            ));
            if let Some(e) = &job.error {
                items.push(ListItem::new(format!("      {}", e)).style(
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}
