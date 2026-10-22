//! Pipeline legacy `run()` — pipeline serial (preprocess paralel per-file +
//! MICD preprocess cache → lexer → parser → elaborator → engine). Dipindah
//! dari `main.rs` monolitik; 1 file = 1 tanggung jawab (jalur legacy saja).
//! Jalur cepat (`run_fast`, CompileSession + MICD penuh) di `run_fast.rs`.

use std::path::{Path, PathBuf};

use crate::anim_glue::{
    anim_abort, anim_active, anim_enabled, anim_finish, anim_log, anim_phase_done,
    anim_phase_running, anim_set_cached, anim_set_exts, anim_set_files, anim_set_modules,
    anim_set_top,
};
use crate::cli::Cli;
#[cfg(feature = "formal")]
use crate::formal_run::run_formal;
use crate::pipeline_config::{micd_root_for, pick_elab_mode};
use crate::sim_finish;
use crate::source_io::{content_has_timescale, read_source_bytes};
// run() memanggil jalur cepat (di file sendiri setelah pemecahan). Binding
// name sama → body verbatim tanpa path-rewrite.
use crate::run_fast::run_fast;
use crate::{
    DEFAULT_MAX_TIME_NS, apply_engine_flags, elab_abort_diag, emit_diags, print_ir_mem_stats,
    trim_heap, truncate_ellipsis,
};

use mivon_api::debugger::Debugger;
use mivon_api::read_project_file;
use mivon_api::simulator::Breakpoint;
use mivon_api::simulator::DebugMode;
use mivon_api::simulator::SimulationEngine;
use mivon_api::simulator::Watchpoint;
use mivon_api::waveform::VcdWriter;
use mivon_core::animasi::{Phase, PipelineAnimator};
use mivon_core::diagnostics::DiagCode;
use mivon_core::diagnostics::DiagLevel;
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::Elaborator;
use mivon_ir::LogicVec;
use mivon_parser::lexer::Lexer;
use mivon_parser::preprocessor::Preprocessor;
use mivon_parser::Parser;
use rayon::prelude::*;

/// Pipeline legacy — pemindahan verbatim dari `main.rs` (hanya visibilitas
/// dari `fn` ke `pub(crate) fn` supaya bisa diimpor root).
pub(crate) fn run(cli: Cli, env: &mut mivon_api::env::GlobalEnv) -> Result<(), SimError> {
    env.telemetry().metrics.inc_build();
    env.telemetry().trace("run", "pipeline legacy dimulai");
    let mut sources: Vec<String> = cli.files.clone();

    // Read file list from -f
    if let Some(ref fpath) = cli.filelist {
        let flist = read_project_file(fpath)?;
        sources.extend(flist);
        // [foreign] di-load di run_fast() — filelist otomatis masuk fast
        // pipeline (line ~691); jalur legacy (.mv inline) memakai CLI flags
        // --vhpi/--pli eksplisit.
    }

    // ── F8: `run x.mv` — transpile on-the-fly ke buffer (tanpa menulis file) ──
    // File `.mv`/`.mvh` (F43) di-transpile (lex → parse → check → codegen)
    // menjadi satu buffer SV (svh + sv, baris `` `include `` di-strip) lalu
    // disuntikkan sebagai sumber inline; pipeline normal berjalan tanpa
    // menyentuh disk.
    let mut inline_src: std::collections::HashMap<PathBuf, Vec<u8>> =
        std::collections::HashMap::new();
    let mv_files: Vec<String> = sources
        .iter()
        .filter(|s| {
            Path::new(s)
                .extension()
                .map(|e| e == "mv" || e == "mvh")
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    // ── F9: transpile SEMUA .mv/.mvh sekaligus (konteks gabungan lintas file) ──
    // `types.mvh` mendefinisikan package, `counter.mv` memakainya — keduanya
    // di-transpile bersama agar `use pkg::*` antar-file lolos type-check.
    if !mv_files.is_empty() {
        let mut items: Vec<mivon_api::mv::MvItem> = Vec::with_capacity(mv_files.len());
        for p in &mv_files {
            let base = Path::new(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("design")
                .to_string();
            let src = std::fs::read_to_string(p)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", p, e)))?;
            let header = Path::new(p)
                .extension()
                .map(|e| e == "mvh")
                .unwrap_or(false);
            // `mivon run x.mv` (F8) memakai emisi default ($unit) — sama
            // dengan `mgen x.mv` tanpa `--package`.
            items.push(mivon_api::mv::MvItem::new(src, base).with_header(header));
        }
        let results = mivon_api::mv::transpile_many_items(&items).map_err(|(i, e)| {
            SimError::with_diag(
                DiagCode::InvalidSyntax,
                mivon_api::mv::format_error(&mv_files[i], &items[i].src, &e),
            )
        })?;
        // Defensif: hasil batch harus sejajar dengan input (jangan zip-truncate).
        assert_eq!(
            results.len(),
            mv_files.len(),
            "transpile_many harus mengembalikan hasil sejajar dengan input"
        );
        // ── §9A.2 warning hazard bawaan (stderr, non-blokir) ──
        {
            let mut n_warn = 0usize;
            for ((p, item), tr) in mv_files.iter().zip(items.iter()).zip(results.iter()) {
                for w in &tr.warnings {
                    eprintln!("{}", mivon_api::mv::format_warning(p, &item.src, w));
                    n_warn += 1;
                }
            }
            if n_warn > 0 {
                eprintln!("── hazard: {n_warn} warning ──");
            }
        }
        for (p, tr) in mv_files.iter().zip(results.iter()) {
            // Gabung .svh + .sv jadi satu buffer; buang baris `` `include "x.svh" ``
            // (definisi bersama sudah ada di atasnya).
            let mut buf = tr.svh.clone();
            buf.push('\n');
            for line in tr.sv.lines() {
                let t = line.trim_start();
                // Strip hanya baris `` `include ... `` (definisi bersama sudah
                // ada di svh di atasnya). Jangan strip baris backtick lain
                // (mis. `` `define X_INCLUDE_Y ``) — heuristik harus tepat.
                if t.starts_with("`include") {
                    continue;
                }
                buf.push_str(line);
                buf.push('\n');
            }
            inline_src.insert(PathBuf::from(p), buf.into_bytes());
        }
    }
    if !mv_files.is_empty() && !cli.quiet {
        eprintln!(
            "[MV] transpiled {} .mv/.mvh file(s) on-the-fly (F9)",
            mv_files.len()
        );
    }

    if sources.is_empty() && !cli.gui {
        return Err(SimError::with_diag(
            mivon_core::diagnostics::DiagCode::InvalidSyntax,
            "no input files: berikan file .sv atau gunakan `--filelist <file>` (bantuan: `mivon --help`)",
        ));
    }
    env.telemetry().metrics.add_files(sources.len() as u64);

    // Create shared preprocessor with CLI config
    let mut base_pp = Preprocessor::new();
    base_pp.quiet = cli.quiet;
    for path in &cli.incdirs {
        base_pp.add_search_path(path);
    }
    for def in &cli.defines {
        if let Some((name, value)) = def.split_once('=') {
            base_pp.define(name, value);
        } else {
            base_pp.define(def, "");
        }
    }
    // ── Fast pipeline via CompileSession (skip legacy pipeline entirely) ──
    // Auto-use fast pipeline when filelist is specified (legacy can't handle large file sets)
    // Also skip expensive auto-incdir scanning for the fast path.
    // Catatan: bila ada sumber .mv (inline F8), jalur legacy dipakai —
    // run_fast membaca file dari disk dan tidak tahu buffer inline.
    // PERF-1 (fix): auto-fast untuk FILE BESAR (>256 KB) — legacy `run` serial
    // + auto-incdir scan memakan 30–42s utk satu file 21k baris (opentitan
    // i3c_reg_top.sv) karena scan ribuan file + elaborasi serial; run_fast
    // (CompileSession paralel + MICD) menyelesaikannya <1s. Mode print
    // (--tokens/--ast) dan inline .mv tetap legacy.
    let total_src_bytes: u64 = sources
        .iter()
        .filter_map(|s| std::fs::metadata(s).ok())
        .map(|m| m.len())
        .sum();
    // `.mv` di-transpile on-the-fly ke buffer inline; `run_fast` tidak
    // melakukan transpile, jadi jalur legacy WAJIB dipakai di sini.
    if inline_src.is_empty() {
        if (cli.fast || cli.filelist.is_some() || total_src_bytes > 256 * 1024)
            && !cli.print_tokens
            && !cli.print_ast
        {
            return run_fast(cli, None, env);
        }
    } else if cli.fast || cli.filelist.is_some() {
        // Jangan diam-diam mengabaikan flag: user meminta jalur cepat, tapi
        // `--fast`/`-f` tidak kompatibel dengan transpile `.mv` on-the-fly.
        eprintln!(
            "[warn] --fast/--filelist diabaikan: input mengandung .mv/.mvh yang \
             perlu transpile on-the-fly (tidak didukung jalur cepat). \
             Jalur legacy dipakai. Garis besar: implementasikan transpile di \
             `run_fast` untuk mengaktifkan kembali."
        );
    }

    // ── Animasi pipeline (baris status gaya Cargo) ──
    // Start di sini = tepat setelah mode jalur ditentukan dan sebelum kerja
    // nyata (include scan + preprocess) — tidak ada output animasi sebelumnya.
    let mut anim = PipelineAnimator::start(anim_enabled(&cli));
    if anim_active(&anim) {
        anim_set_exts(&anim, &sources);
        anim_set_files(&anim, sources.len() as u64, 0);
    }

    // Auto-detect include paths: consolidated single-pass scan
    // Walk up from each source dir's ancestors, recursively scan for SV files (depth ≤ 4)
    // NOTE: This is expensive so only runs for the legacy pipeline, not the fast path.
    // GATE: scan hanya bila ADa sumber yang memuat `` `include `` (tanggal 9s
    // utk header tanpa module — cva6 assign.svh). Tanpa include, scan mubazir.
    let sources_need_include_scan = sources.iter().any(|src| {
        std::fs::read_to_string(src)
            .map(|s| s.contains("`include"))
            .unwrap_or(false)
    });
    let mut seen_dirs = std::collections::HashSet::new();
    let mut src_dirs = std::collections::HashSet::new();
    for src in &sources {
        if let Some(dir) = std::path::Path::new(src).parent() {
            let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            src_dirs.insert(canonical);
        }
    }
    fn collect_sv_dirs(
        dir: &std::path::PathBuf,
        base_pp: &mut Preprocessor,
        seen: &mut std::collections::HashSet<PathBuf>,
        depth: usize,
    ) {
        if depth > 4 {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    let path = entry.path();
                    if ft.is_dir() && depth < 4 {
                        // Dedup direktori (seen dipakai bersama utk file dan
                        // ancestor) — tanpa ini, dir yang sama di-scan ulang
                        // dari setiap ancestor path yang tumpang tindih.
                        if seen.insert(path.clone()) {
                            collect_sv_dirs(&path, base_pp, seen, depth + 1);
                        }
                    } else if ft.is_file() {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if (ext == "svh" || ext == "sv") && seen.insert(path.clone()) {
                            if let Some(parent) = path.parent() {
                                base_pp.add_search_path(parent.to_str().unwrap());
                            }
                        }
                    }
                }
            }
        }
    }
    for src_dir in &src_dirs {
        let mut anc = Some(src_dir.clone());
        // Cap ascend utk project non-git (mis. /home/<user>/proj tanpa .git):
        // tanpa batas, crawl naik sampai root dan meng-scan seluruh HOME
        // depth-4 → run terlihat hang. Projek pribadi (AetherX dst.) memakai
        // .git/.mivon/symlink — batas 10 aman untuk include tree normal.
        let mut ascend = 0usize;
        while let Some(ref d) = anc {
            if ascend >= 10 {
                break;
            }
            ascend += 1;
            if !seen_dirs.insert(d.clone()) {
                break;
            }
            if !sources_need_include_scan {
                break;
            }
            if let Ok(entries) = std::fs::read_dir(d) {
                for entry in entries.flatten() {
                    if let Ok(ft) = entry.file_type() {
                        let path = entry.path();
                        if ft.is_dir() {
                            collect_sv_dirs(&path, &mut base_pp, &mut seen_dirs, 0);
                        }
                    }
                }
            }
            // FIX: stop ascend di project root — root proyek sudah di-scan (descend
            // depth 0..4 utk include headers), naik lebih jauh = crawl keluar
            // proyek (single-file run dari dalam repo sempat naik sampai
            // `/home/<user>` dan meng-scan seluruh HOME depth 4 — jutaan file,
            // run terlihat hang). Marker root: `.git` (repo) ATAU `.mivon`
            // (project mivon tanpa git — mis. AetherX milik user).
            if d.join(".git").exists() || d.join(".mivon").exists() {
                break;
            }
            let parent = d.parent().map(|p| p.to_path_buf());
            anc = parent;
        }
    }

    // Combine all sources (parallel preprocessing for many files)
    // ── MICD: hasil preprocess di-cache per file (konten-hash). File yang
    // tidak berubah di-reuse → preprocessor di-skip. Database scoped per
    // project (ProjectID) agar tidak tercampur antar project. ──
    let micd_root = micd_root_for(&cli);
    let proot = std::env::current_dir().unwrap_or_default();
    let src_paths: Vec<std::path::PathBuf> = sources.iter().map(std::path::PathBuf::from).collect();
    // Fase 6: pid mencakup libfiles (sama alasan dengan run_fast).
    let mut pid_paths = src_paths.clone();
    for lib in &cli.libfiles {
        let p = std::path::PathBuf::from(lib);
        if !pid_paths.contains(&p) {
            pid_paths.push(p);
        }
    }
    let pid = mivon_compiler::micd::MicdDatabase::project_id(
        &proot,
        &pid_paths,
        &cli.incdirs.iter().map(PathBuf::from).collect::<Vec<_>>(),
        &cli.defines
            .iter()
            .filter_map(|d| d.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    );
    let mut micd = mivon_compiler::micd::MicdDatabase::open_project_with_context(
        &micd_root, &pid, &proot, &src_paths,
    );
    // `--cache-clear` atau `--recompile`: hapus MICD SEBELUM reuse, agar run
    // ini benar-benar full-rebuild (lihat juga blok attach_micd untuk jalur
    // run_fast). `--recompile` di legacy sebelumnya tidak menghapus cache →
    // data lama bisa stale (Gap #12).
    if cli.cache_clear || cli.recompile {
        let _ = micd.clear();
    }

    let mut combined = String::new();
    let mut design_timescale = None;
    // F84 (LRM §19.8): segmen `timescale dalam koordinat source gabungan —
    // delay tiap module diskalakan dengan satuan modul sendiri.
    let mut timescale_segments: Vec<(usize, (String, String))> = Vec::new();

    // Slot per source; diisi dari MICD bila cache valid (konten + include sama).
    // Isi tuple = (combined, `timescale global, segmen `timescale per baris) — F84:
    // segmen dibutuhkan parser supaya delay tiap module diskalakan dengan
    // satuan modul SENDIRI (LRM §19.8), bukan satuan global.
    let mut pp_combined: Vec<
        Option<
            Result<
                (
                    String,
                    Option<(String, String)>,
                    Vec<(usize, (String, String))>,
                ),
                String,
            >,
        >,
    > = vec![None; sources.len()];
    let mut micd_reused = 0usize;
    let mut need_preprocess: Vec<(usize, &String)> = Vec::new();
    for (idx, path) in sources.iter().enumerate() {
        if let Ok(content) = read_source_bytes(Path::new(path), &inline_src) {
            let h = mivon_compiler::cache::compute_checksum(&content);
            // Koreksi correctness: jangan reuse bila header include berubah.
            let deps_ok = micd
                .deps_unchanged(std::path::Path::new(path), h)
                .unwrap_or(false);
            if deps_ok {
                if let Some(entry) = micd.get_preprocessed(std::path::Path::new(path), h) {
                    // F84: entry yang TIDAK punya segmen `timescale` tak boleh
                    // dipakai. Directive `timescale sudah di-consume saat
                    // preprocess, jadi dari `combined` saja satuan per-module
                    // tak bisa diturunkan — memakai entry itu = delay module
                    // `1us` diskalakan seolah `1ns`, SALAH SENYAP. Entry lama
                    // (Sebelum F84) dan entry yang ditulis `CompileSession`
                    // (yang tak menyimpan segmen) sama-sama kena → paksa
                    // preprocess ulang, yang biayanya kecil dibanding sim
                    // yang diam-diam salah.
                    let has_segments = !entry.timescale_segments.is_empty();
                    let source_has_directive = content_has_timescale(&content);
                    if has_segments || !source_has_directive {
                        pp_combined[idx] = Some(Ok((
                            entry.combined,
                            entry.timescale,
                            entry.timescale_segments,
                        )));
                        micd_reused += 1;
                        continue;
                    }
                }
            }
        }
        need_preprocess.push((idx, path));
    }

    if !anim_active(&anim) {
        eprintln!(
            "[TIMING] Preprocessing {} files ({} via MICD cache)...",
            sources.len(),
            micd_reused
        );
    }
    if anim_active(&anim) {
        anim_phase_running(&anim, Phase::Lex);
        // File yang reuse MICD cache sudah "selesai" di loop deteksi di atas.
        anim_set_files(&anim, sources.len() as u64, micd_reused as u64);
        anim_set_cached(&anim, micd_reused as u64);
    }
    let pp_start = std::time::Instant::now();
    let pp_for_parallel = &base_pp;
    // Progres per-file dari worker rayon (clone handle sekali, bukan per-file).
    let anim_h = anim.as_ref().map(|a| a.handle());
    let fresh_results: Vec<
        Result<
            (
                usize,
                String,
                Option<(String, String)>,
                Vec<(usize, (String, String))>,
                Vec<PathBuf>,
            ),
            String,
        >,
    > = need_preprocess
        .par_iter()
        .map(|(idx, path)| {
            let mut pp = pp_for_parallel.clone();
            // Sumber inline (.mv hasil transpile F8) — preprocess dari buffer
            // (tanpa include; definisi bersama sudah digabung di atasnya).
            if let Some(bytes) = inline_src.get(Path::new(path)) {
                let text = String::from_utf8_lossy(bytes).to_string();
                return match pp.preprocess(&text, None) {
                    Ok(processed) => {
                        if let Some(h) = &anim_h {
                            h.file_done(Path::new(path), false);
                        }
                        let combined_str = format!("`line 1 \"{}\"\n{}\n", path, processed);
                        Ok((
                            *idx,
                            combined_str,
                            pp.timescale.clone(),
                            pp.timescale_segments.clone(),
                            Vec::new(),
                        ))
                    }
                    Err(e) => Err(format!("preprocessor '{}': {}", path, e)),
                };
            }
            match pp.preprocess_file(path) {
                Ok(processed) => {
                    if let Some(h) = &anim_h {
                        h.file_done(Path::new(path), false);
                    }
                    let combined_str = format!("`line 1 \"{}\"\n{}\n", path, processed);
                    let includes: Vec<PathBuf> = pp.resolved_includes.iter().cloned().collect();
                    Ok((
                        *idx,
                        combined_str,
                        pp.timescale.clone(),
                        pp.timescale_segments.clone(),
                        includes,
                    ))
                }
                Err(e) => Err(format!("preprocessor '{}': {}", path, e)),
            }
        })
        .collect();
    anim_log(
        &anim,
        &format!("[TIMING] Preprocessing done in {:?}", pp_start.elapsed()),
    );

    for r in &fresh_results {
        match r {
            Ok((idx, combined_str, ts, segs, _includes)) => {
                pp_combined[*idx] = Some(Ok((combined_str.clone(), ts.clone(), segs.clone())))
            }
            Err(e) => {
                return Err(SimError::with_diag(
                    DiagCode::PreprocessorError,
                    format!("preprocessing failed: {}", e),
                ));
            }
        }
    }

    for (i, _path) in sources.iter().enumerate() {
        match &pp_combined[i] {
            Some(Ok((combined_str, ts, segs))) => {
                if let Some(ts) = ts {
                    design_timescale = Some(ts.clone());
                }
                // F84: geser segmen ke koordinat `combined` (offset kumulatif
                // baris yang sudah ditulis). Parser nanti menempelkan satuan
                // per-module dari daftar ini.
                let content_offset = combined.lines().count() + 1;
                for (from, ts_pair) in segs {
                    timescale_segments.push((from + content_offset, ts_pair.clone()));
                }
                combined.push_str(combined_str);
            }
            Some(Err(e)) => {
                return Err(SimError::with_diag(
                    DiagCode::PreprocessorError,
                    format!("preprocessing failed: {}", e),
                ));
            }
            None => {
                return Err(SimError::with_diag(
                    DiagCode::PreprocessorError,
                    format!("no preprocessed output for source #{}", i),
                ));
            }
        }
    }

    // ── MICD: simpan hasil preprocess baru untuk run berikutnya ──
    for (idx, combined_str, ts, segs, includes) in fresh_results.iter().flatten() {
        if let Ok(content) = read_source_bytes(Path::new(&sources[*idx]), &inline_src) {
            let h = mivon_compiler::cache::compute_checksum(&content);
            let path = std::path::PathBuf::from(&sources[*idx]);
            micd.cache_preprocessed(
                path.clone(),
                mivon_compiler::micd::PreprocEntry {
                    content_hash: h,
                    combined: combined_str.clone(),
                    timescale: ts.clone(),
                    // F84: segmen ikut di-cache agar run hangat tak kehilangan
                    // satuan per-module.
                    timescale_segments: segs.clone(),
                },
            );
            // Metadata + include hashes (verifikasi header saat reuse).
            let include_hashes: Vec<(std::path::PathBuf, u64)> = includes
                .iter()
                .map(|inc| {
                    let hh = std::fs::read(inc)
                        .map(|b| mivon_compiler::cache::compute_checksum(&b))
                        .unwrap_or(0);
                    (inc.clone(), hh)
                })
                .collect();
            // Ukuran memakai isi sumber yang BENAR-BENAR di-hash (buffer
            // inline untuk .mv, file disk untuk .sv) agar konsisten dengan
            // `content_hash` — bukan metadata disk (yang untuk .mv adalah
            // ukuran sumber .mv asli, bukan buffer transpile).
            let size = content.len() as u64;
            micd.record_file(
                path,
                h,
                vec![],
                mivon_compiler::micd::FileStatus::Unchanged,
                0,
                size,
                include_hashes,
            );
            // verify.mdb: jalur legacy juga menandai file terverifikasi
            // (parse) agar store lengkap walau tanpa `--fast`.
            let mut v = mivon_compiler::micd::VerifyResult::fresh(h);
            v.parse_ok = true;
            v.set_check(
                mivon_compiler::micd::VerifyCheckKind::Parse,
                mivon_compiler::micd::CheckResult::pass(0),
            );
            micd.set_verify(v);
        }
    }
    // Fase 6: rekam libfiles ke MICD di bawah (setelah parse libfiles),
    // agar sidik input IR mencakupnya — libfile berubah → restore IR
    // ditolak (anti stale). flags_hash diset dari defines agar sidik stabil.
    // Save ditunda ke akhir run (setelah symbol/type/graph + prune_stale)
    // agar hanya satu save transaksional per build (Gap #8).

    if !anim_active(&anim) {
        eprintln!(
            "[TIMING] Starting lexer (combined size: {} bytes)...",
            combined.len()
        );
    }
    let lex_start = std::time::Instant::now();
    let mut lexer = Lexer::new(&combined);
    let mut tokens = Vec::new();

    loop {
        let (tok, line, col) = lexer.next_token();
        if cli.print_tokens {
            println!("  {:4}:{:4} {}", line, col, tok);
        }
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    // Fase 2b: payload lexer/ tak lagi ditulis (tak ada pembaca produksi) —
    // blok cache lexer dihapus (hemat observasi per-token tiap build).
    anim_log(
        &anim,
        &format!(
            "[TIMING] Lexer done: {} tokens in {:?}",
            tokens.len(),
            lex_start.elapsed()
        ),
    );
    if anim_active(&anim) {
        anim_phase_done(&anim, Phase::Lex);
        anim_phase_running(&anim, Phase::Par);
    }

    if tokens.is_empty() {
        // File header/macro-only (`` `define ``/komentar, tanpa module) —
        // desain kosong VALID (SV legal). Bukan error: lanjut jalur normal,
        // elaborator melaporkan "no modules found" sebagai NOTE.
        eprintln!("[mivon] no module declarations in design (header/macro-only file)");
        return Ok(());
    }

    let first_source = sources.first().map(|s| s.as_str()).unwrap_or("<unknown>");

    let file_line_map = lexer.file_line_map.clone();
    if !anim_active(&anim) {
        eprintln!("[TIMING] Starting parser...");
    }
    let parse_start = std::time::Instant::now();
    let mut parser = Parser::new(tokens, first_source)
        .with_source_lines(&combined)
        .with_file_line_map(file_line_map);
    // F84 (LRM §19.8): tanpa ini semua delay diskalakan dengan satuan global
    // (`design_timescale` = directive terakhir), sehingga `#5` di module `1us`
    // diperlakukan 5 tick basis `1ns`.
    parser.timescale_segments = timescale_segments.clone();
    let mut design = match parser.parse_design() {
        Ok(d) => {
            if anim_active(&anim) {
                anim_phase_done(&anim, Phase::Par);
                anim_set_modules(&anim, d.modules.len() as u64);
                anim_set_files(&anim, sources.len() as u64, sources.len() as u64);
            }
            anim_log(
                &anim,
                &format!("[TIMING] Parser done in {:?}", parse_start.elapsed()),
            );
            d
        }
        Err(e) => {
            // Emit hanya WARNING di sini; error dicetak SATU kali oleh
            // top-level handler (hindari duplikasi diagnostic line:col).
            // Tanpa clone: closure meminjam `parser.errors` langsung — `anim`
            // dan `parser` objek terpisah, tidak ada konflik borrow.
            if parser.errors.iter().any(|d| !d.is_error()) {
                let emit_warnings = || {
                    let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
                    for diag in &parser.errors {
                        if !diag.is_error() {
                            let _ = emitter.emit(diag);
                        }
                    }
                };
                match anim.as_ref() {
                    Some(a) if a.is_active() => a.suspend_print(emit_warnings),
                    _ => emit_warnings(),
                }
            }
            return Err(e);
        }
    };
    // Emit parser diagnostics (warnings) — errors ditangani top-level sekali.
    // In compile-only mode, individual construct errors are non-fatal.
    if parser.errors.iter().any(|d| !d.is_error()) {
        let emit_warnings = || {
            let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
            for diag in &parser.errors {
                if !diag.is_error() {
                    let _ = emitter.emit(diag);
                }
            }
        };
        match anim.as_ref() {
            Some(a) if a.is_active() => a.suspend_print(emit_warnings),
            _ => emit_warnings(),
        }
    }
    if parser.errors.iter().any(|d| d.is_error()) && !cli.compile_only {
        // Tutup ringkasan animasi dengan jumlah error nyata. Sebelumnya jalur
        // ini `return` langsung sehingga baris "Compile Failed" hilang sama
        // sekali (animator di-Drop tanpa finish) — padahal run_fast
        // menampilkannya. Samakan kedua jalur (hanya level Error yg dihitung).
        if anim_active(&anim) {
            let e = parser.errors.iter().filter(|d| d.is_error()).count();
            let w = parser
                .errors
                .iter()
                .filter(|d| d.level == DiagLevel::Warning)
                .count();
            anim_abort(&mut anim, e, w);
        }
        return Err(mivon_core::error::SimError::from_parse_diagnostic(
            parser.errors[0].clone(),
        ));
    }
    // F84 (LRM §19.8): JANGAN timpa `design.timescale` dengan directive terakhir
    // (`design_timescale`) — parser sudah mengisinya dengan satuan TERHALUS,
    // dan nilai itulah basis diskalakan delay tiap module. Menimpanya dengan
    // `1us` terakhir membuat module `1ns` ikut skala 1us (faktor 1) dan delay
    // design meleset 1000×. Nilai terakhir tetap dipakai `ts_for_ir` untuk
    // konsistensi metadata, tapi TIDAK untuk basis elaborasi.
    let ts_for_ir = design_timescale.clone();
    if design.timescale.is_none() {
        // Tanpa directive sama sekali → tak ada segmen; basis default elaborator
        // (`None` → 1ns) sudah benar. Fallback ini hanya bila parser gagal
        // menyetel (mis. jalur yang tak memasang segmen).
        design.timescale = design_timescale;
    }

    // ── Library scanning: always scan library directories/files before elaboration ──
    for libdir in &cli.libdirs {
        base_pp.add_search_path(libdir);
        if let Ok(entries) = std::fs::read_dir(libdir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "v" || ext == "sv" {
                        let mut pp = base_pp.clone();
                        let path_str = path.to_string_lossy().to_string();
                        match pp.preprocess_file(&path_str) {
                            Ok(processed) => {
                                let combined_lib =
                                    format!("`line 1 \"{}\"\n{}", path.display(), processed);
                                let mut lexer = Lexer::new(&combined_lib);
                                let mut lib_tokens = Vec::new();
                                loop {
                                    let (tok, line, col) = lexer.next_token();
                                    if tok == mivon_parser::lexer::Token::Eof {
                                        break;
                                    }
                                    lib_tokens.push((tok, line, col));
                                }
                                let mut parser =
                                    Parser::new(lib_tokens, path.to_str().unwrap_or("<lib>"));
                                parser = parser.with_source_lines(&combined_lib);
                                match parser.parse_design() {
                                    Ok(lib_design) => {
                                        for m in lib_design.modules {
                                            if !design.modules.iter().any(|dm| dm.name == m.name) {
                                                design.modules.push(m);
                                            }
                                        }
                                    }
                                    Err(e) => eprintln!(
                                        "warning: library file '{}' parse error: {}",
                                        path.display(),
                                        e
                                    ),
                                }
                            }
                            Err(e) => eprintln!(
                                "warning: library file '{}' preprocess error: {}",
                                path.display(),
                                e
                            ),
                        }
                    }
                }
            }
        }
    }
    // Fase 6: libfiles direkam ke MICD di bawah (setelah parse) agar sidik
    // input IR mencakupnya. Kumpulkan libfile yang gagal parse/preprocess.
    let mut lib_parse_failed = std::collections::HashSet::new();
    for libfile in &cli.libfiles {
        let mut pp = base_pp.clone();
        let libfile_path = std::path::Path::new(libfile);
        if let Some(dir) = libfile_path.parent() {
            if let Some(dir_str) = dir.to_str() {
                base_pp.add_search_path(dir_str);
            }
        }
        match pp.preprocess_file(libfile) {
            Ok(processed) => {
                let combined_lib = format!("`line 1 \"{}\"\n{}", libfile, processed);
                let mut lexer = Lexer::new(&combined_lib);
                let mut lib_tokens = Vec::new();
                loop {
                    let (tok, line, col) = lexer.next_token();
                    if tok == mivon_parser::lexer::Token::Eof {
                        break;
                    }
                    lib_tokens.push((tok, line, col));
                }
                let mut parser = Parser::new(lib_tokens, libfile);
                parser = parser.with_source_lines(&combined_lib);
                match parser.parse_design() {
                    Ok(lib_design) => {
                        for m in lib_design.modules {
                            if !design.modules.iter().any(|dm| dm.name == m.name) {
                                design.modules.push(m);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("warning: library file '{}' parse error: {}", libfile, e);
                        lib_parse_failed.insert(libfile.clone());
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "warning: library file '{}' preprocess error: {}",
                    libfile, e
                );
                lib_parse_failed.insert(libfile.clone());
            }
        }
    }

    // Fase 6: rekam libfiles ke MICD (hash + status parse jujur) agar sidik
    // input IR mencakupnya — libfile berubah → restore IR ditolak.
    // flags_hash diset dari defines agar sidik stabil lintas run sama.
    {
        let flag_h = mivon_compiler::micd::flags_hash(
            &cli.defines
                .iter()
                .filter_map(|d| d.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>(),
            &cli.incdirs.iter().map(PathBuf::from).collect::<Vec<_>>(),
        );
        micd.flags_hash = flag_h;
        for libfile in &cli.libfiles {
            if let Ok(content) = std::fs::read(libfile) {
                let h = mivon_compiler::cache::compute_checksum(&content);
                let path = std::path::PathBuf::from(libfile);
                micd.record_file(
                    path,
                    h,
                    vec![],
                    mivon_compiler::micd::FileStatus::Unchanged,
                    flag_h,
                    content.len() as u64,
                    vec![],
                );
                let mut v = mivon_compiler::micd::VerifyResult::fresh(h);
                v.parse_ok = !lib_parse_failed.contains(libfile);
                micd.set_verify(v);
            }
        }
    }

    // ── MICD legacy: lengkapi store symbol/type/graph + save terpadu. ──
    // Jalur `run` (legacy) menggabungkan semua source jadi satu design.
    // Simbol → file di-atribusi via scan teks (`module X`, `package X`, dst)
    // karena AST legacy tidak membawa info file. Satu save di akhir
    // (menggantikan 3 save terpisah sebelumnya — Gap #8) + prune_stale
    // (Gap #10) + auto-snapshot (Gap #4).
    {
        use mivon_ast::ModuleItem;
        let mut syms: Vec<(String, String)> = Vec::new();
        for m in &design.modules {
            syms.push((m.name.to_string(), "module".to_string()));
        }
        for p in &design.packages {
            syms.push((p.name.to_string(), "package".to_string()));
        }
        for i in &design.interfaces {
            syms.push((i.name.to_string(), "interface".to_string()));
        }
        for c in &design.classes {
            syms.push((c.name.to_string(), "class".to_string()));
        }
        let mut def_file: std::collections::HashMap<String, PathBuf> =
            std::collections::HashMap::new();
        let mut all_src: Vec<PathBuf> = sources.iter().map(PathBuf::from).collect();
        all_src.extend(cli.libfiles.iter().map(PathBuf::from));
        for src in &all_src {
            // Baca dari buffer inline untuk file `.mv` (F8) agar teks yang
            // di-scan adalah SV hasil transpile (pola `module X`, `package X`),
            // bukan sintaks `.mv` asli — atribusi simbol tetap akurat.
            let text = read_source_bytes(src, &inline_src)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).to_string());
            if let Some(text) = text {
                for (name, kind) in &syms {
                    if def_file.contains_key(name) {
                        continue;
                    }
                    if text.contains(&format!("{} {}", kind, name)) {
                        def_file.insert(name.clone(), src.clone());
                    }
                }
            }
        }
        let fallback = sources.first().map(PathBuf::from).unwrap_or_default();
        for m in &design.modules {
            let mut sig = 0u64;
            sig = sig
                .wrapping_mul(31)
                .wrapping_add(mivon_compiler::cache::compute_checksum(
                    m.name.as_str().as_bytes(),
                ));
            for p in &m.ports {
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add(mivon_compiler::cache::compute_checksum(
                        p.name.as_str().as_bytes(),
                    ));
            }
            for pr in &m.params {
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add(mivon_compiler::cache::compute_checksum(
                        pr.name.as_str().as_bytes(),
                    ));
            }
            micd.set_module_type(m.name.to_string(), sig);
            let file = def_file
                .get(m.name.as_str())
                .cloned()
                .unwrap_or_else(|| fallback.clone());
            micd.add_symbol(m.name.to_string(), "module".to_string(), file.clone());
        }
        for (name, kind) in &syms {
            let file = def_file
                .get(name)
                .cloned()
                .unwrap_or_else(|| fallback.clone());
            // Definisikan di graph (level simbol) → graph.mdb selalu terisi
            // walau tidak ada instance/import lintas file.
            micd.set_symbol_def(name.clone(), file.clone());
            if kind != "module" {
                micd.add_symbol(name.clone(), kind.clone(), file.clone());
            }
        }
        // graph.mdb: file deps via instance + import.
        for m in &design.modules {
            let file = def_file
                .get(m.name.as_str())
                .cloned()
                .unwrap_or_else(|| fallback.clone());
            let mut deps: Vec<PathBuf> = Vec::new();
            for item in &m.items {
                match item {
                    ModuleItem::Instance(inst) => {
                        if let Some(f) = def_file.get(inst.module_name.as_str()) {
                            if *f != file && !deps.contains(f) {
                                deps.push(f.clone());
                            }
                            micd.set_symbol_def(inst.module_name.to_string(), f.clone());
                            micd.add_symbol_use(file.clone(), inst.module_name.to_string());
                        }
                    }
                    ModuleItem::Import { package, item } => {
                        if let Some(f) = def_file.get(package.as_str()) {
                            if *f != file && !deps.contains(f) {
                                deps.push(f.clone());
                            }
                            if item.as_str() != "*" {
                                micd.add_symbol_use(file.clone(), item.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !deps.is_empty() {
                micd.set_file_deps(file, deps);
            }
        }

        // ── Lapisan cache pipeline (db.md cache/, baris 1141-1605) — jalur
        // legacy (tanpa --fast). Isi kategori dari design merged + state MICD
        // hanya saat ada file fresh (changed); store disimpan di save terpadu. ──
        if micd.cache_layer.is_some() && !fresh_results.is_empty() {
            use mivon_compiler::micd::cache::pipeline::{CachePopulateInput, CachePopulator};
            let input = CachePopulateInput {
                designs: vec![(&fallback, &design)],
                // Jalur legacy tidak punya IR di titik ini (elaborasi belum
                // jalan) — elaborate/ terisi fallback AST, generate/ dari AST.
                ir_design: None,
                expanded_design: None,
                opt_snapshot: None,
            };
            let mut layer = micd.cache_layer.take();
            if let Some(layer) = layer.as_mut() {
                CachePopulator::populate(layer, &input);
            }
            micd.cache_layer = layer;
        }

        // ── Prune stale files (Gap #10): buang file yang bukan bagian
        // project aktif agar tidak menumpuk lintas-project. ──
        let active: Vec<PathBuf> = sources
            .iter()
            .map(PathBuf::from)
            .chain(cli.libfiles.iter().map(PathBuf::from))
            .collect();
        let pruned = micd.prune_stale(&active);
        if pruned > 0 && std::env::var("MIVON_DBG_MICD").is_ok() {
            eprintln!("[MICD-DBG] prune_stale removed {} file(s)", pruned);
        }

        // ── Save terpadu (Gap #8): satu save untuk semua store
        // (preproc + metadata + symbol + type + graph + stats). ──
        let changed_before = micd.changed;
        if let Err(e) = micd.save() {
            eprintln!("[MICD] save warning: {}", e);
        }

        // ── Profil build (stats.mdb) — rekam sekali setelah save. ──
        let mut prof = micd.stats_db.next_profile();
        prof.files = micd.files.len();
        prof.changed_files = changed_before;
        prof.dirty_nodes = changed_before;
        prof.restored_designs = micd_reused;
        prof.cache_hits = micd_reused;
        prof.cache_misses = micd.files.len().saturating_sub(micd_reused);
        prof.peak_mem_kb = mivon_compiler::micd::peak_rss_kb();
        micd.set_stats(prof);
        if let Err(e) = micd.save() {
            eprintln!("[MICD] stats save warning: {}", e);
        }

        // ── Auto-snapshot (Gap #4): seperti commit git, simpan state
        // saat ada perubahan nyata agar rollback tersedia. ──
        if changed_before > 0 && changed_before > micd.last_snapshotted_changed {
            micd.last_snapshotted_changed = changed_before;
            if let Ok(id) = micd.snapshot(format!(
                "build: {} file(s) changed (legacy)",
                changed_before
            )) {
                if !cli.quiet && !anim_active(&anim) {
                    eprintln!("[MICD] snapshot build-{:03} created", id);
                }
            }
        }
    }

    if design.modules.is_empty() {
        // If there are packages, interfaces, or other items but no modules, it's not fatal
        if !design.packages.is_empty()
            || !design.interfaces.is_empty()
            || !design.classes.is_empty()
        {
            if !cli.quiet {
                eprintln!("note: no modules found in design (packages/interfaces present, skipping simulation)");
            }
            return Ok(());
        }
        return Err(SimError::with_diag(
            DiagCode::ModuleNotFound,
            "no modules found in design",
        ));
    }

    let top_name = cli.top.as_deref();
    if !cli.quiet && !anim_active(&anim) {
        println!("Compiling design ({} file sources)...", sources.len());
    }
    if anim_active(&anim) {
        // Subjek fase ELA: HANYA top eksplisit (`--top`). Tanpa fallback diam-diam
        // ke module pertama — itu bisa menampilkan module yang bukan top.
        // Top kosong → subject_line menampilkan jumlah module.
        anim_set_top(&anim, top_name.unwrap_or(""));
        anim_phase_running(&anim, Phase::Ela);
    }
    if !anim_active(&anim) {
        eprintln!("[TIMING] Starting elaboration...");
    }
    let elab_start = std::time::Instant::now();
    // ── Reuse IR cache (db.md "5. elaborate/"): seluruh source di-reuse dari
    // MICD preprocess cache (konten tidak berubah) → coba restore IR hasil
    // elaborasi sebelumnya, skip elaborator. Fase 6: sidik input (termasuk
    // libfiles yang direkam) wajib cocok — libfile berubah → elaborasi ulang.
    let mut restored_legacy_ir: Option<mivon_ir::IrDesign> = None;
    if !cli.recompile && micd_reused == sources.len() && !sources.is_empty() {
        let top = top_name.or_else(|| design.modules.first().map(|m| m.name.as_str()));
        if let Some(top) = top {
            let ih = micd.elaboration_inputs_hash(top);
            let ctx = format!("{:?}", pick_elab_mode(&cli));
            restored_legacy_ir = micd.restore_elaborate_ir(top, ih, &ctx);
        }
    }
    let mut _from_legacy_cache = false;
    let mut elab_diags: Vec<mivon_core::diagnostics::Diagnostic> = Vec::new();
    let mut recovered = false;
    let mut elaborator_opt: Option<Elaborator> = None;
    let mut ir_design = match restored_legacy_ir {
        Some(ir) => {
            _from_legacy_cache = true;
            if !cli.quiet && !anim_active(&anim) {
                eprintln!(
                    "[MICD] IR cache hit — elaborator di-skip (top '{}')",
                    ir.top.name
                );
            }
            ir
        }
        None => {
            let source_lines: Vec<String> = combined.lines().map(|s| s.to_string()).collect();
            let mut elaborator =
                Elaborator::with_source(design, source_lines, first_source.to_string());
            // Mode elaborasi: dengan `--top` eksplisit → StrictSimulation (top wajib).
            // Tanpa `--top` (run file satu-per-satu) → AnalysisRecovery: top yang tidak
            // unik (multiple candidate / circular / missing root) TIDAK menggagalkan
            // analisis — diagnostik dilaporkan, simulasi & VCD dinonaktifkan (Rule 4).
            let elab_mode = pick_elab_mode(&cli);
            let d = match elaborator.elaborate(top_name, elab_mode) {
                Ok(d) => d,
                Err(e) => {
                    let diags = elaborator.flush_diagnostics();
                    if anim_active(&anim) {
                        // Elaborasi gagal → ringkasan pakai jumlah error nyata,
                        // bukan konstanta 1. `max(1)` menjaga jaminan lama:
                        // jalur Err selalu ≥ 1 error walau diagnostiknya tidak
                        // membawa level Error.
                        let w = diags
                            .iter()
                            .filter(|d| d.level == DiagLevel::Warning)
                            .count();
                        let e = diags.iter().filter(|d| d.is_error()).count().max(1);
                        anim_abort(&mut anim, e, w);
                    }
                    emit_diags(&diags);
                    return Err(e);
                }
            };
            elab_diags = elaborator.flush_diagnostics();
            if anim_active(&anim) {
                anim_phase_done(&anim, Phase::Ela);
                anim_phase_done(&anim, Phase::Opt);
                anim_phase_done(&anim, Phase::Ver);
                let w = elab_diags
                    .iter()
                    .filter(|d| d.level == DiagLevel::Warning)
                    .count();
                let e = elab_diags.iter().filter(|d| d.is_error()).count();
                anim_finish(&mut anim, e == 0, e, w);
            }
            anim_log(
                &anim,
                &format!("[TIMING] Elaboration done in {:?}", elab_start.elapsed()),
            );
            // Flush elaboration-time diagnostics (warnings like WR0102)
            emit_diags(&elab_diags);
            // Mode analisis (recovery): top-level tidak dapat ditentukan secara
            // unik karena `--top` tidak diberikan dan design tidak punya top
            // yang jelas (multiple candidate / circular / root hilang).
            recovered = elaborator.recovered;
            elaborator_opt = Some(elaborator);
            d
        }
    };

    // ── Simulation Readiness Check (Rule 6) — full validation ──
    // Note: parse errors already handled earlier (early return for real errors)
    let elab_errs = elab_diags.iter().filter(|d| d.is_error()).count();
    let has_elab_errors = elab_errs > 0;

    // Per-tahap validation
    let sem_errs = elab_diags
        .iter()
        .filter(|d| {
            d.is_error()
                && matches!(
                    d.code,
                    DiagCode::UndefinedSignal
                        | DiagCode::TypeMismatch
                        | DiagCode::WidthMismatch
                        | DiagCode::UndefinedVariable
                )
        })
        .count();
    let hier_errs = elab_diags
        .iter()
        .filter(|d| {
            d.is_error()
                && matches!(
                    d.code,
                    DiagCode::ModuleNotFound
                        | DiagCode::CircularDependency
                        | DiagCode::CircularHierarchy
                        | DiagCode::UnresolvedInstantiation
                        | DiagCode::InstanceNotFound
                )
        })
        .count();
    let top_errs = elab_diags
        .iter()
        .filter(|d| {
            d.is_error()
                && matches!(
                    d.code,
                    DiagCode::TopResolutionFailed
                        | DiagCode::MultipleCandidateTops
                        | DiagCode::MissingRootModule
                )
        })
        .count();
    let dpi_errs = elab_diags
        .iter()
        .filter(|d| {
            d.is_error()
                && matches!(
                    d.code,
                    DiagCode::DpiImportNotFound | DiagCode::DpiError | DiagCode::DpiScopeError
                )
        })
        .count();

    // Cetak hasil pemeriksaan kesiapan
    if !cli.quiet {
        println!("\nKesiapan Simulasi");
        println!("✓ Parse");

        // Helper function (duplikat dari Block 2 — bisa di-refactor nanti)
        fn bl_diag_loc(d: &mivon_core::diagnostics::diagnostic::Diagnostic) -> Option<String> {
            if let Some(ss) = &d.source_snippet {
                return Some(format!("{}:{}:{}", ss.file, ss.line, ss.col));
            }
            if let Some(span) = d.spans.first() {
                return Some(format!("{}:{}:{}", span.file, span.start, span.end));
            }
            None
        }
        fn bl_print_cat(
            label: &str,
            count: usize,
            diags: &[&mivon_core::diagnostics::diagnostic::Diagnostic],
            max: usize,
        ) {
            if count == 0 {
                println!("✓ {}", label);
                return;
            }
            println!("✗ {} ({} error)", label, count);
            for (shown, d) in diags.iter().enumerate() {
                if shown >= max {
                    break;
                }
                let loc = bl_diag_loc(d).unwrap_or_else(|| "?".into());
                let msg = d.message.to_string();
                let short = if msg.len() > 80 {
                    truncate_ellipsis(&msg, 77)
                } else {
                    msg
                };
                println!("  {} | {}", loc, short);
            }
            if count > max {
                println!("  ... dan {} error lainnya", count - max);
            }
        }

        // Semantik
        let sem_diags: Vec<_> = elab_diags
            .iter()
            .filter(|d| {
                d.is_error()
                    && matches!(
                        d.code,
                        DiagCode::UndefinedSignal
                            | DiagCode::TypeMismatch
                            | DiagCode::WidthMismatch
                            | DiagCode::UndefinedVariable
                    )
            })
            .collect();
        bl_print_cat("Semantik", sem_errs, &sem_diags, 10);
        // Hierarki
        let hier_diags: Vec<_> = elab_diags
            .iter()
            .filter(|d| {
                d.is_error()
                    && matches!(
                        d.code,
                        DiagCode::ModuleNotFound
                            | DiagCode::CircularDependency
                            | DiagCode::CircularHierarchy
                            | DiagCode::UnresolvedInstantiation
                            | DiagCode::InstanceNotFound
                    )
            })
            .collect();
        bl_print_cat("Hierarki", hier_errs, &hier_diags, 10);
        // Resolusi Top
        if has_elab_errors || recovered {
            let top_diags: Vec<_> = elab_diags
                .iter()
                .filter(|d| {
                    d.is_error()
                        && matches!(
                            d.code,
                            DiagCode::TopResolutionFailed
                                | DiagCode::MultipleCandidateTops
                                | DiagCode::MissingRootModule
                        )
                })
                .collect();
            bl_print_cat("Resolusi Top", top_errs, &top_diags, 10);
        } else {
            println!("✓ Resolusi Top");
        }
        // Penghubung DPI
        let dpi_diags: Vec<_> = elab_diags
            .iter()
            .filter(|d| {
                d.is_error()
                    && matches!(
                        d.code,
                        DiagCode::DpiImportNotFound | DiagCode::DpiError | DiagCode::DpiScopeError
                    )
            })
            .collect();
        bl_print_cat("Penghubung DPI", dpi_errs, &dpi_diags, 10);
        println!();

        let any_errors = has_elab_errors;
        if any_errors {
            println!("Simulasi: TIDAK SIAP");
            println!("Simulasi dibatalkan.");
            return Err(SimError::Diagnostic(elab_abort_diag(
                elab_errs,
                &elab_diags,
                format!(
                    "simulasi dibatalkan: {} error ({} semantik + {} hierarki + {} top + {} dpi)",
                    elab_errs, sem_errs, hier_errs, top_errs, dpi_errs
                ),
            )));
        } else if recovered {
            println!("Simulasi: TIDAK SIAP (mode analisis)");
            println!("Top-level design tidak bisa ditentukan secara unik — beri `--top <module>` untuk simulasi (VCD & simulasi dinonaktifkan bila tanpa --top).");
        } else {
            println!("Simulasi: SIAP");
        }
    }

    // ── Gate: jangan simulasikan bila masih ada error elaborasi / module di-skip ──
    if elab_errs > 0 && !cli.force_sim {
        if !cli.quiet {
            if sem_errs > 0 {
                eprintln!("⚠  Semantik: {} error.", sem_errs);
            }
            if hier_errs > 0 {
                eprintln!("⚠  Hierarki: {} error.", hier_errs);
            }
            if top_errs > 0 || recovered {
                eprintln!(
                    "⚠  Resolusi Top: {} error, recovered={}.",
                    top_errs, recovered
                );
            }
            if dpi_errs > 0 {
                eprintln!("⚠  Penghubung DPI: {} error.", dpi_errs);
            }
            eprintln!(
                "    Perbaiki semua error terlebih dahulu — VCD TIDAK dihasilkan.\n    (Gunakan `--force-sim` hanya untuk debugging internal.)"
            );
        }
        return Err(SimError::Diagnostic(elab_abort_diag(
            elab_errs,
            &elab_diags,
            format!(
                "simulasi dibatalkan: {} error elaborasi — design belum 100% bersih",
                elab_errs
            ),
        )));
    }

    // ── Recovery/analisis mode (tanpa `--top`): jangan simulasikan modul tebakan ──
    // Top tidak unik → analisis selesai (diagnostik sudah dilaporkan di atas),
    // tapi simulasi & VCD dinonaktifkan agar waveform tidak menyesatkan.
    if recovered && !cli.force_sim {
        if !cli.quiet {
            println!("\nTop-level design tidak dapat ditentukan secara unik — mode analisis.");
            println!("Simulation & VCD dinonaktifkan.\n");
        }
        return Ok(());
    }

    // Basis tick elaborator = satuan TERHALUS (lihat catatan F84 di atas).
    // Menimpanya dengan directive terakhir di sini akan membuat display
    // `$time`/`%t` dan konversi SDF ps→tick memakai satuan yang BERBEDA dari
    // yang dipakai untuk menghitung delay — hasil simulasi jadi tak konsisten
    // dengan basisnya sendiri.
    let _ = &ts_for_ir;
    if ir_design.timescale.is_none() {
        ir_design.timescale = ts_for_ir;
    }

    // ── Lapisan cache pipeline: isi kategori elaborate/ + generate/ dari IR
    // hasil elaborasi (db.md "5. elaborate/", "16. generate/") — dipanggil
    // di sini (setelah elaborasi sukses) karena populate di atas belum punya
    // IR. Design diambil dari elaborator (design asli sudah dipindah saat
    // konstruksi). Best-effort; store disimpan via layer.save(). ──
    if micd.cache_layer.is_some() {
        use mivon_compiler::micd::cache::pipeline::{CachePopulateInput, CachePopulator};
        let fb = sources.first().map(PathBuf::from).unwrap_or_default();
        let (elab_designs, elab_design, opt_snapshot) = match elaborator_opt.as_ref() {
            Some(elab) => (
                vec![(&fb, &elab.design)],
                Some(&elab.design),
                Some(elab.opt_stats.snapshot()),
            ),
            // Jalur cache hit: IR di-restore, elaborator tidak hidup —
            // kategori elaborate/generate sudah terisi dari run sebelumnya.
            None => (Vec::new(), None, None),
        };
        let input = CachePopulateInput {
            designs: elab_designs,
            ir_design: Some(&ir_design),
            // designs sudah post-expansion (elaborator.design) — fallback
            // elaborate/ memakai designs itu sendiri.
            expanded_design: elab_design,
            // Statistik optimasi elaborator (db.md "6. optimize/",
            // "10. expression/") — di-snapshot setelah elaborasi sukses.
            opt_snapshot,
        };
        // Simpan IrDesign LENGKAP (bincode) di key `ir:<top>` — dipakai warm
        // run berikutnya untuk melewati elaborator sepenuhnya (db.md
        // "5. elaborate/"). Best-effort. Fase 6: simpan dengan sidik input
        // (termasuk libfiles yang direkam) agar restore menolak input beda.
        let legacy_ih = micd.elaboration_inputs_hash(ir_design.top.name.as_str());
        let legacy_ctx = format!("{:?}", pick_elab_mode(&cli));
        micd.store_elaborate_ir(&ir_design, legacy_ih, &legacy_ctx);
        let mut layer = micd.cache_layer.take();
        if let Some(layer) = layer.as_mut() {
            CachePopulator::populate_elab(layer, &input);
        }
        micd.cache_layer = layer;
        if let Err(e) = micd.save() {
            eprintln!("[MICD] elaborate/generate cache save warning: {}", e);
        }
    }

    if !cli.quiet {
        println!(
            "Module '{}': {} signals, {} processes",
            ir_design.top.name,
            ir_design.top.signals.len(),
            ir_design.top.processes.len()
        );
    }

    // ── Formal Verification (runs before simulation, skips sim) ──
    #[cfg(feature = "formal")]
    if cli.formal || !cli.formal_connect.is_empty() {
        return run_formal(
            &ir_design,
            cli.formal_bound,
            cli.quiet,
            cli.formal_induction,
            &cli.formal_connect,
        );
    }
    #[cfg(not(feature = "formal"))]
    if cli.formal {
        eprintln!("Formal verification not available: compile with --features formal");
        return Err(SimError::with_diag(
            DiagCode::NotImplemented,
            "formal feature not enabled",
        ));
    }

    // ── Compile-only mode: skip simulation & VCD ──
    if cli.compile_only {
        if !cli.quiet {
            println!("Compile-only mode: skipping simulation");
        }
        return Ok(());
    }

    // ── Setup ──
    let debug_mode = if cli.deep_debug {
        DebugMode::DeepDebug
    } else if cli.debug
        || cli.step
        || !cli.break_cycle.is_empty()
        || !cli.break_change.is_empty()
        || !cli.break_eq.is_empty()
        || !cli.watch.is_empty()
    {
        DebugMode::Debug
    } else {
        DebugMode::Normal
    };

    // Set X-propagation mode from CLI
    if let Some(mode) = mivon_api::simulator::types::XPropagationMode::from_name(&cli.xprop) {
        mivon_api::simulator::value::set_xprop_mode(mode);
        if !cli.quiet {
            println!("X-propagation mode: {}", mode.as_str());
        }
    } else {
        return Err(SimError::with_diag(
            DiagCode::InvalidSyntax,
            format!(
                "invalid --xprop '{}': use optimistic, pessimistic, or x-anywhere",
                cli.xprop
            ),
        ));
    }

    // ── Distributed simulation mode ──
    if cli.dist_master {
        let config = mivon_api::simulator::distributed::MasterConfig {
            port: cli.dist_port,
            num_partitions: cli.num_partitions,
            verbose: !cli.quiet,
            ..Default::default()
        };
        let mut master = mivon_api::simulator::distributed::DistributedMaster::new(config);
        master.run(&ir_design, cli.max_time.unwrap_or(DEFAULT_MAX_TIME_NS))?;
        if !cli.quiet {
            println!("Distributed simulation (master) complete");
        }
        return Ok(());
    }

    if cli.dist_slave {
        let config = mivon_api::simulator::distributed::SlaveConfig {
            master_host: cli.master_host.clone(),
            master_port: cli.dist_port,
            max_time: cli.max_time.unwrap_or(DEFAULT_MAX_TIME_NS),
            verbose: !cli.quiet,
        };
        let mut slave = mivon_api::simulator::distributed::DistributedSlave::new(config);
        slave.run(&ir_design)?;
        if !cli.quiet {
            println!("Distributed simulation (slave) complete");
        }
        return Ok(());
    }

    // IR stats (debug memori OOM) — hanya saat MIVON_MEM_STATS=1.
    print_ir_mem_stats(&ir_design);

    // Default: sim dibatasi `DEFAULT_MAX_TIME_NS` (anti-OOM untuk design
    // besar yang tidak pernah `$finish`). User bisa override dengan `-T`
    // / `--max-time <n>` (jadi Finite) — tanpa itu memakai default finite.
    let sim_limit = cli
        .max_time
        .map(mivon_api::simulator::SimulationLimit::Finite)
        .unwrap_or(mivon_api::simulator::SimulationLimit::Finite(
            DEFAULT_MAX_TIME_NS,
        ));
    let mut engine = SimulationEngine::new_with_limit(ir_design, sim_limit);
    engine.report_progress = !cli.quiet;
    // Lepas heap free (IR cache elaborasi / sisa AST) sebelum simulasi —
    // sim design besar butuh ruang (mesin kecil anti-OOM).
    trim_heap();

    // ── Set SDF timing mode ──
    if let Some(mode) = mivon_api::simulator::sdf::TimingMode::from_name(&cli.timing_mode) {
        mivon_api::simulator::sdf::set_timing_mode(mode);
        if !cli.quiet {
            println!("SDF timing mode: {}", mode.as_str());
        }
    } else {
        return Err(SimError::with_diag(
            DiagCode::InvalidSyntax,
            format!(
                "invalid --timing-mode '{}': use min, typ, or max",
                cli.timing_mode
            ),
        ));
    }

    // ── SDF Annotation (applies timing delays from Standard Delay Format file) ──
    if let Some(ref sdf_path) = cli.sdf {
        let sdf_data = mivon_api::simulator::sdf::SdfData::parse_file(sdf_path).map_err(|e| {
            SimError::with_diag(DiagCode::InvalidSyntax, format!("SDF parse failed: {}", e))
        })?;
        engine.annotate_sdf(&sdf_data)?;
        if !cli.quiet {
            println!("SDF annotation loaded from '{}'", sdf_path);
        }
    }

    engine.debug_mode = debug_mode;
    engine.snapshot_interval = cli.snap_interval;
    engine.use_packed_eval = cli.packed;
    engine.use_dag_parallel = cli.parallel;
    apply_engine_flags(&mut engine, &cli);
    engine.use_timing_wheel = cli.use_timing_wheel;
    // SIM-20: cycle-based mode (--cycle / --cycle-period)
    engine.set_cycle_based(cli.cycle_mode);
    engine.set_cycle_period(cli.cycle_period);
    engine.glitch_window = cli.glitch_window;
    if cli.glitch_window > 0 && !cli.quiet {
        println!(
            "Glitch detection enabled (window = {} time units)",
            cli.glitch_window
        );
    }
    if cli.use_timing_wheel && !cli.quiet {
        println!("Timing wheel enabled (O(1) event scheduling)");
    }
    if cli.jit_body && !cli.quiet {
        println!("Body-level MIR JIT enabled (compiled-code simulation path)");
    }

    // ── SIM-18: auto-checkpoint berkala (crash recovery) ──
    if let Some(ref cp_path) = cli.auto_checkpoint {
        let interval = cli.checkpoint_interval.max(1);
        engine.set_auto_checkpoint(cp_path, interval);
        if !cli.quiet {
            println!("Auto-checkpoint setiap {} cycle → '{}'", interval, cp_path);
        }
    }

    // ── Co-simulation (VHDL/SystemVerilog bridge) ──
    if let Some(cosim_port) = cli.cosim_port {
        // Build signal mapping from --cosim-signals or auto-detect
        let signal_mapping: Vec<(usize, String, bool)> =
            if let Some(ref sig_names) = cli.cosim_signals {
                sig_names
                    .split(',')
                    .filter_map(|name| {
                        let trimmed = name.trim();
                        let is_output = trimmed.starts_with('+');
                        let clean_name = trimmed.trim_start_matches('+');
                        engine
                            .design
                            .top
                            .signals
                            .iter()
                            .position(|s| s.name.as_str() == clean_name)
                            .map(|id| (id, clean_name.to_string(), is_output))
                    })
                    .collect()
            } else {
                // Auto-detect: all output ports are outputs, all input ports are inputs
                engine
                    .design
                    .top
                    .signals
                    .iter()
                    .enumerate()
                    .map(|(id, s)| {
                        let is_output = matches!(
                            s.kind,
                            mivon_ir::SignalKind::Output | mivon_ir::SignalKind::Inout
                        );
                        (id, s.name.to_string(), is_output)
                    })
                    .collect()
            };

        if signal_mapping.is_empty() && !cli.quiet {
            eprintln!(
                "warning: no signals mapped for co-simulation on port {}",
                cosim_port
            );
        }

        let n_sigs = signal_mapping.len();
        let cosim_state = mivon_api::simulator::cosim::start_cosim_server(cosim_port, n_sigs);
        engine.cosim_state = cosim_state;
        engine.cosim_signals = signal_mapping.clone();

        if !cli.quiet {
            println!(
                "Co-simulation bridge active on port {} ({} signals)",
                cosim_port,
                signal_mapping.len()
            );
        }
    }

    // Configure signal history disk spill
    if let Some(ref spill_path) = cli.signal_history_spill {
        engine
            .signal_history
            .enable_spill(std::path::PathBuf::from(spill_path));
        if !cli.quiet {
            println!("Signal history spill to '{}'", spill_path);
        }
    }

    // ── UPF Power Intent (power-aware simulation) ──
    if let Some(ref upf_path) = cli.upf {
        match mivon_api::simulator::upf::PowerIntent::parse_file(upf_path) {
            Ok(mut power_intent) => {
                power_intent.build_signal_mapping(&engine.design.top.signals);
                if !cli.quiet {
                    println!(
                        "UPF power intent loaded from '{}' ({} domains, {} supply nets)",
                        upf_path,
                        power_intent.domains.len(),
                        power_intent.supply_nets.len()
                    );
                }
                engine.power_intent = Some(power_intent);
            }
            Err(e) => {
                eprintln!("warning: UPF parse failed: {}", e);
            }
        }
    }

    // Load DPI shared libraries
    #[cfg(feature = "dpi")]
    if !cli.dpi_libs.is_empty() {
        use mivon_api::simulator::dpi::DpiEngine;
        #[allow(unused_imports)]
        use std::sync::Mutex;
        fn get_dpi_engine() -> &'static Mutex<Option<DpiEngine>> {
            use std::sync::OnceLock;
            static DPI: OnceLock<Mutex<Option<DpiEngine>>> = OnceLock::new();
            DPI.get_or_init(|| Mutex::new(Some(DpiEngine::new())))
        }
        if let Ok(mut guard) = get_dpi_engine().lock() {
            if let Some(ref mut eng) = *guard {
                for lib_path in &cli.dpi_libs {
                    match eng.load_library(lib_path) {
                        Ok(_) => {
                            if !cli.quiet {
                                println!("  DPI library loaded: {}", lib_path);
                            }
                        }
                        Err(e) => {
                            eprintln!("warning: failed to load DPI library '{}': {}", lib_path, e);
                        }
                    }
                }
            }
        }
    }

    // Load VHPI shared libraries (IEEE 1076-2008 ABI-compatible adapter).
    // Tanpa feature "dpi" (libloading) pemakaian --vhpi mengembalikan error
    // jelas dari loader, bukan panic.
    if !cli.vhpi_libs.is_empty() {
        for lib_path in &cli.vhpi_libs {
            match mivon_api::vhpi::loader::load_vhpi_library(lib_path) {
                Ok(vhpi) => {
                    if !cli.quiet {
                        println!(
                            "  VHPI library loaded: {} (abi {:?})",
                            vhpi.path.display(),
                            vhpi.abi
                        );
                    }
                    if let Err(e) = mivon_api::vhpi::loader::call_vhpi_startup(&vhpi) {
                        eprintln!("warning: vhpi_startup '{}': {}", lib_path, e);
                    }
                }
                Err(e) => {
                    eprintln!("warning: failed to load VHPI library '{}': {}", lib_path, e);
                }
            }
        }
    }

    // Load PLI shared libraries (IEEE 1364 ABI-compatible adapter).
    if !cli.pli_libs.is_empty() {
        for lib_path in &cli.pli_libs {
            match mivon_api::pli::loader::load_pli_library(lib_path) {
                Ok(pli) => {
                    if !cli.quiet {
                        println!(
                            "  PLI library loaded: {} (abi {:?})",
                            pli.path.display(),
                            pli.abi
                        );
                    }
                    if let Err(e) = mivon_api::pli::loader::call_pli_startup(&pli) {
                        eprintln!("warning: vpi_startup (PLI) '{}': {}", lib_path, e);
                    }
                }
                Err(e) => {
                    eprintln!("warning: failed to load PLI library '{}': {}", lib_path, e);
                }
            }
        }
    }

    // Apply plusargs
    for pa in &cli.plusargs {
        if let Some((key, val)) = pa.split_once('=') {
            engine.plusargs.insert(key.to_string(), val.to_string());
        } else {
            engine.plusargs.insert(pa.clone(), String::new());
        }
    }

    // Apply breakpoints
    for c in &cli.break_cycle {
        engine.breakpoints.push(Breakpoint::Cycle(*c));
        if !cli.quiet {
            println!("  breakpoint: cycle {}", c);
        }
    }
    for name in &cli.break_change {
        engine
            .breakpoints
            .push(Breakpoint::SignalChange(name.clone()));
        if !cli.quiet {
            println!("  breakpoint: change {}", name);
        }
    }
    for eq in &cli.break_eq {
        if let Some((name, val_hex)) = eq.split_once('=') {
            if let Ok(val) = u64::from_str_radix(
                val_hex.trim_start_matches("0x").trim_start_matches("0X"),
                16,
            ) {
                let w = engine
                    .design
                    .top
                    .signals
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| s.width)
                    .unwrap_or(32);
                engine.breakpoints.push(Breakpoint::SignalEq(
                    name.to_string(),
                    LogicVec::from_u64(val, w),
                ));
                if !cli.quiet {
                    println!("  breakpoint: {} == 0x{:X}", name, val);
                }
            }
        }
    }
    // Apply watchpoints
    for name in &cli.watch {
        engine.watchpoints.push(Watchpoint::Signal(name.clone()));
        if !cli.quiet {
            println!("  watchpoint: {}", name);
        }
    }

    // VCD setup
    let vcd_path = cli
        .output
        .clone()
        .unwrap_or_else(|| format!("{}.vcd", engine.design.top.name));
    let mut vcd = VcdWriter::new(&vcd_path, &engine.design).map_err(|e| {
        SimError::with_diag(
            DiagCode::WaveformError,
            format!("VCD creation failed: {}", e),
        )
    })?;
    // --no-vcd: nonaktifkan dump (write_raw & dump_all di-guard `enabled`).
    if cli.no_vcd {
        vcd.enabled = false;
    }
    if cli.waveform_stream {
        vcd.stream_flush_interval = 1;
        if !cli.quiet {
            println!("Waveform streaming enabled (flush every time step)");
        }
    }
    if cli.waveform_bg {
        vcd.enable_background().map_err(|e| {
            SimError::with_diag(DiagCode::WaveformError, format!("VCD background: {}", e))
        })?;
        if !cli.quiet {
            println!("Waveform background writer enabled (non-blocking dump)");
        }
    }
    // WAV-04: Enable gzip compression for VCD output
    if cli.waveform_gzip {
        vcd.enable_compression().map_err(|e| {
            SimError::with_diag(DiagCode::WaveformError, format!("VCD gzip: {}", e))
        })?;
        if !cli.quiet {
            println!("Waveform gzip compression enabled");
        }
    }
    engine.set_vcd(vcd);

    // CSV waveform setup
    if let Some(ref csv_path) = cli.waveform_csv {
        let csv =
            mivon_api::waveform::CsvWaveWriter::new(csv_path, &engine.design).map_err(|e| {
                SimError::with_diag(
                    DiagCode::WaveformError,
                    format!("CSV creation failed: {}", e),
                )
            })?;
        engine.set_csv(csv);
        if !cli.quiet {
            println!("CSV waveform: {}", csv_path);
        }
    }

    // Signal statistics setup
    if cli.signal_stats.is_some() {
        let stats = mivon_api::waveform::SignalStats::new(&engine.design);
        engine.set_signal_stats(stats);
    }

    // ── Simulation ──
    let mut debugger = Debugger::new(engine);

    // SIM-17: restore checkpoint sebelum sim — `--restore <path>` melanjutkan
    // sim dari state tersimpan (`--save` / auto-checkpoint). Sebelumnya flag
    // --restore didefinisikan di cli.rs tapi tidak pernah di-wire.
    if let Some(restore_path) = &cli.restore {
        let path = std::path::Path::new(restore_path);
        debugger.engine.load_checkpoint(path).map_err(|e| {
            SimError::with_diag(
                DiagCode::IoError,
                format!("checkpoint restore failed: {}", e),
            )
        })?;
        if !cli.quiet {
            println!("Checkpoint restored from '{}'", restore_path);
        }
    }

    if cli.print_tree {
        println!("\n{}", debugger.hierarchy_tree());
    }

    if cli.step && debug_mode != DebugMode::Normal {
        if !cli.quiet {
            println!("\nStep mode: running one cycle...");
        }
        debugger.step_cycle()?;
        if !cli.quiet {
            println!("{}\n", debugger.print_state_summary());
        }
        if !debugger.engine.event_log.is_empty() && !cli.quiet {
            println!("{}", debugger.print_event_log());
        }
    } else {
        if !cli.quiet {
            println!(
                "\nStarting simulation (max time={}, vcd={})",
                sim_limit.display(),
                vcd_path
            );
        }
        debugger.run()?;
    }

    // Flush runtime diagnostics (warnings) SEGERA setelah sim selesai —
    // SEBELUM semua output hasil (coverage/completed/VCD) sehingga baris
    // akhir log murni hasil sim, tanpa warning menyusul.
    emit_diags(&debugger.engine.flush_diagnostics());

    if !cli.quiet {
        println!(
            "\nSimulation completed at time {}",
            debugger.engine.state.time
        );
    }

    // ── Simulation performance dashboard (SIM-25) ──
    if cli.perf_dashboard && !cli.quiet {
        println!("{}", debugger.engine.sim_perf);
    }

    if debug_mode != DebugMode::Normal && !cli.quiet {
        if debugger.engine.paused {
            println!("(debugger paused)");
        }
        if !debugger.engine.event_log.is_empty() {
            println!("\nDebug events:");
            println!("{}", debugger.print_event_log());
        }
    }

    sim_finish::finish_simulation(&cli, &mut debugger, &vcd_path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi wiring pemisahan run(): fn `run` harus tetap jalan dengan
    /// tanda tangan sama dari root (`run()` lama di main.rs) — piecing
    /// antarmuka dijaga agar call-site root tidak berubah perilaku.
    #[test]
    fn run_signature_unchanged() {
        let _f: fn(Cli, &mut mivon_api::env::GlobalEnv) -> Result<(), SimError> =
            self::run;
        // Konstanta default masih pub (dipakai sig sim_limit di run()).
        assert!(crate::DEFAULT_MAX_TIME_NS > 0);
    }
}
