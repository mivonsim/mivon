//! Editor RTL (tengah): tampilkan konten file aktif dengan nomor baris,
//! highlight baris diagnostic, dan pending goto. Scroll sederhana.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::{DiagEntry, DiagLevel, GuiState};
use super::super::syntax::highlight_line;
pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let mode = match state.editor_mode {
        super::super::state::EditorMode::Insert => "INSERT",
        super::super::state::EditorMode::Normal => "NORMAL",
    };
    let block = Block::default()
        .title(format!(
            "Editor [{}]  L{}:C{}  (i=insert, Esc=normal)",
            mode,
            state.editor_cursor_line + 1,
            state.editor_cursor_col + 1
        ))
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

            let (icon, _color) = match diag {
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

            // Batasi lebar line sesuai area (aman untuk karakter multibyte).
            let max_w = inner.width.saturating_sub(7) as usize;
            let mut chars: Vec<char> = line_text.chars().collect();
            if chars.len() > max_w && max_w > 0 {
                chars.truncate(max_w.saturating_sub(1));
                chars.push('…');
            }
            let line_text: String = chars.into_iter().collect();

            let is_cursor = i == state.editor_cursor_line;

            // ── Nomor baris + ikon diagnostic ──
            let line_no_style = if is_target {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else if is_cursor {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let mut spans: Vec<Span> = Vec::new();
            spans.push(Span::styled(format!("{:4} ", line_no), line_no_style));

            let icon_style = match diag {
                Some(d) => match d.level {
                    DiagLevel::Error | DiagLevel::Bug | DiagLevel::Fatal => {
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                    }
                    DiagLevel::Warning => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    _ => Style::default().fg(Color::Blue),
                },
                None => Style::default().fg(Color::Gray),
            };
            spans.push(Span::styled(format!("{} ", icon), icon_style));

            // ── Syntax highlight isi baris ──
            for seg in &highlight_line(&line_text) {
                spans.push(Span::styled(seg.text.clone(), seg.style));
            }

            // Tandai kursor dengan span '▏' di awal isi (bila baris aktif).
            if is_cursor {
                spans.insert(
                    2,
                    Span::styled(
                        "▏",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                );
            }

            ListItem::new(Line::from(spans))
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
