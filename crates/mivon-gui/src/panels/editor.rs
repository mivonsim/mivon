//! Editor RTL (tengah): tampilkan konten file aktif dengan nomor baris,
//! highlight baris diagnostic, dan pending goto. Scroll sederhana.
//!
//! Performa: render hanya baris yang terlihat, tidak clone seluruh isi file,
//! dan syntax highlight dieksekusi per baris visible (bukan seluruh file)
//! tiap frame. Konten file dipinjam (borrow) dari state, tidak dikloning.

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

    // Konsumsi pending_goto sekali, lalu hapus dari state agar tidak diproses
    // ulang frame berikutnya. Setiap borrow bersifat sementara dan tidak
    // tumpang tindih dengan borrow immutable `file` di bawah.
    let pending = state
        .open_files
        .get(idx)
        .and_then(|f| f.pending_goto.as_ref().copied());
    if let Some(line) = pending {
        state.editor_goto_line = Some(line.max(1));
        if let Some(f) = state.open_files.get_mut(idx) {
            f.pending_goto = None;
        }
    }

    // Pinjam referensi file (tidak clone seluruh OpenFile / konten).
    let file = match state.open_files.get(idx) {
        Some(f) => f,
        None => return,
    };

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
    let scroll_start = state.editor_scroll.min(max_start);

    // Render hanya baris yang masuk layar.
    let mut items: Vec<ListItem> = Vec::with_capacity(view_height);
    for (offset, line) in file.content.lines().skip(scroll_start).take(view_height).enumerate() {
        let line_no = scroll_start + offset + 1;
        let is_target = state.editor_goto_line.map(|l| l == line_no).unwrap_or(false);
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

        let is_cursor = line_no == state.editor_cursor_line + 1;

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
        "File: {}  |  {} baris  |  scroll={}  |  goto={:?}",
        file.name,
        total,
        scroll_start,
        state.editor_goto_line
    );
    if state.lsp_completion_show && !state.lsp_completion_items.is_empty() {
        let sel = state.lsp_completion_selected.min(state.lsp_completion_items.len() - 1);
        footer.push_str(&format!(
            "  |  Completion: {} ({}/{})",
            state.lsp_completion_items[sel],
            sel + 1,
            state.lsp_completion_items.len()
        ));
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
