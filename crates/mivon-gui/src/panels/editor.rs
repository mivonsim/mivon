//! Editor tengah: tab bar file terbuka + CodeEditor (egui_code_editor)
//! dengan syntax highlighting SystemVerilog.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
use egui::TextBuffer;

use super::super::semantic;
use super::super::state::{
    diag_matches_file, word_count, BottomTab, DiagEntry, DiagLevel, GuiState, LspPendingKind,
    OpenFile, PeekInfo, StickyScope,
};

pub fn show(ui: &mut egui::Ui, state: &mut super::super::state::GuiState) {
    // ── Welcome screen ──
    if state.open_files.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Mivon")
                        .size(42.0)
                        .strong()
                        .color(ui.visuals().selection.bg_fill),
                );
                ui.label(
                    egui::RichText::new("RTL Engineering Control Center")
                        .size(14.0)
                        .weak(),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Open project → compile → run simulation")
                        .weak()
                        .italics(),
                );
                ui.add_space(12.0);
                if ui.button("📂 Open Project").clicked() {
                    super::super::app::trigger_open_project(state);
                }
                // ── Recent projects — klik buka langsung ──
                let recents = crate::workspace::load_recents();
                if !recents.is_empty() {
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new("Recent projects").strong().size(12.0));
                    ui.add_space(4.0);
                    let mut to_open: Option<std::path::PathBuf> = None;
                    for p in recents {
                        let label = p
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| p.display().to_string());
                        if ui
                            .button(
                                egui::RichText::new(format!("▸ {}", label))
                                    .monospace()
                                    .size(11.0),
                            )
                            .on_hover_text(p.display().to_string())
                            .clicked()
                        {
                            to_open = Some(p.clone());
                        }
                    }
                    if let Some(p) = to_open {
                        super::super::app::open_project_dir(state, &p);
                    }
                }
            });
        });
        return;
    }

    // ── Tab bar ──
    ui.horizontal(|ui| {
        let mut to_close: Option<usize> = None;
        for (i, f) in state.open_files.iter().enumerate() {
            let active = state.active_file == Some(i);
            let label = if f.dirty {
                format!("● {}", f.name)
            } else {
                f.name.clone()
            };
            let text = egui::RichText::new(label).monospace().size(12.0);
            if ui.selectable_label(active, text).clicked() {
                state.active_file = Some(i);
            }
            // tombol close kecil
            let resp = ui.add(
                egui::Button::new(egui::RichText::new("✕").size(10.0))
                    .frame(false)
                    .small(),
            );
            if resp.clicked() {
                to_close = Some(i);
            }
        }
        if let Some(i) = to_close {
            state.close_file(i);
        }
    });
    ui.separator();

    // ── Editor aktif ──
    let Some(idx) = state.active_file else {
        return;
    };
    let Some(f) = state.open_files.get_mut(idx) else {
        return;
    };

    // ── Breadcrumb interaktif: project › folder › file.sv ──
    // Klik folder → buka tab Project; klik file → salin path lengkap.
    {
        // Salin data yang dibutuhkan dulu — `f` meminjam `state.open_files`
        // secara mut, jadi jangan tangkap `state` utuh di closure breadcrumb
        // (konflik borrow). Flag diterapkan setelah closure.
        let path_str = f.path.display().to_string();
        let project_name = state.project_name.clone();
        let root = state.project_root.clone();
        let mut want_project_tab = false;
        let mut want_copy = false;

        ui.horizontal(|ui| {
            let rel_parts: Vec<String> = root
                .as_ref()
                .and_then(|r| std::path::Path::new(&path_str).strip_prefix(r).ok())
                .map(|p| {
                    p.components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();

            // Segmen pertama: nama proyek
            if !project_name.is_empty() {
                let resp = ui
                    .selectable_label(
                        false,
                        egui::RichText::new(&project_name)
                            .monospace()
                            .size(11.0)
                            .strong(),
                    )
                    .on_hover_text("Buka tab Project");
                if resp.clicked() {
                    want_project_tab = true;
                }
                ui.label(egui::RichText::new("›").weak().size(10.0));
            }

            // Segmen folder + nama file
            let n = rel_parts.len();
            for (i, part) in rel_parts.iter().enumerate() {
                let is_last = i + 1 == n;
                let text = egui::RichText::new(part).monospace().size(11.0);
                let resp = if is_last {
                    ui.selectable_label(false, text.strong())
                        .on_hover_text("Klik: salin path lengkap")
                } else {
                    ui.selectable_label(false, text)
                        .on_hover_text("Buka tab Project")
                };
                if resp.clicked() {
                    if is_last {
                        want_copy = true;
                    } else {
                        want_project_tab = true;
                    }
                }
                if !is_last {
                    ui.label(egui::RichText::new("›").weak().size(10.0));
                }
            }

            // Fallback: file di luar project root → tampilkan path mentah
            if rel_parts.is_empty() {
                ui.label(egui::RichText::new(&path_str).weak().monospace().size(10.0));
            }
        });

        if want_copy {
            ui.ctx().copy_text(path_str.clone());
        }
        if want_project_tab {
            state.sidebar_tab = crate::state::SidebarTab::Project;
        }
    }
    ui.add_space(2.0);

    // ── Code Lens strip: deklarasi module/interface/package di file ini ──
    // dengan jumlah referensi (berapa kali module di-instansiasi di seluruh
    // design). Klik segmen → salin nama. Hanya file yang sudah di-compile.
    // `ref_counts` di-precompute saat compile — tidak iterasi design per frame.
    // Statistik global (compile time + coverage) ditampilkan di ujung strip —
    // sesuai desain Code Lens: "Compile Time 0.31 ms, Coverage 98% langsung di
    // atas module". Coverage disalin dulu (nilai Copy) agar borrow field
    // `state.coverage` tidak konflik dengan `f` (yang meminjam open_files).
    let cov_pct: Option<f64> = if state.coverage.branch_total > 0 {
        Some(state.coverage.branch_percent)
    } else {
        None
    };
    if let Some(info) = state.compile_info.as_ref() {
        let lens = build_code_lens(&f.content, &info.ref_counts);
        if !lens.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (kind, name, refs) in &lens {
                    let icon = match kind.as_str() {
                        "interface" => "◇",
                        "package" => "◈",
                        _ => "▣",
                    };
                    let label = format!("{} {} · {}×", icon, name, refs);
                    let resp = ui
                        .selectable_label(false, egui::RichText::new(label).monospace().size(10.0))
                        .on_hover_text(format!(
                            "{} · direferensikan {} kali\nKlik: salin nama",
                            kind, refs
                        ));
                    if resp.clicked() {
                        ui.ctx().copy_text(name.clone());
                    }
                }

                // Statistik global (compile time + coverage) di ujung strip.
                ui.separator();
                let ct = format!("⏱ {:.2} ms", info.total_time_ms);
                ui.label(egui::RichText::new(&ct).weak().monospace().size(10.0))
                    .on_hover_text("Total compile + elaborate");
                if let Some(pct) = cov_pct {
                    let color = if pct >= 90.0 {
                        egui::Color32::from_rgb(34, 197, 94) // hijau
                    } else if pct >= 60.0 {
                        egui::Color32::from_rgb(234, 179, 8) // kuning
                    } else {
                        egui::Color32::from_rgb(239, 68, 68) // merah
                    };
                    ui.label(
                        egui::RichText::new(format!("📊 {:.1}%", pct))
                            .weak()
                            .monospace()
                            .size(10.0)
                            .color(color),
                    )
                    .on_hover_text("Branch coverage (hasil simulasi terakhir)");
                }
            });
            ui.separator();
        }
    }

    // ── Editor kustom (semantic highlight) + Mini Map ──
    // egui_code_editor memakai layouter internal yang terkunci (tidak bisa
    // disuntik), jadi editor dibangun dari `TextEdit::multiline` + layouter
    // `semantic::highlight` yang mengklasifikasi identifier secara semantik:
    // module biru, interface ungu, package cyan, parameter oranye, signal
    // putih, clock kuning, reset merah, macro abu, typedef hijau, enum teal.
    // Gutter nomor baris berada dalam ScrollArea vertikal yang sama dengan
    // editor sehingga scroll-nya sinkron.
    let id = format!("sv_editor:{}", f.path.display());

    // Kumpulkan diagnostic untuk file ini (cocokkan nama file) — dibaca dari
    // field `state.diagnostics` dan `state.lsp_diags` (LSP server `mivon
    // --lsp`), keduanya disjoint dari `state.open_files` (borrow field-split
    // valid, sama seperti Code Lens di atas).
    let path_str = f.path.display().to_string();
    let file_diags: Vec<&DiagEntry> = state
        .diagnostics
        .iter()
        .chain(state.lsp_diags.iter())
        .filter(|d| diag_matches_file(&d.file, &path_str))
        .collect();

    // Data compile untuk Hover tooltip — field-split valid (disjoint dari
    // `state.open_files` yang dipinjam mut oleh `f`), sama seperti Code Lens.
    let sig_info: Option<&HashMap<String, (String, usize)>> =
        state.compile_info.as_ref().map(|ci| &ci.signal_info);
    let ref_counts: Option<&HashMap<String, usize>> =
        state.compile_info.as_ref().map(|ci| &ci.ref_counts);
    let symbol_files: Option<&HashMap<String, PathBuf>> =
        state.compile_info.as_ref().map(|ci| &ci.symbol_files);
    // Data AST untuk Autocomplete — module/interface/package dari compile
    // (field-split valid: disjoint dari `state.open_files` yang dipinjam `f`).
    let modules: Option<&Vec<String>> = state.compile_info.as_ref().map(|ci| &ci.modules);
    let packages: Option<&Vec<String>> = state.compile_info.as_ref().map(|ci| &ci.packages);
    let interfaces: Option<&Vec<String>> = state.compile_info.as_ref().map(|ci| &ci.interfaces);

    let mm_w = 14.0;
    let avail = (ui.available_width() - mm_w - 8.0).max(200.0);
    let mut want_problems = false;
    // Go To Definition (Ctrl+Click): (file target, baris) — diterapkan setelah
    // closure (perlu `state` untuk membuka file; `f` masih dipinjam di dalam).
    let mut want_goto: Option<(PathBuf, Option<usize>)> = None;
    // Rename Symbol: flag hasil popup rename — diterapkan setelah closure
    // (perlu iterasi `state.open_files`; `f` masih dipinjam di dalam).
    let mut rename_done = false;
    let mut rename_cancel = false;
    // Fokus input rename diminta di frame pertama sesi (di-reset saat F2).
    let mut rename_focus_pending = false;
    // Peek Definition (Alt+Click): (nama, file target, baris, posisi klik).
    let mut want_peek: Option<(String, PathBuf, usize, egui::Pos2)> = None;
    // Popup peek baru dibuka frame ini — jangan langsung ditutup oleh klik yang
    // sama (klik pembuka juga terdeteksi sebagai `any_click`).
    let mut peek_opened_this_frame = false;
    // Autocomplete: accept terdeteksi di dalam popup, diterapkan SETELAH
    // closure (perlu `state.open_files`; `f` masih dipinjam di dalam).
    let mut completion_accept = false;
    // Konten awal frame — basis rollback saat accept via Enter (TextEdit
    // multiline menyisipkan '\n' sebelum kita sempat memproses kandidat).
    let mut frame_before = String::new();
    // Teks berubah frame ini (dipakai trigger popup saat mengetik).
    let mut text_changed = false;
    // Popup baru dibuka frame ini — rebuild kandidat pertama wajib jalan
    // (Ctrl+Space dengan prefix kosong & items kosong harus tetap tampil),
    // meski teks tidak berubah.
    let mut completion_just_opened = false;

    // ── LSP (server `mivon --lsp`) ──
    // Di dalam closure editor `state` dipinjam mut via `f` (open_files) —
    // method `lsp_send`/`lsp_alloc_id` (borrow seluruh state) tidak bisa
    // dipanggil di sana. Perintah diakumulasi di lokal, di-flush SETELAH
    // closure (borrow `state` penuh tersedia).
    let lsp_ready = state.lsp_ready;
    let mut lsp_cmds: Vec<crate::lsp_client::LspCmd> = Vec::new();
    let mut lsp_pending: Vec<(u64, LspPendingKind)> = Vec::new();
    let mut lsp_next = state.lsp_next_id;
    // Snapshot hover sebelumnya — diperbarui di dalam closure (bukan
    // `state.lsp_last_hover` langsung, hindari borrow).
    let mut lsp_last_hover0 = state.lsp_last_hover.clone();

    // ── Sticky Header ──
    // Deklarasi scope enclosing (module/interface/package/function/task/
    // always/initial/begin) ditempel di atas editor saat scroll — sesuai
    // desain: "saat scroll, `module cache_controller` tetap terlihat".
    // Tinggi baris monospace memetakan offset scroll (pixel) → nomor baris.
    let line_h = ui
        .ctx()
        .fonts_mut(|fonts| fonts.row_height(&egui::FontId::monospace(semantic::FONT_SIZE)))
        .max(1.0);
    // Lebar satu glyph monospace — memetakan hover → kolom karakter.
    let char_w = ui
        .ctx()
        .fonts_mut(|fonts| fonts.glyph_width(&egui::FontId::monospace(semantic::FONT_SIZE), 'm'))
        .max(1.0);
    rebuild_sticky(f);
    let sticky_jump = draw_sticky_header(ui, f, line_h);
    // Lompat yang belum sempat dieksekusi (Go To Definition dari frame lalu).
    let jump_line = sticky_jump.or_else(|| f.pending_goto.take());

    // Gutter nomor baris — dibangun sekali per frame dari konten aktif.
    let line_count = f.content.lines().count().max(1);
    let rows = line_count;
    let gutter_text: String = (1..=line_count)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut gutter_buf = gutter_text;
    let gutter_w = (line_count.to_string().len() as f32) * 8.0 + 10.0;

    ui.horizontal(|ui| {
        // Layouter gutter: angka baris abu-abu, non-interaktif.
        let mut gutter_layouter = |ui: &egui::Ui, buf: &dyn TextBuffer, _: f32| {
            let job = egui::text::LayoutJob::single_section(
                buf.as_str().to_string(),
                egui::TextFormat::simple(
                    egui::FontId::monospace(semantic::FONT_SIZE),
                    egui::Color32::from_rgb(110, 118, 129),
                ),
            );
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };

        // Layouter editor: semantic highlight SystemVerilog.
        let mut layouter = |ui: &egui::Ui, buf: &dyn TextBuffer, _wrap_width: f32| {
            let job = semantic::highlight(buf.as_str());
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };

        // Gutter + editor dalam satu ScrollArea vertikal (scroll sinkron),
        // editor dalam ScrollArea horizontal untuk baris panjang.
        // ScrollArea vertikal — offset bisa di-set programatik (lompat ke
        // deklarasi scope dari Sticky Header). Offset hasil dibaca kembali
        // untuk menyinkronkan `f.scroll_top` (dipakai Sticky Header).
        let mut vscroll = egui::ScrollArea::vertical()
            .id_salt(format!("{}_vscroll", id))
            .auto_shrink([false, false]);
        if let Some(line) = jump_line {
            vscroll = vscroll.vertical_scroll_offset((line.saturating_sub(1)) as f32 * line_h);
        }
        // Response editor + offset scroll horizontal di-hoist agar bisa dipakai
        // setelah ScrollArea selesai (untuk Hover tooltip).
        let mut editor_resp: Option<egui::Response> = None;
        let mut scroll_left: f32 = 0.0;
        let vout = vscroll.show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut gutter_buf)
                        .id_source(format!("{}_gutter", id))
                        .font(egui::FontSelection::FontId(egui::FontId::monospace(
                            semantic::FONT_SIZE,
                        )))
                        .interactive(false)
                        .frame(egui::Frame::NONE)
                        .desired_rows(rows)
                        .desired_width(gutter_w)
                        .layouter(&mut gutter_layouter),
                );

                let hout = egui::ScrollArea::horizontal()
                    .id_salt(format!("{}_hscroll", id))
                    .show(ui, |ui| {
                        let before = f.content.clone();
                        let r = ui.add(
                            egui::TextEdit::multiline(&mut f.content)
                                .id_source(id.as_str())
                                // Font widget HARUS sama dengan FontId layouter
                                // (monospace FONT_SIZE) agar kursor/selection
                                // sejajar dengan teks yang dirender layouter.
                                .font(egui::FontSelection::FontId(egui::FontId::monospace(
                                    semantic::FONT_SIZE,
                                )))
                                // Tanpa lock_focus: fokus lock editor akan
                                // memblokir transfer fokus ke popup Rename
                                // Symbol (request_focus widget lain dibuang).
                                .desired_rows(rows)
                                .desired_width(avail)
                                .layouter(&mut layouter),
                        );
                        editor_resp = Some(r.clone());
                        if f.content != before {
                            f.dirty = true;
                            text_changed = true;
                        }
                        // Basis rollback accept (konten sebelum TextEdit
                        // memproses Enter/karakter frame ini).
                        frame_before = before;
                    });
                scroll_left = hout.state.offset.x;
            });
        });
        let scroll_top = vout.state.offset.y.max(0.0);
        f.scroll_top = scroll_top;

        // ── Hover Tooltip ──
        // Identifier di bawah kursor → info (desain: "saat mouse di `cache_valid`
        // langsung muncul logic, width, declared, used, last assignment").
        // Posisi mouse dipetakan ke baris/kolom global via metrik monospace +
        // offset scroll vertikal & horizontal, lalu `semantic::identifier_at`.
        if let Some(resp) = editor_resp {
            // Identifier di bawah kursor — dipakai kursor link, tooltip, & goto
            // (dihitung SEKALI per frame, bukan per fitur).
            let hover_id = if resp.hovered() {
                hovered_identifier(&resp, &f.content, scroll_top, scroll_left, char_w, line_h)
            } else {
                None
            };
            // Go To Definition: kursor tangan hanya saat Ctrl dipegang di atas
            // identifier yang benar-benar bisa di-goto (bukan whitespace).
            if hover_id.is_some() && ui.input(|i| i.modifiers.command) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            // Rename Symbol: F2 saat hover di atas identifier → buka input.
            if !f.renaming && hover_id.is_some() && ui.input(|i| i.key_pressed(egui::Key::F2)) {
                if let Some((name, _)) = &hover_id {
                    f.renaming = true;
                    f.rename_old = name.clone();
                    f.rename_new = name.clone();
                    // Fokus diminta saat popup pertama dirender (id field harus
                    // dihitung dari ui Area, bukan outer ui).
                    rename_focus_pending = true;
                }
            }
            // LSP hover: kirim request saat identifier hover BERUBAH (bukan
            // tiap frame). Konten datang async → popup di app.rs.
            if let Some((name, _)) = &hover_id {
                if lsp_ready && lsp_last_hover0.as_deref() != Some(name.as_str()) {
                    lsp_last_hover0 = Some(name.clone());
                    if let Some(pos) = resp.hover_pos() {
                        if let Some((l0, c0)) = hover_line_col(
                            &resp,
                            &f.content,
                            scroll_top,
                            scroll_left,
                            char_w,
                            line_h,
                            pos,
                        ) {
                            let id = lsp_next;
                            lsp_next = lsp_next.wrapping_add(1).max(1000);
                            lsp_pending.push((id, LspPendingKind::HoverRequest));
                            lsp_cmds.push(crate::lsp_client::LspCmd::Hover {
                                id,
                                path: f.path.clone(),
                                line: l0,
                                character: c0,
                            });
                        }
                    }
                }
            } else {
                lsp_last_hover0 = None;
            }
            // Hover tooltip (identifier di bawah kursor).
            if let Some((name, kind)) = hover_id {
                egui::Tooltip::for_widget(&resp)
                    .at_pointer()
                    .gap(12.0)
                    .show(|ui| {
                        hover_tooltip_ui(ui, f, &name, kind, sig_info, ref_counts);
                    });
            }
            // Go To Definition: Ctrl+Click → buka file deklarasi / lompat baris.
            // Bila LSP siap, pakai hasil server (lebih akurat); kalau tidak,
            // fallback heuristik lokal.
            if resp.clicked() && ui.input(|i| i.modifiers.command) {
                if let Some(pos) = resp.interact_pointer_pos() {
                    if let Some((name, kind)) = identifier_at_pos(
                        &resp,
                        &f.content,
                        scroll_top,
                        scroll_left,
                        char_w,
                        line_h,
                        pos,
                    ) {
                        if lsp_ready {
                            if let Some((l0, c0)) = hover_line_col(
                                &resp,
                                &f.content,
                                scroll_top,
                                scroll_left,
                                char_w,
                                line_h,
                                pos,
                            ) {
                                let id = lsp_next;
                                lsp_next = lsp_next.wrapping_add(1).max(1000);
                                lsp_pending.push((id, LspPendingKind::GotoRequest));
                                lsp_cmds.push(crate::lsp_client::LspCmd::Goto {
                                    id,
                                    path: f.path.clone(),
                                    line: l0,
                                    character: c0,
                                });
                            }
                        } else {
                            want_goto = resolve_goto(f, &name, kind, symbol_files);
                        }
                    }
                }
            }
            // Peek Definition: Alt+Click → pratinjau deklarasi (tanpa pindah
            // tab — sesuai desain "hanya popup"). Gate `!command`: satu gesture
            // = satu aksi (Ctrl+Alt+Click cukup memicu goto saja).
            if resp.clicked() && ui.input(|i| i.modifiers.alt && !i.modifiers.command) {
                if let Some(pos) = resp.interact_pointer_pos() {
                    if let Some((name, kind)) = identifier_at_pos(
                        &resp,
                        &f.content,
                        scroll_top,
                        scroll_left,
                        char_w,
                        line_h,
                        pos,
                    ) {
                        if let Some((path, line)) = resolve_goto(f, &name, kind, symbol_files) {
                            want_peek = Some((name, path, line.unwrap_or(1), pos));
                        }
                    }
                }
            }
            // Rename Symbol: popup input nama baru di dekat kursor.
            if f.renaming {
                let anchor = resp.hover_pos().unwrap_or_else(|| resp.rect.center())
                    + egui::vec2(12.0, -36.0);
                egui::Area::new(ui.id().with("rename_input"))
                    .fixed_pos(anchor)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        // Id field dihitung dari ui AREA (bukan outer ui) —
                        // TextEdit memakai `ui.make_persistent_id(salt)` =
                        // `ui.id().with(salt)`, jadi salt harus dari ui yang
                        // sama agar `request_focus` tepat sasaran.
                        let field_id = ui.id().with("rename_field");
                        ui.label(
                            egui::RichText::new(format!("✏ Rename '{}'", f.rename_old)).strong(),
                        );
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut f.rename_new)
                                .id_source(field_id)
                                .font(egui::FontSelection::FontId(egui::FontId::monospace(
                                    semantic::FONT_SIZE,
                                )))
                                .desired_width(240.0)
                                .hint_text("Nama baru"),
                        );
                        if rename_focus_pending {
                            r.request_focus();
                            rename_focus_pending = false;
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            rename_cancel = true;
                        } else if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            rename_done = true;
                        } else if r.lost_focus() {
                            rename_cancel = true;
                        }
                    });
            }

            // ── Autocomplete ──
            // Kandidat dari AST (module/interface/package/signal dari design)
            // + keyword SV. Popup otomatis saat mengetik identifier (prefix
            // non-kosong & teks berubah) atau Ctrl+Space eksplisit. Navigasi
            // ↑/↓ pilih, Enter/klik sisip, Esc/klik luar batal. Posisi caret
            // dibaca dari state TextEdit (id = response editor).
            {
                let caret_char = egui::text_edit::TextEditState::load(ui.ctx(), resp.id)
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| r.primary.index.0);
                let caret_byte = caret_char.map(|c| char_to_byte(&f.content, c));
                let ctrl_space =
                    ui.input(|i| i.key_pressed(egui::Key::Space) && i.modifiers.command);

                // Buka popup: ketik karakter baru di dalam kata, atau Ctrl+Space.
                if !f.completing {
                    if let Some(cb) = caret_byte {
                        let (ws, we) = word_region(&f.content, cb);
                        let prefix = &f.content[ws..we.min(f.content.len())];
                        let prefix_valid = !prefix.is_empty() && prefix.bytes().all(is_word_char);
                        if (text_changed && prefix_valid) || ctrl_space {
                            f.completing = true;
                            f.completion_insert = ws;
                            f.completion_end = we.min(f.content.len());
                            f.completion_prefix = prefix.to_string();
                            f.completion_selected = 0;
                            f.completion_items.clear(); // rebuild di bawah
                            completion_just_opened = true;
                        }
                    }
                }

                if f.completing {
                    // Accept: Enter — TextEdit sudah menyisipkan '\n' di caret;
                    // region [insert, end) dari frame sebelumnya tetap akurat
                    // (rollback ke frame_before saat apply).
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        completion_accept = true;
                    } else if let Some(cb) = caret_byte {
                        // Update akhir kata bila caret masih di dalam kata
                        // (mengetik memperpanjang). Bila caret KELUAR kata,
                        // tutup popup HANYA jika teks berubah frame ini —
                        // ArrowUp/Down menggerakkan caret teks (TextEdit
                        // memegang fokus) TANPA mengubah teks; tanpa gate
                        // ini, panah pertama langsung menutup popup dan
                        // navigasi daftar tidak pernah berfungsi.
                        let in_word = cb >= f.completion_insert
                            && cb <= f.content.len()
                            && f.content.as_bytes()[f.completion_insert..cb]
                                .iter()
                                .all(|b| is_word_char(*b));
                        if in_word {
                            f.completion_end = cb;
                        } else if !f.completion_prefix.is_empty() && text_changed {
                            f.completing = false;
                        }
                    }
                    // Rebuild kandidat hanya saat prefix berubah atau popup baru
                    // dibuka (Ctrl+Space dengan prefix kosong harus tetap tampil).
                    // Tanpa flag `just_opened`, rebuild hanya pada perubahan
                    // prefix — panah/klik tidak memicu rebuild yang sia-sia.
                    if f.completing {
                        let prefix = if f.completion_end >= f.completion_insert {
                            &f.content[f.completion_insert..f.completion_end]
                        } else {
                            ""
                        };
                        if prefix != f.completion_prefix || completion_just_opened {
                            completion_just_opened = false;
                            f.completion_prefix = prefix.to_string();
                            let all = completion_candidates(
                                &f.content, modules, packages, interfaces, sig_info,
                            );
                            let filtered: Vec<String> = all
                                .into_iter()
                                .filter(|s| {
                                    s.get(..prefix.len())
                                        .map(|h| h.eq_ignore_ascii_case(prefix))
                                        .unwrap_or(false)
                                })
                                .take(50)
                                .collect();
                            f.completion_items = filtered;
                            f.completion_selected = 0;
                        }
                    }
                }

                if !completion_accept && f.completing && !f.completion_items.is_empty() {
                    // Navigasi ↑/↓ (item pilihan bergerak; caret teks ikut
                    // bergerak — tradeoff TextEdit yang memegang fokus).
                    let n = f.completion_items.len();
                    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                        f.completion_selected = (f.completion_selected + 1).min(n - 1);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        f.completion_selected = f.completion_selected.saturating_sub(1);
                    }
                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));

                    // Posisi popup: dekat caret (perkiraan baris/kolom dari
                    // indeks karakter via metrik monospace + offset scroll).
                    let (row, col) = caret_char
                        .map(|c| line_col_at_char(&f.content, c))
                        .unwrap_or((0, 0));
                    let margin = egui::Margin::symmetric(4, 2);
                    let origin = resp.rect.min + egui::vec2(margin.left as f32, margin.top as f32);
                    let anchor = origin
                        + egui::vec2(
                            col as f32 * char_w - scroll_left,
                            (row as f32 + 1.0) * line_h - scroll_top + 2.0,
                        );

                    let inner = egui::Area::new(ui.id().with("autocomplete"))
                        .fixed_pos(anchor)
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.set_min_width(260.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("✦")
                                            .color(egui::Color32::from_rgb(79, 193, 255)),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} — {} kandidat",
                                            f.completion_prefix,
                                            f.completion_items.len()
                                        ))
                                        .weak()
                                        .size(10.0),
                                    );
                                });
                                ui.separator();
                                egui::ScrollArea::vertical()
                                    .id_salt("autocomplete_list")
                                    .max_height(180.0)
                                    .show(ui, |ui| {
                                        let items = f.completion_items.clone();
                                        for (i, item) in items.iter().enumerate() {
                                            let sel = i == f.completion_selected;
                                            let text = egui::RichText::new(item)
                                                .monospace()
                                                .size(11.0)
                                                .color(if sel {
                                                    egui::Color32::from_rgb(59, 130, 246)
                                                } else {
                                                    ui.visuals().text_color()
                                                });
                                            let r = ui.selectable_label(sel, text);
                                            if sel {
                                                r.scroll_to_me(Some(egui::Align::Center));
                                            }
                                            if r.clicked() {
                                                f.completion_selected = i;
                                                completion_accept = true;
                                            }
                                        }
                                    });
                                ui.separator();
                                ui.label(
                                    egui::RichText::new("↑↓ pilih · Enter sisip · Esc batal")
                                        .weak()
                                        .size(10.0),
                                );
                            });
                        });

                    // Klik di luar popup → tutup (Esc juga).
                    let click_outside = !completion_accept
                        && ui.input(|i| i.pointer.any_click())
                        && ui
                            .input(|i| i.pointer.interact_pos())
                            .map(|p| !inner.response.rect.contains(p))
                            .unwrap_or(false);
                    if esc || click_outside {
                        f.completing = false;
                    }
                }
            }
        }

        ui.add_space(4.0);
        if draw_minimap(ui, &f.content, &file_diags) {
            want_problems = true;
        }
    });

    // ── Flush perintah LSP dari dalam closure (borrow `state` penuh OK di
    // sini) + sinkron status hover. ──
    state.lsp_last_hover = lsp_last_hover0;
    if !lsp_pending.is_empty() || !lsp_cmds.is_empty() {
        state.lsp_next_id = lsp_next.max(1000);
        for (id, kind) in lsp_pending {
            state.lsp_pending.insert(id, kind);
        }
        for cmd in lsp_cmds {
            state.lsp_send(cmd);
        }
    }
    if want_problems {
        state.bottom_tab = BottomTab::Problems;
    }

    // ── Autocomplete: sisipkan kandidat terpilih (Enter / klik item) ──
    if completion_accept {
        let mut applied = false;
        if let Some(of) = state.open_files.get_mut(idx) {
            if of.completing {
                let item = of.completion_items.get(of.completion_selected).cloned();
                if let Some(item) = item {
                    let insert = of.completion_insert;
                    let end = of.completion_end;
                    // Basis = konten awal frame: TextEdit multiline menyisipkan
                    // '\n' pada Enter sebelum accept diproses — rollback supaya
                    // region [insert, end) akurat (tanpa '\n').
                    let mut c = frame_before;
                    if insert <= end && end <= c.len() {
                        c.replace_range(insert..end, &item);
                        of.content = c;
                        of.dirty = true;
                        applied = true;
                    }
                }
                of.completing = false;
            }
        }
        if applied {
            state.log("✦ Autocomplete: kandidat disisipkan");
        }
    }

    // ── Go To Definition: buka file target (bila beda) + lompat ke baris ──
    if let Some((path, line)) = want_goto {
        if state.open_files.iter().all(|of| of.path != path) {
            state.open_file(path.clone());
        }
        if let Some(idx) = state.open_files.iter().position(|of| of.path == path) {
            state.active_file = Some(idx);
            match line {
                Some(l) => {
                    state.open_files[idx].pending_goto = Some(l);
                    state.log(format!("→ Go to definition: {}:{}", path.display(), l));
                }
                None => state.log(format!("→ Go to definition: {}", path.display())),
            }
        }
    }

    // ── Rename Symbol: ganti semua referensi kata-utuh di SEMUA file terbuka
    // + simpan ke disk (refactor intent). Enter = terapkan, Esc/klik luar = batal.
    if rename_cancel {
        state.open_files[idx].renaming = false;
    }
    if rename_done {
        let old = state.open_files[idx].rename_old.clone();
        let new = state.open_files[idx].rename_new.clone();
        state.open_files[idx].renaming = false;
        if !old.is_empty() && old != new {
            let mut renamed = 0usize;
            for of in state.open_files.iter_mut() {
                let n = replace_word(&mut of.content, &old, &new);
                if n > 0 {
                    renamed += n;
                    of.dirty = !std::fs::write(&of.path, &of.content).is_ok();
                }
            }
            state.log(format!(
                "✏ Rename '{}' → '{}' · {} referensi",
                old, new, renamed
            ));
        }
    }

    // ── Peek Definition (Alt+Click): siapkan data pratinjau ──
    if let Some((name, path, line, pos)) = want_peek {
        if let Some(info) = build_peek_info(state, &name, &path, line) {
            state.peek = Some(info);
            // Anchor sedikit offset dari titik klik agar klik pembuka tidak
            // jatuh di dalam popup (menghindari goto langsung pada frame buka).
            state.peek_anchor = Some((pos.x + 12.0, pos.y + 12.0));
            peek_opened_this_frame = true;
        }
    }

    // ── Render popup Peek Definition (jika aktif) ──
    if let Some(peek) = state.peek.clone() {
        if let Some((ax, ay)) = state.peek_anchor {
            let mut goto = false;
            let mut clicked_inside = false;
            egui::Area::new(ui.id().with("peek_def"))
                .fixed_pos(egui::pos2(ax, ay))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    let fname = std::path::Path::new(&peek.file)
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    ui.label(
                        egui::RichText::new(format!("◉ {} — {}:{}", peek.name, fname, peek.line))
                            .strong()
                            .size(12.0),
                    );
                    ui.separator();
                    for (ln, text) in &peek.lines {
                        let is_decl = *ln == peek.line;
                        let color = if is_decl {
                            egui::Color32::from_rgb(59, 130, 246)
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.label(
                            egui::RichText::new(format!("{:>4} │ {}", ln, text))
                                .monospace()
                                .size(11.0)
                                .color(color),
                        );
                    }
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Klik: buka definisi · Esc: tutup")
                            .weak()
                            .size(10.0),
                    );
                    let resp2 = ui.interact(
                        ui.max_rect(),
                        ui.id().with("peek_click"),
                        egui::Sense::click(),
                    );
                    if resp2.clicked() {
                        goto = true;
                        clicked_inside = true;
                    }
                });
            if goto {
                // Buka file target + lompat ke baris deklarasi.
                let path = peek.file.clone();
                let line = peek.line;
                if !state.open_files.iter().any(|of| of.path == path) {
                    state.open_file(path.clone());
                }
                if let Some(gidx) = state.open_files.iter().position(|of| of.path == path) {
                    state.active_file = Some(gidx);
                    state.open_files[gidx].pending_goto = Some(line);
                }
                state.peek = None;
                state.peek_anchor = None;
                state.log(format!("→ Buka definisi: {}:{}", path.display(), line));
            } else {
                let any_click = ui.ctx().input(|i| i.pointer.any_click());
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if esc || (any_click && !clicked_inside && !peek_opened_this_frame) {
                    state.peek = None;
                    state.peek_anchor = None;
                }
            }
        }
    }
}

/// Bangun `PeekInfo` (pratinjau deklarasi) untuk popup Peek Definition.
/// Konten diambil dari file yang sudah terbuka (bila ada) atau dibaca disk;
/// konteks = 2 baris sebelum + 1 baris sesudah baris deklarasi.
fn build_peek_info(state: &GuiState, name: &str, path: &PathBuf, line: usize) -> Option<PeekInfo> {
    let content = state
        .open_files
        .iter()
        .find(|of| of.path == *path)
        .map(|of| of.content.clone())
        .or_else(|| std::fs::read_to_string(path).ok())?;
    let all: Vec<&str> = content.lines().collect();
    let start = line.saturating_sub(2).min(all.len());
    let end = (line + 1).min(all.len()).max(start + 1);
    let mut lines = Vec::new();
    for i in start..end {
        if i < all.len() {
            lines.push((i + 1, all[i].to_string()));
        }
    }
    Some(PeekInfo {
        file: path.clone(),
        name: name.to_string(),
        line,
        lines,
    })
}

/// Scan konten file untuk deklarasi `module`/`interface`/`package`, lalu ambil
/// jumlah referensi dari peta precomputed (`ref_counts`, dibangun sekali saat
/// compile di backend). Mengembalikan (kind, name, reference_count), terurut
/// sesuai kemunculan di file. Tidak iterasi design per frame — performa aman
/// untuk desain sebesar OpenTitan.
fn build_code_lens(
    content: &str,
    ref_counts: &std::collections::HashMap<String, usize>,
) -> Vec<(String, String, usize)> {
    // Scan baris file untuk deklarasi (baris tanpa komentar `//`).
    let mut out: Vec<(String, String, usize)> = Vec::new();
    for raw in content.lines() {
        let line = raw.split("//").next().unwrap_or(raw).trim();
        let mut it = line.split_whitespace();
        let kw = match it.next() {
            Some("module") => Some("module"),
            Some("interface") => Some("interface"),
            Some("package") => Some("package"),
            _ => None,
        };
        let Some(kind) = kw else { continue };
        // Nama module: token pertama setelah keyword, tanpa karakter `#(`/`(`.
        let name: String = it
            .next()
            .map(|tok| {
                tok.trim_start_matches('#')
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect()
            })
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let refs = ref_counts.get(&name).copied().unwrap_or(0);
        out.push((kind.to_string(), name, refs));
    }
    out
}

/// Mini Map editor: strip sempit di kanan editor, setiap baris file diwarnai
/// sesuai status — hijau (stable), kuning (warning), merah (error) — persis
/// desain Mivon. Hover pada baris bermasalah → tooltip pesan diagnostic.
/// Klik strip → buka tab Problems. Mengembalikan true jika diklik.
fn draw_minimap(ui: &mut egui::Ui, content: &str, file_diags: &[&DiagEntry]) -> bool {
    let mm_w = 14.0;
    let height = ui.available_height().max(40.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(mm_w, height), egui::Sense::click());
    let painter = ui.painter_at(rect);

    // Latar belakang strip
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);

    let total = content.lines().count().max(1);
    let line_h = rect.height() / total as f32;

    // Severity terparah per baris — peringkat dari tipe pusat (mivon-diag):
    // Fatal/Bug/Error > Warning > sisanya (Note/Info/Help/Trace/Debug).
    let mut sev: std::collections::HashMap<usize, DiagLevel> = std::collections::HashMap::new();
    for d in file_diags {
        if d.line == 0 {
            continue;
        }
        let rank = |l: DiagLevel| match l {
            DiagLevel::Fatal | DiagLevel::Bug | DiagLevel::Error => 2,
            DiagLevel::Warning => 1,
            _ => 0,
        };
        let cur = sev.get(&d.line).copied();
        let new = match cur {
            Some(prev) if rank(prev) >= rank(d.level) => prev,
            _ => d.level,
        };
        sev.insert(d.line, new);
    }

    let green = egui::Color32::from_rgb(46, 110, 60);
    let yellow = egui::Color32::from_rgb(234, 179, 8);
    let red = egui::Color32::from_rgb(239, 68, 68);

    for (i, _) in content.lines().enumerate() {
        let line_no = i + 1;
        let y0 = rect.top() + i as f32 * line_h;
        let y1 = rect.top() + (i as f32 + 1.0) * line_h;
        let color = match sev.get(&line_no) {
            Some(DiagLevel::Error) => red,
            Some(DiagLevel::Warning) => yellow,
            _ => green,
        };
        let r = egui::Rect::from_min_max(
            egui::pos2(rect.left(), y0),
            egui::pos2(rect.right(), y1.max(y0 + 1.0)),
        );
        painter.rect_filled(r, 0.0, color);
    }

    // Border tipis
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        egui::StrokeKind::Inside,
    );

    // Hover pada baris bermasalah → tooltip pesan
    if resp.hovered() {
        if let Some(pos) = resp.hover_pos() {
            let line_no = ((pos.y - rect.top()) / line_h) as usize + 1;
            let msgs: Vec<(DiagLevel, String)> = file_diags
                .iter()
                .filter(|d| d.line == line_no)
                .map(|d| {
                    // Icon per level tipe pusat: Fatal/Bug/Error → ✖.
                    let icon = match d.level {
                        DiagLevel::Error | DiagLevel::Bug | DiagLevel::Fatal => "✖",
                        DiagLevel::Warning => "⚠",
                        _ => "ℹ",
                    };
                    (d.level, format!("{} L{}: {}", icon, d.line, d.message))
                })
                .collect();
            if !msgs.is_empty() {
                resp.clone().on_hover_ui(|ui| {
                    for (level, m) in msgs {
                        let color = match level {
                            DiagLevel::Error | DiagLevel::Bug | DiagLevel::Fatal => red,
                            DiagLevel::Warning => yellow,
                            _ => egui::Color32::from_rgb(59, 130, 246),
                        };
                        ui.label(egui::RichText::new(m).monospace().size(11.0).color(color));
                    }
                });
            }
        }
    }

    resp.clicked()
}

// ───────────────────────────── Sticky Header ─────────────────────────────

/// Fingerprint FNV-1a sederhana (cepat) — mendeteksi perubahan konten file
/// untuk tahu kapan cache scope (Sticky Header) perlu di-rebuild.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Rebuild cache scope hanya jika konten berubah (bandingkan fingerprint) —
/// bukan setiap frame. Dipanggil sekali per frame, tapi build_sticky hanya
/// jalan saat file benar-benar diedit.
fn rebuild_sticky(f: &mut OpenFile) {
    let fp = fnv1a(&f.content);
    if fp != f.sticky_fp {
        f.sticky = build_sticky(&f.content);
        f.sticky_fp = fp;
    }
}

/// Potong teks hingga ~64 karakter (jangan pecah di tengah char UTF-8).
fn truncate(t: &str) -> String {
    let t = t.trim();
    let mut chars = t.chars();
    let mut s: String = chars.by_ref().take(64).collect();
    if chars.next().is_some() {
        s.push('…');
    }
    s
}

/// Nama function/task: token setelah keyword, lewati return type & modifier
/// (`function automatic int foo(...)` → `foo`).
fn fn_task_name(words: &[&str]) -> String {
    const SKIP: &[&str] = &[
        "automatic",
        "static",
        "pure",
        "extern",
        "void",
        "int",
        "integer",
        "logic",
        "bit",
        "byte",
        "shortint",
        "longint",
        "real",
        "shortreal",
        "reg",
        "wire",
        "signed",
        "unsigned",
    ];
    for w in words.iter().skip(1) {
        let clean: String = w
            .trim_start_matches('#')
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !clean.is_empty() && !SKIP.contains(&clean.as_str()) {
            return clean;
        }
    }
    words
        .get(1)
        .map(|w| {
            w.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect()
        })
        .unwrap_or_default()
}

/// Scan konten untuk deklarasi scope (Sticky Header). Menghasilkan daftar
/// scope urut kemunculan di file. `depth` = kedalaman blok begin/end saat
/// deklarasi (untuk indentasi tampilan). Heuristik per-baris:
/// `module/interface/package` → nama setelah keyword; `function/task` → nama
/// setelah return type; `always_*`/`always` → sampai `begin` (sensitivity
/// list ikut); `initial/final` → gabung `begin`; `begin [ : label ]` → blok
/// bernama. `end`/`endmodule` dll hanya menurunkan/mereset kedalaman.
fn build_sticky(content: &str) -> Vec<StickyScope> {
    let mut out: Vec<StickyScope> = Vec::new();
    let mut depth: usize = 0;
    for (i, raw) in content.lines().enumerate() {
        let line_no = i + 1;
        let code = raw.split("//").next().unwrap_or(raw);
        let trimmed = code.trim();
        if trimmed.is_empty() {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let first = words.first().copied().unwrap_or("");

        // Scope yang dibuka baris ini (sebelum update kedalaman).
        let pushed: Option<(String, String)> = match first {
            "module" | "interface" | "package" | "program" => {
                Some((first.to_string(), truncate(trimmed)))
            }
            "function" | "task" => {
                let name = fn_task_name(&words);
                Some((first.to_string(), format!("{} {}", first, name)))
            }
            "always_comb" | "always_ff" | "always_latch" | "always" => {
                let end = trimmed.find("begin").unwrap_or(trimmed.len());
                Some((first.to_string(), truncate(trimmed[..end].trim())))
            }
            "initial" | "final" => {
                let text = if trimmed.contains("begin") {
                    format!("{} begin", first)
                } else {
                    first.to_string()
                };
                Some((first.to_string(), text))
            }
            "begin" => {
                let mut text = first.to_string();
                if words.len() > 1 && words[1].starts_with(':') {
                    text = format!("begin {}", words[1]);
                }
                Some((first.to_string(), text))
            }
            _ => None,
        };
        if let Some((kind, text)) = pushed {
            out.push(StickyScope {
                line: line_no,
                depth,
                kind,
                text,
            });
        }

        // Update kedalaman per kata agar `end else begin` netral (end -1,
        // begin +1). Scope-end (`endmodule` dll) mereset kedalaman ke 0.
        for w in &words {
            match *w {
                "begin" => depth += 1,
                "end" => depth = depth.saturating_sub(1),
                "endmodule" | "endinterface" | "endpackage" | "endprogram" | "endfunction"
                | "endtask" => depth = 0,
                _ => {}
            }
        }
    }
    out
}

/// Scope enclosing untuk `first_line`: semua scope dengan `line <= first_line`,
/// ambil `max` terakhir (yang terdalam). Daftar scope terurut line menaik,
/// jadi `take_while` berhenti di scope pertama yang mulai setelah first_line.
fn enclosing_chain(sticky: &[StickyScope], first_line: usize, max: usize) -> Vec<&StickyScope> {
    let idx = sticky.iter().take_while(|s| s.line <= first_line).count();
    let start = idx.saturating_sub(max);
    sticky[start..idx].iter().collect()
}

/// Strip Sticky Header: rantai scope enclosing untuk baris pertama yang
/// terlihat (terluar → terdalam, maks 4). Baris paling dalam ditebalkan,
/// yang luar diredupkan, diindentasi sesuai kedalaman blok. Klik baris →
/// lompat ke baris deklarasinya (dikembalikan sebagai `Some(line)`).
fn draw_sticky_header(ui: &mut egui::Ui, f: &OpenFile, line_h: f32) -> Option<usize> {
    if f.sticky.is_empty() {
        return None;
    }
    let first_line = (f.scroll_top / line_h.max(1.0)) as usize + 1;
    let chain = enclosing_chain(&f.sticky, first_line, 4);
    if chain.is_empty() {
        return None;
    }

    let n = chain.len();
    let row_h = 18.0;
    let height = row_h * n as f32;
    let (rect, _resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    // Latar strip sedikit lebih terang dari bg editor + border tipis.
    painter.rect_filled(rect, 3.0, ui.visuals().faint_bg_color);
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        egui::StrokeKind::Inside,
    );

    let mut clicked: Option<usize> = None;
    for (i, s) in chain.iter().enumerate() {
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + i as f32 * row_h),
            egui::vec2(rect.width(), row_h),
        );
        let row_resp = ui.interact(row_rect, ui.id().with(("sticky", i)), egui::Sense::click());

        let is_inner = i + 1 == n;
        let color = if is_inner {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        if row_resp.hovered() {
            painter.rect_filled(row_rect, 2.0, egui::Color32::from_white_alpha(14));
        }

        // Titik berwarna per jenis scope (palet selaras semantic highlight).
        let dot = match s.kind.as_str() {
            "module" => egui::Color32::from_rgb(59, 130, 246), // biru
            "interface" => egui::Color32::from_rgb(168, 85, 247), // ungu
            "package" => egui::Color32::from_rgb(6, 182, 212), // cyan
            "function" | "task" => egui::Color32::from_rgb(249, 115, 22), // oranye
            "always" | "initial" => egui::Color32::from_rgb(34, 197, 94), // hijau
            _ => egui::Color32::from_rgb(148, 163, 184),       // abu (begin)
        };
        painter.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(row_rect.left() + 12.0, row_rect.center().y),
                egui::vec2(6.0, 6.0),
            ),
            1.0,
            dot,
        );

        let indent = "  ".repeat(s.depth);
        painter.text(
            egui::pos2(row_rect.left() + 24.0, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{}{}", indent, s.text),
            egui::FontId::monospace(11.0),
            color,
        );

        if row_resp
            .on_hover_text(format!("Lompat ke deklarasi (baris {})", s.line))
            .clicked()
        {
            clicked = Some(s.line);
        }
    }
    clicked
}

// ───────────────────────────── Hover Tooltip ─────────────────────────────

/// Identifikasi identifier di bawah kursor mouse pada editor. Posisi mouse
/// (screen) dipetakan ke (baris, kolom) global memakai metrik monospace +
/// offset scroll vertikal & horizontal, lalu `semantic::identifier_at` memberi
/// nama + kategori. Margin teks TextEdit = `Margin::symmetric(4.0, 2.0)`.
fn hovered_identifier(
    resp: &egui::Response,
    content: &str,
    scroll_top: f32,
    scroll_left: f32,
    char_w: f32,
    line_h: f32,
) -> Option<(String, semantic::SemKind)> {
    identifier_at_pos(
        resp,
        content,
        scroll_top,
        scroll_left,
        char_w,
        line_h,
        resp.hover_pos()?,
    )
}

/// (baris, kolom) 0-based LSP dari posisi pointer — metrik monospace + offset
/// scroll, lalu `byte_idx_at_line_col` + `line_col_at_char` yang sudah ada.
fn hover_line_col(
    resp: &egui::Response,
    content: &str,
    scroll_top: f32,
    scroll_left: f32,
    char_w: f32,
    line_h: f32,
    pos: egui::Pos2,
) -> Option<(u32, u32)> {
    let margin = egui::Margin::symmetric(4, 2);
    let origin = resp.rect.min + egui::vec2(margin.left as f32, margin.top as f32);
    let y = (pos.y - origin.y) + scroll_top;
    let x = (pos.x - origin.x) + scroll_left;
    if y < 0.0 || x < 0.0 {
        return None;
    }
    let row = (y / line_h.max(1.0)) as usize;
    let col = (x / char_w.max(1.0)) as usize;
    let byte = byte_idx_at_line_col(content, row, col);
    let char_idx = content[..byte.min(content.len())].chars().count();
    let (r0, c0) = line_col_at_char(content, char_idx);
    Some((r0 as u32, c0 as u32))
}

/// Identifier di posisi tertentu (screen). Dipakai hover tooltip & Ctrl+Click
/// (Go To Definition). Posisi dipetakan ke (baris, kolom) global via metrik
/// monospace + offset scroll, lalu `semantic::identifier_at`.
fn identifier_at_pos(
    resp: &egui::Response,
    content: &str,
    scroll_top: f32,
    scroll_left: f32,
    char_w: f32,
    line_h: f32,
    pos: egui::Pos2,
) -> Option<(String, semantic::SemKind)> {
    // Margin teks TextEdit default = `Margin::symmetric(4, 2)` (i8).
    let margin = egui::Margin::symmetric(4, 2);
    let origin = resp.rect.min + egui::vec2(margin.left as f32, margin.top as f32);
    let y = (pos.y - origin.y) + scroll_top;
    let x = (pos.x - origin.x) + scroll_left;
    if y < 0.0 || x < 0.0 {
        return None;
    }
    let row = (y / line_h.max(1.0)) as usize;
    let col = (x / char_w.max(1.0)) as usize;
    let idx = byte_idx_at_line_col(content, row, col);
    semantic::identifier_at(content, idx)
}

// ────────────────────────── Go To Definition ───────────────────────────

/// Resolve Ctrl+Click → (file target, baris deklarasi) untuk Go To Definition.
/// - module/interface/package: buka file asal (dari `symbol_files`) & lompat ke
///   baris deklarasi (`module foo ...` dst).
/// - signal/clock/reset/parameter/typedef/enum: lompat ke baris deklarasi di
///   file yang sama (heuristik teks; fallback ke kemunculan pertama).
fn resolve_goto(
    f: &OpenFile,
    name: &str,
    kind: semantic::SemKind,
    symbol_files: Option<&HashMap<String, PathBuf>>,
) -> Option<(PathBuf, Option<usize>)> {
    match kind {
        semantic::SemKind::Module | semantic::SemKind::Interface | semantic::SemKind::Package => {
            let path = symbol_files.and_then(|m| m.get(name)).cloned()?;
            let kw = match kind {
                semantic::SemKind::Module => "module",
                semantic::SemKind::Interface => "interface",
                _ => "package",
            };
            let line = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| find_decl_line(&c, kw, name));
            Some((path, line))
        }
        semantic::SemKind::Signal
        | semantic::SemKind::Clock
        | semantic::SemKind::Reset
        | semantic::SemKind::Parameter
        | semantic::SemKind::Typedef
        | semantic::SemKind::Enum => {
            let info = scan_hover_info(&f.content, name);
            let line = info
                .declared_line
                .or_else(|| first_word_line(&f.content, name));
            Some((f.path.clone(), line))
        }
        _ => None,
    }
}

/// Baris (1-based) tempat `name` dideklarasikan dengan keyword `kw` di awal
/// baris (`module foo`, `interface bus_if`, `package pkg`).
fn find_decl_line(content: &str, kw: &str, name: &str) -> Option<usize> {
    for (i, raw) in content.lines().enumerate() {
        let code = raw.split("//").next().unwrap_or(raw).trim();
        let mut it = code.split_whitespace();
        if it.next() == Some(kw) {
            let clean: String = it
                .next()
                .map(|tok| {
                    tok.trim_start_matches('#')
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect()
                })
                .unwrap_or_default();
            if clean == name {
                return Some(i + 1);
            }
        }
    }
    None
}

/// Baris (1-based) kemunculan pertama `name` sebagai kata utuh.
fn first_word_line(content: &str, name: &str) -> Option<usize> {
    for (i, raw) in content.lines().enumerate() {
        if word_count(raw.split("//").next().unwrap_or(raw), name) > 0 {
            return Some(i + 1);
        }
    }
    None
}

/// Byte index karakter pada (row 0-based, col) di konten multi-baris.
fn byte_idx_at_line_col(content: &str, row: usize, col: usize) -> usize {
    let mut idx = 0usize;
    for (i, line) in content.split('\n').enumerate() {
        if i == row {
            return (idx + col.min(line.len())).min(content.len());
        }
        idx += line.len() + 1; // + newline
    }
    content.len()
}

/// Hasil scan heuristik file aktif untuk tooltip hover.
struct HoverInfo {
    declared_line: Option<usize>,
    used_count: usize,
    last_assign_line: Option<usize>,
    docs: Option<String>,
}

/// Scan file aktif: baris deklarasi, jumlah pemakaian, assignment terakhir,
/// dan doc comment — heuristik teks (bukan LSP penuh, cukup untuk tooltip).
fn scan_hover_info(content: &str, name: &str) -> HoverInfo {
    const DECL_KW: &[&str] = &[
        "input",
        "output",
        "inout",
        "logic",
        "reg",
        "wire",
        "bit",
        "var",
        "tri",
        "integer",
        "int",
        "signed",
        "unsigned",
        "parameter",
        "localparam",
        "typedef",
        "genvar",
        "time",
        "real",
        "byte",
        "shortint",
        "longint",
    ];
    let mut declared: Option<usize> = None;
    let mut used = 0usize;
    let mut last_assign: Option<usize> = None;
    let mut docs: Option<String> = None;

    let lines: Vec<&str> = content.lines().collect();
    for (i, raw) in lines.iter().enumerate() {
        let line_no = i + 1;
        let code = raw.split("//").next().unwrap_or(raw);
        let count = word_count(code, name);
        if count > 0 {
            used += count;
            let has_decl = DECL_KW.iter().any(|kw| word_count(code, kw) > 0);
            if declared.is_none() && has_decl {
                declared = Some(line_no);
            }
            if has_assign(code, name) {
                last_assign = Some(line_no);
            }
        }
        // Doc: komentar `//` pada baris deklarasi, atau baris komentar di atasnya.
        if declared == Some(line_no) && docs.is_none() {
            if let Some(pos) = raw.find("//") {
                let c = raw[pos + 2..].trim().to_string();
                if !c.is_empty() {
                    docs = Some(truncate_doc(&c));
                }
            }
            if docs.is_none() {
                if let Some(prev) = i.checked_sub(1).and_then(|p| lines.get(p)) {
                    if let Some(c) = prev.trim_start().strip_prefix("//") {
                        let c = c.trim().to_string();
                        if !c.is_empty() {
                            docs = Some(truncate_doc(&c));
                        }
                    }
                }
            }
        }
    }
    HoverInfo {
        declared_line: declared,
        used_count: used,
        last_assign_line: last_assign,
        docs,
    }
}

/// Potong doc comment hingga ~120 karakter (jangan pecah char UTF-8).
fn truncate_doc(c: &str) -> String {
    let mut chars = c.chars();
    let mut s: String = chars.by_ref().take(120).collect();
    if chars.next().is_some() {
        s.push('…');
    }
    s
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Apakah `name` (kata utuh) di baris ini diikuti operator assignment
/// (=, <=, +=, -=, *=, /=, &=, |=, ^=, <<=, >>=)? Indeks array `[i]` antara
/// nama dan operator ikut dilewati (`data[3] <= 1` tetap terdeteksi).
fn has_assign(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let b = text.as_bytes();
    let wb = name.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    while i + wb.len() <= n {
        if b[i] == wb[0] {
            let prev_ok = i == 0 || !is_word_char(b[i - 1]);
            let end = i + wb.len();
            let next_ok = end >= n || !is_word_char(b[end]);
            if prev_ok && next_ok && &b[i..end] == wb {
                let mut j = end;
                // Lewati spasi + indeks array `[...]` berulang.
                loop {
                    while j < n && (b[j] as char).is_whitespace() {
                        j += 1;
                    }
                    if j < n && b[j] == b'[' {
                        j += 1;
                        while j < n && b[j] != b']' {
                            j += 1;
                        }
                        j = (j + 1).min(n);
                        continue;
                    }
                    break;
                }
                let rest = &text[j..];
                // Tolak operator perbandingan ==/===/!== (bukan assignment).
                if rest.starts_with("==") || rest.starts_with("!=") {
                    i = end;
                    continue;
                }
                for op in [
                    "<<=", ">>=", "<=", "+=", "-=", "*=", "/=", "&=", "|=", "^=", "=",
                ] {
                    if rest.starts_with(op) {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

/// Isi tooltip hover — sesuai desain Mivon: nama, tipe/lebar, baris deklarasi,
/// jumlah pemakaian, assignment terakhir, dan doc comment.
fn hover_tooltip_ui(
    ui: &mut egui::Ui,
    f: &OpenFile,
    name: &str,
    kind: semantic::SemKind,
    sig_info: Option<&HashMap<String, (String, usize)>>,
    ref_counts: Option<&HashMap<String, usize>>,
) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(name)
                .monospace()
                .strong()
                .size(13.0)
                .color(semantic::color(kind)),
        );
        ui.separator();

        // Baris pertama: tipe/lebar signal — atau info khusus kategori.
        match kind {
            semantic::SemKind::Module | semantic::SemKind::Interface => {
                let refs = ref_counts.and_then(|m| m.get(name)).copied().unwrap_or(0);
                let what = if kind == semantic::SemKind::Module {
                    "module"
                } else {
                    "interface"
                };
                ui.monospace(format!("{} · direferensikan {} kali", what, refs));
            }
            semantic::SemKind::Package => {
                ui.monospace("package");
            }
            semantic::SemKind::Parameter => {
                ui.monospace("parameter");
            }
            semantic::SemKind::Typedef => {
                ui.monospace("typedef");
            }
            semantic::SemKind::Enum => {
                ui.monospace("enum member");
            }
            semantic::SemKind::Type => {
                ui.monospace("tipe data");
            }
            _ => {
                // signal / clock / reset — tipe + lebar dari design (jika ada).
                match sig_info.and_then(|m| m.get(name)) {
                    Some((ty, w)) => {
                        ui.monospace(format!("{} · {} bit", ty, w));
                    }
                    None => {
                        ui.monospace("signal");
                    }
                }
            }
        }

        let info = scan_hover_info(&f.content, name);
        if let Some(line) = info.declared_line {
            ui.monospace(format!("Declared: {}:{}", f.name, line));
        }
        ui.monospace(format!("Used: {}×", info.used_count));
        if let Some(line) = info.last_assign_line {
            ui.monospace(format!("Last assign: baris {}", line));
        }
        if let Some(doc) = info.docs {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(doc).italics().weak().size(11.0));
        }
    });
}

// ─────────────────────────── Rename Symbol ────────────────────────────

/// Ganti semua kemunculan kata utuh `old` dengan `new` dalam konten.
/// Mengembalikan jumlah penggantian. Aman UTF-8: karakter non-ASCII disalin
/// utuh (via `utf8_len`) sehingga slicing tidak pernah pecah di tengah char.
fn replace_word(content: &mut String, old: &str, new: &str) -> usize {
    if old.is_empty() {
        return 0;
    }
    let src = content.clone();
    let b = src.as_bytes();
    let ob = old.as_bytes();
    let n = b.len();
    let mut out = String::with_capacity(src.len());
    let mut count = 0usize;
    let mut i = 0usize;
    while i < n {
        if i + ob.len() <= n && b[i] == ob[0] {
            let prev_ok = i == 0 || !is_word_char(b[i - 1]);
            let end = i + ob.len();
            let next_ok = end >= n || !is_word_char(b[end]);
            if prev_ok && next_ok && &b[i..end] == ob {
                out.push_str(new);
                count += 1;
                i = end;
                continue;
            }
        }
        let ch_len = utf8_len(b[i]);
        out.push_str(&src[i..i + ch_len]);
        i += ch_len;
    }
    *content = out;
    count
}

/// Panjang byte satu karakter UTF-8 dari leading byte-nya.
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

// ─────────────────────────── Autocomplete ───────────────────────────

/// Keyword SystemVerilog umum — kandidat autocomplete tingkat pertama
/// (selain symbol dari AST: module/interface/package/signal).
const SV_KEYWORDS: &[&str] = &[
    "module",
    "endmodule",
    "interface",
    "endinterface",
    "package",
    "endpackage",
    "always",
    "always_ff",
    "always_comb",
    "always_latch",
    "initial",
    "final",
    "begin",
    "end",
    "if",
    "else",
    "case",
    "casez",
    "casex",
    "default",
    "for",
    "while",
    "repeat",
    "forever",
    "fork",
    "join",
    "join_any",
    "join_none",
    "input",
    "output",
    "inout",
    "logic",
    "reg",
    "wire",
    "bit",
    "int",
    "integer",
    "byte",
    "shortint",
    "longint",
    "time",
    "real",
    "tri",
    "parameter",
    "localparam",
    "genvar",
    "assign",
    "function",
    "endfunction",
    "task",
    "endtask",
    "typedef",
    "enum",
    "struct",
    "union",
    "class",
    "endclass",
    "return",
    "break",
    "continue",
    "signed",
    "unsigned",
    "var",
    "void",
    "import",
    "export",
    "assert",
    "cover",
];

/// Kandidat autocomplete: keyword SV + module/interface/package + signal
/// (dari SEMUA module di design) + typedef/enum/parameter di file aktif.
fn completion_candidates(
    content: &str,
    modules: Option<&Vec<String>>,
    packages: Option<&Vec<String>>,
    interfaces: Option<&Vec<String>>,
    sig_info: Option<&HashMap<String, (String, usize)>>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.extend(SV_KEYWORDS.iter().map(|s| s.to_string()));
    if let Some(m) = modules {
        out.extend(m.iter().cloned());
    }
    if let Some(p) = packages {
        out.extend(p.iter().cloned());
    }
    if let Some(i) = interfaces {
        out.extend(i.iter().cloned());
    }
    if let Some(m) = sig_info {
        out.extend(m.keys().cloned());
    }
    out.extend(scan_declared_names(content));
    out.sort();
    out.dedup();
    out
}

/// Nama typedef/enum/parameter yang dideklarasikan di file aktif — scan
/// heuristik per-baris: typedef → nama terakhir sebelum ';'; parameter →
/// identifier pertama setelah keyword & tipe opsional (int/logic/[..] dll).
fn scan_declared_names(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in content.lines() {
        let code = raw.split("//").next().unwrap_or(raw).trim();
        if code.is_empty() {
            continue;
        }
        let mut it = code.split_whitespace();
        match it.next() {
            Some("typedef") => {
                // typedef [enum|struct|logic ...] nama; — token terakhir
                // sebelum ';' (setelah '}' untuk enum berisi member).
                let sans_semi = code.trim_end_matches(';');
                if let Some(tok) = sans_semi.split_whitespace().last() {
                    let clean: String = tok
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !clean.is_empty() && !SV_KEYWORDS.contains(&clean.as_str()) {
                        out.push(clean);
                    }
                }
            }
            Some("parameter") | Some("localparam") => {
                // parameter [int|logic|...] NAMA = ...; — identifier pertama
                // yang bukan tipe/keyword.
                for tok in it {
                    let clean: String = tok
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if clean.is_empty() || SV_KEYWORDS.contains(&clean.as_str()) {
                        continue;
                    }
                    out.push(clean);
                    break;
                }
            }
            _ => {}
        }
    }
    out
}

/// Byte offset karakter ke-`char_idx` (indeks karakter, bukan byte).
fn char_to_byte(content: &str, char_idx: usize) -> usize {
    content
        .char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(content.len())
}

/// (baris, kolom) 0-based dari indeks karakter.
fn line_col_at_char(content: &str, char_idx: usize) -> (usize, usize) {
    let mut seen = 0usize;
    for (i, line) in content.split('\n').enumerate() {
        let n = line.chars().count();
        if char_idx <= seen + n {
            return (i, char_idx.saturating_sub(seen));
        }
        seen += n + 1; // + newline
    }
    (content.lines().count().saturating_sub(1), 0)
}

/// Region kata (start..end byte) yang menutupi `byte_idx` (scan kata utuh
/// kiri-kanan; karakter non-ASCII bukan word char → boundary aman).
fn word_region(content: &str, byte_idx: usize) -> (usize, usize) {
    let b = content.as_bytes();
    let n = content.len();
    let idx = byte_idx.min(n);
    let mut start = idx;
    while start > 0 && is_word_char(b[start - 1]) {
        start -= 1;
    }
    let mut end = idx;
    while end < n && is_word_char(b[end]) {
        end += 1;
    }
    (start, end)
}
