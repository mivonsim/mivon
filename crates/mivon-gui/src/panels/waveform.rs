//! Tab Waveform: renderer gelombang VCD di terminal.
//! Tampilkan trace per signal sebagai bar karakter (█ tinggi, ░ rendah,
//! ▧ x, ▦ z) dengan time ruler di atas dan zoom horizontal.

use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::super::state::GuiState;

/// Karakter untuk nilai sinyal per sel waktu.
fn signal_char(value: &str, width: usize) -> char {
    let v = value.trim();
    if v.is_empty() {
        return ' ';
    }
    let first = v.chars().next().unwrap_or('0');
    if width == 1 {
        match first {
            '1' | 'H' => '█',
            '0' | 'L' => '░',
            'x' | 'X' => '▧',
            'z' | 'Z' => '▦',
            _ => '░',
        }
    } else {
        // Multi-bit: tampilkan sebagai blok terpadu berdasarkan first bit.
        match first {
            '1' => '█',
            '0' => '░',
            'x' | 'X' => '▧',
            'z' | 'Z' => '▦',
            _ => '░',
        }
    }
}

/// Render satu baris waveform: nama signal di kiri, trace di kanan.
/// `cursor_col`: kolom tempat kursor waktu ditandai '|' (bila dalam rentang).
fn waveform_row(
    name: &str,
    trace: &[(u64, String)],
    width: usize,
    t_start: u64,
    t_end: u64,
    col_w: usize,
    cursor_col: usize,
) -> ListItem<'static> {
    let trace_w = col_w.saturating_sub(40); // sisa untuk trace
    if trace_w == 0 {
        return ListItem::new(name.to_string()).style(Style::default().fg(Color::Cyan));
    }

    // Bangun string trace: tiap sel = satu interval waktu.
    let mut chars: Vec<char> = Vec::with_capacity(trace_w);
    let step = if t_end > t_start {
        (t_end - t_start).max(1) / trace_w as u64
    } else {
        1
    };
    let step = step.max(1);

    for col in 0..trace_w {
        // Kursor waktu menimpa karakter sinyal pada kolomnya.
        if col == cursor_col {
            chars.push('|');
            continue;
        }
        let t = t_start + col as u64 * step;
        // Cari nilai terbaru <= t.
        let mut val = "0";
        for (tt, v) in trace {
            if *tt <= t {
                val = v;
            } else {
                break;
            }
        }
        chars.push(signal_char(val, width));
    }

    let trace_str: String = chars.into_iter().collect();
    ListItem::new(format!("{:<36} {}", name, trace_str))
        .style(Style::default().fg(Color::Cyan))
}

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title(format!(
            "Waveform ({} signal)  zoom={}x  {}",
            state.waveform.len(),
            state.wave_zoom as u32,
            if state.wave_compare { "[COMPARE]" } else { "" }
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 3 {
        return;
    }

    if state.waveform.is_empty() {
        let p = Paragraph::new("Tidak ada waveform — jalankan simulasi dulu (F5)")
            .style(
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            );
        f.render_widget(p, inner);
        return;
    }

    // Layout: ruler (1 baris) + trace area.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    // ── Time ruler ──
    let t_max = state
        .waveform
        .iter()
        .flat_map(|s| s.trace.iter())
        .map(|(t, _)| *t)
        .max()
        .unwrap_or(0);
    let t_start = 0u64;
    let t_end = t_max.max(1);
    let ruler_w = chunks[0].width as usize;
    let step = (t_end - t_start).max(1) / ruler_w as u64;
    let step = step.max(1);
    let mut ruler = String::new();
    for col in 0..ruler_w {
        let t = t_start + col as u64 * step;
        if col % 10 == 0 {
            ruler.push_str(&format!("{:>width$}", t, width = 1));
        } else {
            ruler.push('─');
        }
    }
    // Potong/perpanjang ruler agar pas lebar area.
    if ruler.len() > ruler_w {
        ruler.truncate(ruler_w);
    }
    while ruler.len() < ruler_w {
        ruler.push('─');
    }
    let ruler_p = Paragraph::new(ruler)
        .style(Style::default().fg(Color::Gray).add_modifier(Modifier::DIM));
    f.render_widget(ruler_p, chunks[0]);

    // ── Trace area ──
    let trace_w = chunks[1].width as usize;
    // Posisi kursor waktu di kolom (0-based), dipetakan dari wave_cursor.
    let span = t_end.saturating_sub(t_start).max(1);
    let cursor_col = if span > 0 {
        ((state.wave_cursor.saturating_sub(t_start) as u128)
            .checked_mul(trace_w as u128)
            .unwrap_or(u128::MAX)
            / span as u128) as usize
    } else {
        0
    }
    .min(trace_w.saturating_sub(1));
    let mut items: Vec<ListItem> = Vec::new();
    let max_rows = chunks[1].height as usize;
    for s in state.waveform.iter().take(max_rows) {
        items.push(waveform_row(
            &s.name,
            &s.trace,
            s.width,
            t_start,
            t_end,
            trace_w,
            cursor_col,
        ));
    }

    let list = List::new(items);
    f.render_widget(list, chunks[1]);

    // Hint kontrol + bacaan waktu kursor.
    let hint = Paragraph::new(format!(
        "Zoom: state.wave_zoom  |  Filter: trace_filter  |  Compare: wave_compare  |  Kursor t={} (,/. navigasi)",
        state.wave_cursor
    ))
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        hint,
        ratatui::layout::Rect {
            x: inner.x,
            y: inner.y.saturating_add(inner.height.saturating_sub(1)),
            width: inner.width,
            height: 1,
        },
    );
}
