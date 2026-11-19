//! Editor RTL (tengah): menampilkan isi file aktif dengan nomor baris,
//! highlight baris diagnostic, dan pending goto.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::{DiagEntry, GuiState};

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Editor")
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

    let Some(idx) = state.active_file else {
        let p = Paragraph::new("Tidak ada file yang dibuka (Ctrl+O untuk buka project)").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, inner);
        return;
    };

    // Clone file agar tidak borrow state.open_files immutable & mutable secara bersamaan.
    let mut file = match state.open_files.get(idx) {
        Some(f) => f.clone(),
        None => return,
    };

    // Handle pending goto (lompat ke baris).
    if let Some(line) = file.pending_goto.take() {
        state.editor_goto_line = Some(line.max(1));
        if let Some(f) = state.open_files.get_mut(idx) {
            f.pending_goto = None;
        }
    }

    let lines: Vec<&str> = file.content.lines().collect();
    let total = lines.len();

    // Kumpulkan diagnostic per baris untuk file ini.
    let diag_by_line: std::collections::HashMap<usize, &DiagEntry> = state
        .diagnostics
        .iter()
        .filter(|d| {
            std::path::Path::new(&d.file)
                .file_name()
                .and_then(|n| n.to_str())
                == file.path.file_name().and_then(|n| n.to_str())
        })
        .filter_map(|d| if d.line > 0 { Some((d.line, d)) } else { None })
        .collect();

    // Hitung offset scroll sederhana (tidak ada scroll state, tampilkan dari awal).
    let max_start = total.saturating_sub(inner.height as usize);
    let scroll_start = state.editor_scroll.min(max_start);

    let items: Vec<ListItem> = lines
        .iter()
        .enumerate()
        .skip(scroll_start)
        .take(inner.height as usize)
        .map(|(i, line)| {
            let line_no = i + 1;
            let is_target = state
                .editor_goto_line
                .map(|l| l == line_no)
                .unwrap_or(false);
            let diag = diag_by_line.get(&line_no);
            let (icon, color) = match diag {
                Some(d) => match d.level {
                    crate::state::DiagLevel::Error
                    | crate::state::DiagLevel::Bug
                    | crate::state::DiagLevel::Fatal => ("✖", Color::Red),
                    crate::state::DiagLevel::Warning => ("⚠", Color::Yellow),
                    _ => ("ℹ", Color::Blue),
                },
                None => (" ", Color::Gray),
            };

            let line_text = if line.is_empty() {
                " ".to_string()
            } else {
                line.to_string()
            };

            // Batasi lebar line sesuai area.
            let max_w = inner.width.saturating_sub(6) as usize;
            let line_text = if line_text.len() > max_w {
                format!("{}…", &line_text[..max_w.saturating_sub(1)])
            } else {
                line_text
            };

            ListItem::new(format!("{:4} {} {}", line_no, icon, line_text)).style(
                Style::default()
                    .fg(if is_target { Color::Green } else { color })
                    .add_modifier(if is_target {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            )
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);

    // Footer info.
    let info = Paragraph::new(format!(
        "File: {}  |  {} baris  |  {} baris visible  |  Goto: {:?}",
        file.name,
        total,
        inner.height.saturating_sub(1),
        state.editor_goto_line
    ))
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        info,
        ratatui::layout::Rect {
            x: inner.x,
            y: inner.y.saturating_add(inner.height.saturating_sub(1)),
            width: inner.width,
            height: 1,
        },
    );
}
