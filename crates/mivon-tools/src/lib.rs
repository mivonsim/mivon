//! CLI tools untuk mivon — backend tunggal untuk tools terminal
//! (minspect, mlint, melab, msim, mcov, mwave, mfmt, mprof, mcheck, mbench).
//!
//! Semua tools memakai infrastruktur Mivon yang sama (lexer/parser/elaborator/
//! simulator) agar tidak ada duplikasi logika antara CLI dan GUI.

// Allow large Result Err variant for SimError (intentional — Diagnostic contains spans/files)
#![allow(clippy::result_large_err, clippy::type_complexity)]

pub mod batch;
pub mod bench;
pub mod bpf_monitor;
pub mod check;
pub mod ci;
pub mod collab;
pub mod compile_daemon;
pub mod compile_partition;
pub mod cov;
pub mod cov_closure;
pub mod cov_server;
pub mod crypto;
pub mod design_repo;
pub mod dft_interface;
pub mod distributed_compile;
pub mod eco;
pub mod elab;
pub mod epoll_async;
pub mod equiv_check;
pub mod fmt;
pub mod gen;
pub mod hugepage;
pub mod ieee_coverage;
pub mod incremental_compile;
pub mod inspect;
pub mod instance_array;
pub mod io_uring;
pub mod ipxact;
pub mod kvm_accel;
pub mod license;
pub mod license_server;
pub mod lint;
pub mod memcheck;
pub mod migrate;
pub mod numa;
pub mod perf;
pub mod platform;
pub mod plugin_market;
pub mod pr_interface;
pub mod prof;
pub mod project;
pub mod python_bindings;
pub mod regression;
pub mod remote_cache;
pub mod requirements;
pub mod rest_api;
pub mod sdc;
pub mod sim;
pub mod sim_dashboard;
pub mod sim_farm;
pub mod sim_server;
pub mod sta_interface;
pub mod synth;
pub mod synth_interface;
pub mod tbgen;
pub mod tcl;
pub mod testdb;
pub mod update;
pub mod vault;
pub mod verif_dashboard;
pub mod vip_lib;
pub mod waiver;
pub mod wave;
pub mod wave_server;

/// Modul presentasi tunggal untuk output tool (section/kv/footer/human_bytes/
/// expr_to_string). Semua helper cetak dikonsolidasi di sini agar tidak duplikat
/// di tiap tool.
pub mod render;
pub use render::{bullet, expr_to_string, footer, human_bytes, kv, section};

/// Bridge konversi tipe diagnostik paralel (mv, micd, ...) → `Diagnostic` core
/// agar seluruh jalur error dirender lewat renderer tunggal `format_diagnostic`.
pub mod bridge;
pub mod bugtrack;
pub use bridge::{mv_error_to_diagnostic, mv_warning_to_diagnostic};

use std::path::{Path, PathBuf};

use mivon_ast::types::Design;
use mivon_compiler::frontend::compile_session::{CompileSession, SessionConfig};
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::ElaborateMode;

fn diag_io(msg: impl Into<String>) -> SimError {
    SimError::with_diag(DiagCode::IoError, msg)
}

/// Expand input target menjadi daftar file:
/// - direktori  → scan recursive (`.sv`/`.svh`/`.v`/`.vh`)
/// - file `.f` → daftar file (relatif ke direktori file list)
/// - lainnya → file tunggal
///
/// File di-unique-kan dan diurutkan agar output deterministik.
pub fn collect_targets(paths: &[String]) -> Result<Vec<PathBuf>, SimError> {
    use mivon_compiler::frontend::discovery::DiscoveryOptions;
    use mivon_compiler::frontend::FileDiscovery;
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if !path.exists() {
            return Err(diag_io(format!("path tidak ditemukan: '{}'", p)));
        }
        if path.is_dir() {
            // F10: sertakan `.mv` (Mivon HDL) + F43 `.mvh` (header) di scan
            // direktori — tool yang memakai open_project/open_elaborated
            // otomatis men-transpile-nya.
            let mut opts = DiscoveryOptions::default();
            opts.extensions.push("mv".into());
            opts.extensions.push("mvh".into());
            let res = FileDiscovery::scan_dir(path, &opts);
            out.extend(res.files.into_iter().map(|f| f.path));
        } else {
            let is_list = path
                .extension()
                .map(|e| e == "f" || e == "mivon")
                .unwrap_or(false);
            if is_list {
                let files = FileDiscovery::scan_file_list(path)
                    .map_err(|e| diag_io(format!("{}: {}", p, e)))?;
                out.extend(files);
            } else {
                out.push(path.to_path_buf());
            }
        }
    }
    out.sort();
    out.dedup();
    if out.is_empty() {
        return Err(diag_io(
            "tidak ada file ditemukan — berikan direktori, file list, atau file .sv",
        ));
    }
    Ok(out)
}

/// Bangun `SessionConfig` dari daftar file + CLI options.
fn make_session_config(
    files: Vec<PathBuf>,
    incdirs: &[String],
    defines: &[String],
    top: Option<String>,
) -> SessionConfig {
    SessionConfig {
        sources: files,
        incdirs: incdirs.iter().map(PathBuf::from).collect(),
        defines: defines
            .iter()
            .filter_map(|d| d.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        top_module: top,
        ..Default::default()
    }
}

/// F10: bangun `SessionConfig` dari daftar file + CLI options, sekaligus
/// transpile semua `.mv` ke buffer inline. Satu-satunya cara membuat config
/// dengan dukungan `.mv` — dipakai open_project / open_elaborated / prof /
/// bench (DRY, agar tidak ada call site yang lupa memasang inline_sources).
fn make_session_config_with_mv(
    files: Vec<PathBuf>,
    incdirs: &[String],
    defines: &[String],
    top: Option<String>,
) -> Result<SessionConfig, SimError> {
    let inline = transpile_mv_to_inline(&files)?;
    let mut cfg = make_session_config(files, incdirs, defines, top);
    cfg.inline_sources = inline;
    Ok(cfg)
}

/// F10: transpile semua file `.mv`/`.mvh` dalam daftar ke buffer SV inline
/// (svh+sv digabung, baris `` `include `` di-strip — definisi bersama sudah
/// ada di atasnya). File non-Mivon-HDL tidak disentuh. Hasil: peta path →
/// buffer. Sumber `.mvh` (F43) ikut divalidasi header-only (E2008) —
/// buffer-nya hanya berisi bagian `.svh`.
///
/// Memakai `transpile_many_items` (konteks gabungan F9) sehingga package
/// lintas-file (`types.mvh` → `counter.mv`) bekerja di tool manapun.
pub fn transpile_mv_to_inline(
    files: &[PathBuf],
) -> Result<std::collections::HashMap<PathBuf, Vec<u8>>, SimError> {
    let mv_idx: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, p)| gen::is_mv_source(p))
        .map(|(i, _)| i)
        .collect();
    if mv_idx.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let mut items: Vec<mivon_mv::MvItem> = Vec::with_capacity(mv_idx.len());
    for &i in &mv_idx {
        let p = &files[i];
        let base = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("design")
            .to_string();
        let src =
            std::fs::read_to_string(p).map_err(|e| diag_io(format!("{}: {}", p.display(), e)))?;
        let header = p.extension().map(|e| e == "mvh").unwrap_or(false);
        // Pipeline inline (run/sim/...) tidak punya opsi `mgen --package`;
        // emisi default ($unit) dipakai agar `mivon run x.mv` tetap identik
        // dengan output `mgen x.mv` tanpa flag.
        items.push(mivon_mv::MvItem::new(src, base).with_header(header));
    }
    let results = mivon_mv::transpile_many_items(&items)
        .map_err(|(i, e)| diag_io(format!("{}: {}", files[mv_idx[i]].display(), e)))?;

    // ── §9A.2 warning hazard bawaan (stderr, non-blokir) ──
    {
        let mut n_warn = 0usize;
        for ((&i, item), tr) in mv_idx.iter().zip(items.iter()).zip(results.iter()) {
            for w in &tr.warnings {
                eprintln!(
                    "{}",
                    mivon_mv::format_warning(&files[i].display().to_string(), &item.src, w)
                );
                n_warn += 1;
            }
        }
        if n_warn > 0 {
            eprintln!("── hazard: {n_warn} warning ──");
        }
    }

    let mut inline = std::collections::HashMap::new();
    for (&i, tr) in mv_idx.iter().zip(results.iter()) {
        let mut buf = tr.svh.clone();
        buf.push('\n');
        for line in tr.sv.lines() {
            let t = line.trim_start();
            if t.starts_with("`include") {
                continue;
            }
            buf.push_str(line);
            buf.push('\n');
        }
        inline.insert(files[i].clone(), buf.into_bytes());
    }
    Ok(inline)
}

/// Buka project: expand target → CompileSession → parse semua file secara
/// paralel (dengan cache MICD bila tersedia) → merged `Design` + session.
///
/// Session dipertahankan agar tool bisa mengakses `module_index` (peta
/// module → file) untuk output yang butuh lokasi source.
pub fn open_project(
    targets: &[String],
    incdirs: &[String],
    defines: &[String],
    top: Option<&str>,
) -> Result<(Design, CompileSession), SimError> {
    let files = collect_targets(targets)?;
    let cfg = make_session_config_with_mv(files, incdirs, defines, top.map(|s| s.to_string()))?;
    let mut session = CompileSession::new(cfg);
    let (design, _index) = session.compile()?;
    Ok((design, session))
}

/// Buka project + elaborasi penuh → (session, design, ir_design).
/// Dipakai melab/msim/mcov/mprof.
pub fn open_elaborated(
    targets: &[String],
    incdirs: &[String],
    defines: &[String],
    top: Option<&str>,
    mode: ElaborateMode,
) -> Result<(CompileSession, Design, mivon_ir::IrDesign), SimError> {
    let files = collect_targets(targets)?;
    let cfg = make_session_config_with_mv(files, incdirs, defines, top.map(|s| s.to_string()))?;
    let mut session = CompileSession::new(cfg);
    match session.compile_and_elaborate_with_mode(top, mode) {
        Ok((design, ir, _len)) => Ok((session, design, ir)),
        Err(e) => {
            // Elaborasi gagal: tampilkan diagnostik akar (instance tak
            // ter-resolve, param gagal, dsb.) — bukan hanya E3005 generik.
            // Best-effort: kegagalan render tak menutupi error asli.
            let diags = session.take_elab_diagnostics();
            if !diags.is_empty() {
                let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
                for d in &diags {
                    let _ = emitter.emit(d);
                }
            }
            Err(e)
        }
    }
}

/// Buka lapisan cache pipeline (`cache/<pid>/`) untuk project yang sama dengan
/// targets/incdirs/defines — tanpa compile. Dipakai tool read-back
/// (mprof --cached, melab --from-cache, minspect cache): membaca hasil build
/// sebelumnya (db.md cache/ "tidak perlu dielaborasi ulang").
///
/// Mengembalikan (CacheLayer, project id). Gagal bila targets tidak ada atau
/// database root tidak bisa dibuka.
pub fn open_cache_layer(
    targets: &[String],
    incdirs: &[String],
    defines: &[String],
) -> Result<(mivon_compiler::micd::CacheLayer, String), SimError> {
    use mivon_compiler::micd::MicdDatabase;
    let files = collect_targets(targets)?;
    let root = MicdDatabase::default_root();
    let proot = std::env::current_dir().unwrap_or_default();
    let inc: Vec<PathBuf> = incdirs.iter().map(PathBuf::from).collect();
    let defs: Vec<(String, String)> = defines
        .iter()
        .filter_map(|d| d.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let pid = MicdDatabase::project_id(&proot, &files, &inc, &defs);
    let db = MicdDatabase::open_project_with_context(&root, &pid, &proot, &files);
    let layer = db.cache_layer.ok_or_else(|| {
        diag_io("lapisan cache pipeline tidak tersedia (database root tak bisa ditulis)")
    })?;
    Ok((layer, pid))
}
