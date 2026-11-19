//! Editor RTL (tengah): tampilkan konten file aktif dengan nomor baris,
//! highlight baris diagnostic, dan pending goto. Scroll sederhana.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::{DiagEntry, DiagLevel, GuiState};
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
        let p = Paragraph::new("Tidak ada file yang dibuka (Ctrl+O untuk buka project)")
            .style(Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC));
        f.render_widget(p, inner);
        return;
    };

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

    // Kumpulkan diagnostic per baris untuk file ini (baca state, tidak borrow mutable).
    let file_name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let diag_by_line: Vec<(usize, &DiagEntry)> = state
        .diagnostics
        .iter()
        .filter(|d| {
            std::path::Path::new(&d.file)
                .file_name()
                .and_then(|n| n.to_str())
                == Some(file_name)
        })
        .filter_map(|d| if d.line > 0 { Some((d.line, d)) } else { None })
        .collect();

    // Hitung offset scroll sederhana.
    let max_start = total.saturating_sub(inner.height.saturating_sub(1) as usize);
    let scroll_start = state.editor_scroll.min(max_start);

    let view_height = inner.height.saturating_sub(1) as usize;
    let items: Vec<ListItem> = lines
        .iter()
        .enumerate()
        .skip(scroll_start)
        .take(view_height)
        .map(|(i, line)| {
            let line_no = i + 1;
            let is_target = state.editor_goto_line.map(|l| l == line_no).unwrap_or(false);
            let diag = diag_by_line.iter().find(|(l, _)| *l == line_no).map(|(_, d)| *d);

            let (icon, color) = match diag {
                Some(d) => match d.level {
                    DiagLevel::Error | DiagLevel::Bug | DiagLevel::Fatal => {
                        ("✖", Color::Red)
                    }
                    DiagLevel::Warning => ("⚠", Color::Yellow),
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
            let max_w = inner.width.saturating_sub(7) as usize;
            let line_text = if line_text.len() > max_w {
                format!("{}…", &line_text[..max_w.saturating_sub(1)])
            } else {
                line_text
            };

            ListItem::new(format!("{:4} {} {}", line_no, icon, line_text)).style(
                Style::default()
                    .fg(if is_target { Color::Green } else { color })
                    .add_modifier(if is_target {
                        Modifier::BOLD | Modifier::REVERSED
                    } else {
                        Modifier::empty()
                    }),
            )
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);

    // Footer info file.
    let info = Paragraph::new(format!(
        "File: {}  |  {} baris  |  scroll={}  |  goto={:?}",
        file.name,
        total,
        scroll_start,
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
