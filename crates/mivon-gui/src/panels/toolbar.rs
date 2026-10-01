//! Toolbar atas: brand, project name, tombol open/compile/run/stop.

use eframe::egui;

use super::super::state::GuiState;

pub fn show(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.horizontal(|ui| {
        // ── Brand ──
        ui.label(
            egui::RichText::new("Mivon")
                .strong()
                .size(15.0)
                .color(ui.visuals().selection.bg_fill),
        );
        ui.separator();

        // ── Nama proyek ──
        if state.project_name.is_empty() {
            ui.label(egui::RichText::new("No project").weak().italics());
        } else {
            ui.label(egui::RichText::new(&state.project_name).strong());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // ── Max time ──
            ui.label("T=");
            ui.add(
                egui::DragValue::new(&mut state.max_time)
                    .range(1..=100_000_000)
                    .speed(100),
            );

            // ── Run / Stop ──
            if state.is_running {
                if ui
                    .button(
                        egui::RichText::new("⏹ Stop").color(egui::Color32::from_rgb(239, 68, 68)),
                    )
                    .on_hover_text("Stop (F5)")
                    .clicked()
                {
                    super::super::app::trigger_stop(state);
                }
            } else {
                if ui
                    .button(
                        egui::RichText::new("▶ Run").color(egui::Color32::from_rgb(34, 197, 94)),
                    )
                    .on_hover_text("Run simulation (F5)")
                    .clicked()
                {
                    // Aksi di-handle di app.rs via state flag
                    super::super::app::trigger_run(state);
                }
            }

            // ── Compile ──
            if ui
                .button("Compile")
                .on_hover_text("Compile + elaborate (F7)")
                .clicked()
            {
                super::super::app::trigger_compile(state);
            }

            // ── F25: Generate (.mv/.mvh → .sv/.svh) ──
            // Aktif hanya saat file aktif ber-ekstensi .mv/.mvh (Mivon HDL).
            let active_is_mv = state
                .active_file
                .and_then(|i| state.open_files.get(i))
                .map(|f| {
                    matches!(
                        f.path.extension().and_then(|e| e.to_str()),
                        Some("mv" | "mvh")
                    )
                })
                .unwrap_or(false);
            if ui
                .add_enabled(active_is_mv, egui::Button::new("⚙ Generate"))
                .on_hover_text(
                    "Generate SV/SVH dari Mivon HDL (.mv/.mvh) — file aktif (Ctrl+G)",
                )
                .clicked()
            {
                super::super::app::trigger_generate(state);
            }
            // Generate All: transpile semua .mv/.mvh proyek (konteks gabungan).
            let mv_count = state.collect_mv_files().len();
            if ui
                .add_enabled(
                    mv_count > 0,
                    egui::Button::new(egui::RichText::new("⚙ All").weak()),
                )
                .on_hover_text(format!(
                    "Generate semua .mv/.mvh proyek ({} file, konteks gabungan)",
                    mv_count
                ))
                .clicked()
            {
                super::super::app::trigger_generate_all(state);
            }

            // ── Export waveform (VCD) — trace hasil sim terakhir ──
            let has_wave = !state.waveform.is_empty();
            if ui
                .add_enabled(has_wave, egui::Button::new("⇓ VCD"))
                .on_hover_text("Export waveform hasil simulasi ke file VCD")
                .clicked()
            {
                super::super::app::trigger_export_vcd(state);
            }

            // ── Open project ──
            if ui
                .button("Open Project")
                .on_hover_text("Buka folder proyek (Ctrl+O)")
                .clicked()
            {
                super::super::app::trigger_open_project(state);
            }
        });
    });
}
