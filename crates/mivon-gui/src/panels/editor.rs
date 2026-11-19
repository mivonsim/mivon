//! Editor RTL (tengah): tampilkan konten file aktif dengan nomor baris,
//! highlight baris diagnostic, dan pending goto. Scroll sederhana.
//!
//! Performa: render hanya baris yang terlihat, tidak clone seluruh isi file,
//! dan syntax highlight dieksekusi per baris visible (bukan seluruh file)
//! tiap frame. Konten file dipinjam (borrow) dari state, tidak dikloning.
//!
//! Split editor: bila `state.editor_split` aktif, area editor dibagi menjadi
//! dua panel (Vertical = kolom samping, Horizontal = baris bawah) dengan
//! masing-masing active file, cursor, dan scroll sendiri.

use ratatui::{
    layout::{Constraint, Direction},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::{DiagEntry, DiagLevel, GuiState, SplitDirection};
use super::super::syntax::highlight_line;

/// Render satu panel editor dengan file index, cursor, dan scroll sendiri.
fn render_editor_panel(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: &GuiState,
    file_idx: Option<usize>,
    cursor_line: usize,
    cursor_col: usize,
    scroll: usize,
    title_suffix: &str,
    is_focused: bool,
) {
    let mode = match state.editor_mode {
        super::super::state::EditorMode::Insert => "INSERT",
        super::super::state::EditorMode::Normal => "NORMAL",
    };
    let block = Block::default()
        .title(format!(
            "Editor [{}]  L{}:C{}{}",
            mode,
            cursor_line + 1,
            cursor_col + 1,
            title_suffix
        ))
        .borders(Borders::ALL)
        .style(if is_focused {
            Style::default().bg(Color::Black).fg(Color::White)
        } else {
            Style::default().bg(Color::Black).fg(Color::DarkGray)
        });
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 1 {
        return;
    }

    let Some(idx) = file_idx else {
        let p = Paragraph::new("Tidak ada file yang dibuka (Ctrl+O untuk buka project)")
            .style(Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC));
        f.render_widget(p, inner);
        return;
    };

    let file = match state.open_files.get(idx) {
        Some(f) => f,
        None => return,
    };

    // Konsumsi pending_goto hanya pada panel utama (active_file).
    let pending = if is_focused {
        file.pending_goto.as_ref().copied()
    } else {
        None
    };
    if let Some(line) = pending {
        // Dipakai untuk set cursor di panel utama.
        drop(pending);
    }

    // Ambil baris sebagai iterator &str (borrow, tidak alokasi Vec penuh).
    let total = file.content.lines().count();

    // Kumpulkan diagnostic hanya untuk file ini, satu kali per render.
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

    // Hitung offset scroll.
    let view_height = inner.height.saturating_sub(1) as usize;
    let max_start = total.saturating_sub(view_height);
    let scroll_start = scroll.min(max_start);

    // Render hanya baris yang masuk layar.
    let mut items: Vec<ListItem> = Vec::with_capacity(view_height);
    for (offset, line) in file.content.lines().skip(scroll_start).take(view_height).enumerate() {
        let line_no = scroll_start + offset + 1;
        let is_target = false; // goto hanya dipakai panel utama
        let diag = diag_by_line
            .iter()
            .find(|(l, _)| *l == line_no)
            .map(|(_, d)| *d);

        let (icon, _) = match diag {
            Some(d) => match d.level {
                DiagLevel::Error | DiagLevel::Bug | DiagLevel::Fatal => ("✖", Color::Red),
                DiagLevel::Warning => ("⚠", Color::Yellow),
                _ => ("ℹ", Color::Blue),
            },
            None => (" ", Color::Gray),
        };

        // Batasi lebar line sesuai area (aman multibyte) tanpa alokasi Vec<char>
        // untuk setiap baris — gunakan char count dan char boundary untuk
        // truncate agar tidak panic pada karakter multibyte (CJK/emoji).
        let max_w = inner.width.saturating_sub(7) as usize;
        let line_text = truncate_line(line, max_w);

        let is_cursor = line_no == cursor_line + 1;

        // ── Nomor baris + ikon diagnostic ──
        let line_no_style = if is_target {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else if is_cursor {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else if diag.is_some() && diag.unwrap().level == DiagLevel::Error {
            Style::default().fg(Color::LightRed)
        } else if diag.is_some() && diag.unwrap().level == DiagLevel::Warning {
            Style::default().fg(Color::LightYellow)
        } else {
            Style::default().fg(Color::Gray)
        };
        let mut spans: Vec<Span> = Vec::with_capacity(4);
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

        items.push(ListItem::new(Line::from(spans)));
    }

    let list = List::new(items);
    f.render_widget(list, inner);

    // Footer info file.
    let mut footer = format!(
        "File: {}  |  {} baris  |  scroll={}",
        file.name,
        total,
        scroll_start
    );
    if state.editor_split && is_focused {
        footer.push_str("  [SPLIT]");
    }
    let info = Paragraph::new(footer).style(Style::default().fg(Color::DarkGray));
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

/// Potong `line` paling banyak `max_w` karakter, potong pada char boundary
/// dan tambahkan '…' bila dipotong. Aman untuk multibyte (CJK/emoji).
fn truncate_line(line: &str, max_w: usize) -> String {
    if max_w == 0 {
        return String::new();
    }
    if line.chars().count() <= max_w {
        return line.to_string();
    }
    let cut = line
        .char_indices()
        .nth(max_w)
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    let mut out = line[..cut].to_string();
    out.push('…');
    out
}

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    if !state.editor_split {
        // Single editor panel.
        render_editor_panel(
            f,
            area,
            state,
            state.active_file,
            state.editor_cursor_line,
            state.editor_cursor_col,
            state.editor_scroll,
            "",
            true,
        );
        return;
    }

    // Split editor: bagi area menjadi dua panel.
    let (left_area, right_area) = match state.split_direction {
        SplitDirection::Vertical => {
            let chunks = ratatui::layout::Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            (chunks[0], chunks[1])
        }
        SplitDirection::Horizontal => {
            let chunks = ratatui::layout::Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            (chunks[0], chunks[1])
        }
    };

    // Panel kiri = active_file utama, panel kanan = split_active_file.
    render_editor_panel(
        f,
        left_area,
        state,
        state.active_file,
        state.editor_cursor_line,
        state.editor_cursor_col,
        state.editor_scroll,
        " (kiri)",
        true,
    );
    render_editor_panel(
        f,
        right_area,
        state,
        state.split_active_file,
        state.split_cursor_line,
        state.split_cursor_col,
        state.split_scroll,
        " (kanan)",
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_line_multibyte_safe() {
        // "ab" (2 char) + "中" (1 char, 3 byte) + "de" (2 char) = 5 char, 7 byte.
        let line = "ab中de";
        let out = truncate_line(line, 3);
        assert_eq!(out, "ab中…"); // 3 char lalu elipsis
        // Harus valid UTF-8: truncate pada char boundary, tidak panic.
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn truncate_line_no_overflow_when_short() {
        let out = truncate_line("hello", 10);
        assert_eq!(out, "hello");
    }

    #[test]
    fn truncate_line_ascii() {
        let out = truncate_line("1234567890", 5);
        assert_eq!(out, "12345…"); // 5 char lalu elipsis
    }

    #[test]
    fn truncate_line_zero_width() {
        assert_eq!(truncate_line("abc", 0), "");
    }
}
