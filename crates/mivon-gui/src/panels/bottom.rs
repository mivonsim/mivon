//! Panel bawah: Problems (diagnostics), Console (log), Signals (hasil sim).

use eframe::egui;

use super::super::splitter;
use super::super::state::{BottomTab, DiagEntry, DiagLevel, GuiState};
use super::assertions;
use super::benchmark;
use super::coverage;
use super::pipeline;
use super::terminal;
use super::trace;
use super::waveform;

pub fn show(ui: &mut egui::Ui, state: &mut GuiState) {
    // ── Splitter handle di border atas panel — drag untuk resize ──
    // Kursor berubah jadi ns-resize saat hover; tinggi di-clamp ke
    // [min tab bar, 80% tinggi window] (lihat `splitter::bottom_bounds`).
    let bounds = splitter::bottom_bounds(ui);
    let new_height = splitter::show_resizer(
        ui,
        egui::Id::new("bottom_panel_resizer"),
        state.bottom_height,
        bounds,
    );
    if new_height != state.bottom_height {
        state.bottom_height = new_height;
    }

    // ── Tab selector ──
    ui.horizontal(|ui| {
        let mut clicked: Option<BottomTab> = None;
        for (tab, label) in [
            (BottomTab::Problems, "Problems"),
            (BottomTab::Console, "Console"),
            (BottomTab::Signals, "Signals"),
            (BottomTab::Waveform, "Waveform"),
            (BottomTab::Trace, "Trace"),
            (BottomTab::Benchmark, "Benchmark"),
            (BottomTab::Coverage, "Coverage"),
            (BottomTab::Assertions, "Assertions"),
            (BottomTab::Terminal, "Terminal"),
            (BottomTab::Pipeline, "Pipeline"),
        ] {
            let count = match tab {
                BottomTab::Problems => state.diagnostics.len() + state.lsp_diags.len(),
                BottomTab::Console => state.console.len(),
                BottomTab::Signals => state.signals.len(),
                BottomTab::Waveform => state.waveform.len(),
                BottomTab::Trace => 0,
                BottomTab::Benchmark => 0,
                BottomTab::Coverage => state.coverage.branch_total as usize,
                BottomTab::Assertions => state.assertions.len(),
                BottomTab::Terminal => state.term_lines.len(),
                BottomTab::Pipeline => state
                    .compile_info
                    .as_ref()
                    .map(|c| c.micd.as_ref().map(|m| m.files).unwrap_or(0))
                    .unwrap_or(0),
            };
            let text = egui::RichText::new(format!("{} ({})", label, count)).size(12.0);
            if ui.selectable_label(state.bottom_tab == tab, text).clicked() {
                clicked = Some(tab);
            }
        }
        if let Some(t) = clicked {
            state.bottom_tab = t;
        }
    });
    ui.separator();

    // Konten wajib mengisi seluruh tinggi panel. Tanpa ini egui menciutkan
    // frame panel ke tinggi konten (ScrollArea auto-shrink), sehingga panel
    // kehilangan ukuran yang di-set dan resize handle jadi tidak berfungsi —
    // drag tidak bisa membesarkan panel secara bebas.
    ui.set_min_height(ui.available_height());

    // Waveform & Trace punya ScrollArea sendiri (horizontal+vertikal / vertikal)
    // — jangan dibungkus ScrollArea vertikal di sini (scroll bersarang).
    if matches!(state.bottom_tab, BottomTab::Waveform | BottomTab::Trace) {
        match state.bottom_tab {
            BottomTab::Waveform => waveform::show(ui, state),
            BottomTab::Trace => trace::show(ui, state),
            _ => unreachable!("handled above"),
        }
        return;
    }

    let height = ui.available_height().max(80.0);
    egui::ScrollArea::vertical()
        .id_salt("bottom_scroll")
        .auto_shrink([false, false])
        .max_height(height)
        .show(ui, |ui| match state.bottom_tab {
            BottomTab::Problems => problems_tab(ui, state),
            BottomTab::Console => console_tab(ui, state),
            BottomTab::Signals => signals_tab(ui, state),
            BottomTab::Waveform => unreachable!("handled above"),
            BottomTab::Trace => unreachable!("handled above"),
            BottomTab::Benchmark => benchmark::show(ui, state),
            BottomTab::Coverage => coverage::show(ui, state),
            BottomTab::Assertions => assertions::show(ui, state),
            BottomTab::Terminal => terminal::show(ui, state),
            BottomTab::Pipeline => pipeline::show(ui, state),
        });
}

fn problems_tab(ui: &mut egui::Ui, state: &mut GuiState) {
    // Diagnostics lokal (compile/lint) + LSP server — digabung utk tampil.
    let all: Vec<&DiagEntry> = state
        .diagnostics
        .iter()
        .chain(state.lsp_diags.iter())
        .collect();
    if all.is_empty() {
        ui.label(egui::RichText::new("No problems detected").weak().italics());
        return;
    }
    // Aksi dikumpulkan dulu (borrow `state.diagnostics` immutable selesai)
    // lalu diterapkan SETELAH loop — `state.apply_quick_fix` & `open_file`
    // meminjam `state` mutable, tidak bisa di dalam iterasi.
    let mut goto: Option<(String, usize)> = None;
    let mut fix_idx: Option<usize> = None;
    for (i, d) in all.iter().enumerate() {
        let (icon, color) = match d.level {
            // Fatal/Bug/Error → icon error; lainnya per level UI GUI.
            DiagLevel::Error | DiagLevel::Bug | DiagLevel::Fatal => {
                ("✖", egui::Color32::from_rgb(239, 68, 68))
            }
            DiagLevel::Warning => ("⚠", egui::Color32::from_rgb(234, 179, 8)),
            // Note/Info/Help/Trace/Debug → tampilan info.
            _ => ("ℹ", egui::Color32::from_rgb(59, 130, 246)),
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icon).color(color));
            // Lokasi — klik → buka file di editor & lompat ke baris.
            let loc = format!("{}:{}", d.file, d.line);
            // on_hover_text dipanggil di Response (bukan builder Link) —
            // API egui 0.35 tidak lagi menyediakannya di builder widget.
            let loc_resp = ui
                .add(egui::Link::new(
                    egui::RichText::new(&loc).weak().monospace().size(11.0),
                ))
                .on_hover_text("Buka file & lompat ke baris ini");
            if loc_resp.clicked() {
                goto = Some((d.file.clone(), d.line));
            }
            ui.label(egui::RichText::new(&d.message).size(12.0).color(color));
            // Quick Fix — hanya diagnostic yang punya perbaikan otomatis.
            if let Some(fix) = &d.fix {
                let fix_resp = ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(format!("💡 {}", fix.action)).size(11.0),
                        )
                        .small()
                        .fill(egui::Color32::from_rgb(30, 41, 59)),
                    )
                    .on_hover_text("Terapkan perbaikan otomatis");
                if fix_resp.clicked() {
                    fix_idx = Some(i);
                }
            }
        });
    }
    // Terapkan aksi setelah loop (borrow immutable diagnostics selesai).
    if let Some((file, line)) = goto {
        open_diag_location(state, &file, line);
    }
    if let Some(i) = fix_idx {
        state.apply_quick_fix(i);
    }
}

/// Buka file diagnostic di editor & lompat ke baris target. Lompat dieksekusi
/// ScrollArea editor pada frame berikutnya via `OpenFile.pending_goto`.
fn open_diag_location(state: &mut GuiState, file: &str, line: usize) {
    if file.is_empty() {
        return;
    }
    let path = std::path::PathBuf::from(file);
    state.open_file(path);
    if let Some(idx) = state.active_file {
        if let Some(f) = state.open_files.get_mut(idx) {
            f.pending_goto = Some(line.max(1));
        }
    }
    state.log(format!("→ {}:{}", file, line));
}

fn console_tab(ui: &mut egui::Ui, state: &mut GuiState) {
    if state.console.is_empty() {
        ui.label(egui::RichText::new("Console kosong").weak().italics());
        return;
    }
    for line in &state.console {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&line.time)
                    .weak()
                    .monospace()
                    .size(11.0),
            );
            ui.label(egui::RichText::new(&line.msg).monospace().size(12.0));
        });
    }
}

fn signals_tab(ui: &mut egui::Ui, state: &mut GuiState) {
    if state.signals.is_empty() {
        ui.label(
            egui::RichText::new("Jalankan simulasi untuk melihat signal")
                .weak()
                .italics(),
        );
        return;
    }
    egui::Grid::new("signals_grid")
        .striped(true)
        .num_columns(4)
        .min_col_width(90.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Signal").strong().size(11.0));
            ui.label(egui::RichText::new("Width").strong().size(11.0));
            ui.label(egui::RichText::new("Value (hex)").strong().size(11.0));
            ui.label(egui::RichText::new("Kind").strong().size(11.0));
            ui.end_row();

            for s in &state.signals {
                ui.label(egui::RichText::new(&s.name).monospace().size(12.0));
                ui.label(s.width.to_string());
                ui.label(
                    egui::RichText::new(&s.value)
                        .monospace()
                        .color(egui::Color32::from_rgb(79, 193, 255)),
                );
                ui.label(egui::RichText::new(&s.kind).weak().size(11.0));
                ui.end_row();
            }
        });
}
