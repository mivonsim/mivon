// Allow large Result Err variant for SimError (intentional)
#![allow(clippy::result_large_err, clippy::type_complexity)]

mod anim_glue;
mod cli;
#[cfg(feature = "formal")]
mod formal_run;
mod pipeline_config;
mod report_glue;
mod run_legacy;
mod sim_finish;
mod source_io;
mod dispatch;

use anim_glue::{
    anim_abort, anim_active, anim_enabled, anim_finish, anim_phase_done,
    anim_phase_running, anim_set_exts, anim_set_files, anim_set_modules,
    anim_set_top, emit_diags_anim,
};
use clap::Parser as ClapParser;
use cli::Cli;
use run_legacy::run;
use pipeline_config::{micd_root_for, pick_elab_mode};
use source_io::load_project_foreign_libs;
#[cfg(feature = "formal")]
use formal_run::run_formal;
use dispatch::{
    dispatch_batch, dispatch_bench, dispatch_check, dispatch_cov, dispatch_cov_closure,
    dispatch_design_repo, dispatch_eco, dispatch_elab, dispatch_emu, dispatch_equiv_check,
    dispatch_fmt,
    dispatch_gen, dispatch_inspect, dispatch_ipxact, dispatch_lint, dispatch_memcheck,
    dispatch_prof, dispatch_project, dispatch_regression, dispatch_sdc, dispatch_sim,
    dispatch_synth, dispatch_tbgen, dispatch_update, dispatch_vault, dispatch_waiver,
    dispatch_wave,
};
use std::path::PathBuf;
use std::process;

use mivon_api::debugger::Debugger;
use mivon_api::read_project_file;
use mivon_api::simulator::Breakpoint;
use mivon_api::simulator::DebugMode;
use mivon_api::simulator::SimulationEngine;
use mivon_api::simulator::Watchpoint;
use mivon_api::waveform::VcdWriter;
use mivon_api::SessionConfig;
use mivon_compiler::frontend::CompileSession;
use mivon_core::animasi::{Phase, PipelineAnimator};
use mivon_core::diagnostics::DiagCode;
use mivon_core::diagnostics::DiagLevel;
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::Elaborator;
use mivon_ir::LogicVec;

/// Default simulasi maksimum (ns) saat tidak ada `-T` / `simulation.max_time`.
///
/// VCS/Questa mengharuskan pengguna menetapkan batas via `-T`; tanpa itu
/// simulasi berjalan sampai `$finish` (unlimited) → untuk design besar
/// (SoC/GPU) yang tidak pernah `$finish`, RSS naik terus bersama akumulasi
/// waveform → OOM kill. Shell default finite yang cukup kuat untuk SoC/GPU
/// (100µs @ 1GHz = 100k cycle) namun tetap membatasi memori. User bisa
/// override kapan saja dengan `-T` / `--max-time <ns>`.
pub const DEFAULT_MAX_TIME_NS: u64 = 100_000;

/// Emit a list of diagnostics through TerminalEmitter.
// Global warning filter state (set once at startup, used by emit_diags_filtered)
static WARN_FILTER_NO_WARN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static WARN_FILTER_ALLOW: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
static WARN_FILTER_MAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// Potong `s` ke maksimum `max` BYTE pada batas karakter UTF-8, lalu tambah
/// `…`. Aman terhadap karakter multi-byte: byte index yang jatuh di tengah
/// karakter (mis. `—` pada pesan diagnostik) dimundurkan ke batas sebelumnya
/// — menghindari panic `not a char boundary` (temuan fuzz verify_bug_0228).
fn truncate_ellipsis(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Initialize global warning filter from CLI args (call once at startup)
fn init_warn_filter(cli: &Cli) {
    WARN_FILTER_NO_WARN.store(cli.no_warn, std::sync::atomic::Ordering::Relaxed);
    let _ = WARN_FILTER_ALLOW.set(cli.allow_codes.clone());
    let _ = WARN_FILTER_MAX.set(cli.max_warnings.unwrap_or(usize::MAX));
}

pub(crate) fn emit_diags(diags: &[mivon_core::diagnostics::diagnostic::Diagnostic]) {
    let no_warn = WARN_FILTER_NO_WARN.load(std::sync::atomic::Ordering::Relaxed);
    let allow_codes = WARN_FILTER_ALLOW.get().cloned().unwrap_or_default();
    let max_warn = WARN_FILTER_MAX.get().copied();
    if diags.is_empty() {
        return;
    }
    use std::collections::HashSet;
    let mut seen: HashSet<(String, String, usize)> = HashSet::new(); // (code, file, line)
    let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
    static WARN_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    for diag in diags {
        if no_warn && diag.level == mivon_core::diagnostics::DiagLevel::Warning {
            continue;
        }
        if !allow_codes.is_empty() && diag.level == mivon_core::diagnostics::DiagLevel::Warning {
            let code_str = format!("{}", diag.code);
            if allow_codes.iter().any(|c| c == &code_str) {
                continue;
            }
        }
        // Deduplicate warnings by (code, file, line)
        if diag.level == mivon_core::diagnostics::DiagLevel::Warning {
            let file = diag
                .source_snippet
                .as_ref()
                .map(|s| s.file.clone())
                .unwrap_or_default();
            let line = diag.source_snippet.as_ref().map(|s| s.line).unwrap_or(0);
            let key = (format!("{}", diag.code), file, line);
            if !seen.insert(key) {
                continue;
            }
        }
        if let Some(max) = max_warn {
            if diag.level == mivon_core::diagnostics::DiagLevel::Warning {
                let count = WARN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if count > max {
                    if count == max + 1 {
                        eprintln!("\n... more warnings suppressed (use --max-warnings to adjust)");
                    }
                    continue;
                }
            }
        }
        let _ = emitter.emit(diag);
    }
}

/// Bangun diagnostic E3001 "design not ready" dengan lokasi error elaborasi
/// pertama (file:line:col) agar user langsung tahu di mana memperbaiki.
/// Prioritas: source_snippet (punya line:col) → span (byte offset) → tanpa lokasi.
fn elab_abort_diag(
    _elab_errs: usize,
    diags: &[mivon_core::diagnostics::diagnostic::Diagnostic],
    message: impl Into<String>,
) -> mivon_core::diagnostics::diagnostic::Diagnostic {
    use mivon_core::diagnostics::diagnostic::{DiagCode, DiagLevel, Diagnostic};
    let first_err = diags.iter().find(|d| d.is_error());
    let loc = first_err.and_then(|d| {
        d.source_snippet
            .as_ref()
            .map(|s| format!("{}:{}:{}", s.file, s.line, s.col))
    });
    let msg = match &loc {
        Some(l) => format!("{} (first error at {})", message.into(), l),
        None => message.into(),
    };
    let mut diag = Diagnostic::new(
        DiagLevel::Error,
        // Kode error SEBENARNYA dari error pertama (bukan selalu
        // ModuleNotFound — mis. width/type mismatch dilabeli "module not
        // found" → user mencari masalah di tempat salah).
        first_err
            .map(|d| d.code)
            .unwrap_or(DiagCode::ModuleNotFound),
        msg,
    )
    .with_code_context();
    if let Some(e) = first_err {
        if let Some(snippet) = &e.source_snippet {
            diag = diag.with_source_snippet(snippet.clone());
        } else if let Some(span) = e.spans.first() {
            diag = diag.with_span(span.clone());
        }
        if let Some(loc_str) = loc {
            diag = diag.with_note(format!("first elaboration error at {}", loc_str));
        }
    }
    diag
}

/// Terapkan config TOML (`configs/*.toml`) ke CLI — HANYA bila CLI belum
/// menyetelnya (CLI menang). Field yang tidak punya padanan CLI (opt_level,
/// lto, max_parse_steps, lint check, dsb.) dibiarkan sebagai dokumentasi.
/// jobs diterapkan langsung ke rayon pool di main() (bukan lewat cli).
fn apply_config_to_cli(cli: &mut Cli, cfg: &mivon_core::config::MivonConfig) {
    // compiler.cache=false / incremental=false → lewati MICD (setara --recompile).
    if cfg.compiler.cache == Some(false) || cfg.compiler.incremental == Some(false) {
        cli.recompile = true;
    }
    // elaborate.mode → pilihan ElaborateMode (StrictSimulation / AnalysisRecovery).
    if let Some(m) = &cfg.elaborate.mode {
        cli.config_elab_mode = Some(m.clone());
    }
    // simulation.max_time → -T bila CLI tidak set.
    if cli.max_time.is_none() {
        cli.max_time = cfg.simulation.max_time;
    }
    // simulation.force_sim → --force-sim bila CLI tidak set.
    if !cli.force_sim {
        cli.force_sim = cfg.simulation.force_sim.unwrap_or(false);
    }
    // waveform.stream → --waveform-stream.
    if !cli.waveform_stream {
        cli.waveform_stream = cfg.waveform.stream.unwrap_or(false);
    }
    // coverage → --coverage-threshold / --coverage-html / --coverage-ucis.
    if cli.coverage_threshold.is_none() {
        cli.coverage_threshold = cfg.coverage.branch_threshold;
    }
    if cli.coverage_html.is_none() && cfg.coverage.html == Some(true) {
        // Prefix kosong → string kosong → run() mensubstitusi nama top
        // (konsisten dengan --coverage-ucis yang memakai top.name bila kosong).
        let prefix = cfg.coverage.output_prefix.clone().unwrap_or_default();
        cli.coverage_html = Some(if prefix.is_empty() {
            String::new()
        } else {
            format!("{}.coverage.html", prefix)
        });
    }
    if cli.coverage_ucis.is_none() && cfg.coverage.ucis == Some(true) {
        let prefix = cfg.coverage.output_prefix.clone().unwrap_or_default();
        cli.coverage_ucis = Some(if prefix.is_empty() {
            String::new()
        } else {
            format!("{}.ucis.xml", prefix)
        });
    }
    // debug → --deep-debug / --snap-interval / breakpoint & watchpoint.
    if !cli.deep_debug && cfg.debug.deep == Some(true) {
        cli.deep_debug = true;
    }
    if cli.snap_interval == 1000 {
        if let Some(v) = cfg.debug.snapshot_interval {
            cli.snap_interval = v;
        }
    }
    if cli.break_cycle.is_empty() {
        if let Some(v) = cfg.debug.break_cycle {
            cli.break_cycle.push(v);
        }
    }
    if cli.watch.is_empty() {
        if let Some(v) = &cfg.debug.watch {
            cli.watch.extend(v.iter().cloned());
        }
    }
    if !cli.print_tree && cfg.debug.print_tree == Some(true) {
        cli.print_tree = true;
    }
    if cli.timeline.is_empty() {
        if let Some(v) = &cfg.debug.timeline {
            cli.timeline.extend(v.iter().cloned());
        }
    }
    if cli.timeline_len == 20 {
        if let Some(v) = cfg.debug.timeline_len {
            cli.timeline_len = v;
        }
    }
}

/// Bersihkan database MICD (`.mivon/database`). Menghapus isi
/// `<root>/.mivon/database` (atau `MIVON_MICD_DIR` bila di-set).
fn run_clean() -> ! {
    use mivon_compiler::micd::MicdDatabase;
    let root = MicdDatabase::default_root();
    if root.exists() {
        match std::fs::remove_dir_all(&root) {
            Ok(()) => {
                eprintln!("MICD database dihapus: {}", root.display());
                process::exit(0);
            }
            Err(e) => {
                eprintln!("Gagal menghapus MICD database '{}': {}", root.display(), e);
                process::exit(1);
            }
        }
    } else {
        eprintln!("Tidak ada MICD database di '{}'", root.display());
        process::exit(0);
    }
}

/// Simpan MICD dan cetak statistik ringkas (best-effort, tidak menggagalkan run).
/// `suppress` menekan output (saat animasi pipeline aktif di terminal).
fn micd_save_and_print(session: &mut CompileSession, quiet: bool, suppress: bool) {
    let t0 = std::time::Instant::now();
    match session.save_micd() {
        Ok(Some(st)) => {
            if !quiet && !suppress {
                // Tambah info precompiled bila ada.
                let precompiled_info = session
                    .micd
                    .as_ref()
                    .and_then(|db| db.precompiled_db.as_ref())
                    .map(|pdb| format!(" precompiled={}", pdb.len()))
                    .unwrap_or_default();
                eprintln!(
                    "[MICD] files={} restored={} changed={} snapshots={} save_ms={}{}",
                    st.files,
                    st.restored_designs,
                    st.changed_files,
                    st.snapshot_id,
                    t0.elapsed().as_millis(),
                    precompiled_info
                );
            }
        }
        Ok(None) => {}
        Err(e) => {
            if !quiet && !suppress {
                eprintln!("[MICD] save warning: {}", e);
            }
        }
    }
}

/// Jalankan seluruh CLI di thread dengan stack besar.
/// Parser/elaborator rekursif pada design besar (OpenTitan: 1600 modul,
/// hierarki dalam) membutuhkan jauh lebih dari stack default 8MB main thread
/// (stack overflow "thread 'main' has overflowed its stack" saat elaborasi).
/// Fix rayon stack_size hanya menyentuh worker threads, bukan main thread.
fn main() {
    let stack_size = std::env::var("MIVON_STACK_SIZE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(256 * 1024 * 1024); // 256MB default, override via env
    match std::thread::Builder::new()
        .stack_size(stack_size)
        .name("mivon-main".into())
        .spawn(real_main)
    {
        Ok(handle) => {
            if let Err(panic) = handle.join() {
                eprintln!("fatal: mivon-main thread panicked: {:?}", panic);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("fatal: cannot spawn mivon-main worker thread: {}", e);
            std::process::exit(1);
        }
    }
}

/// Lepas free heap pages (glibc) kembali ke OS setelah beban besar dibebaskan
/// (AST parse 2GB+/IR cache elaborasi). Tanpa ini RSS tetap tinggi karena
/// allocator menahan page free — mesin kecil (RAM 6GB) OOM saat elaborasi/
/// simulasi desain besar. GNU-libc specific; no-op aman bila trim tidak
/// mengembalikan apa pun.
fn trim_heap() {
    #[cfg(all(target_os = "linux", not(target_env = "musl")))]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Tune glibc allocator: alokasi ≥ 64KB langsung via mmap (bukan heap arena)
/// sehingga saat di-free pages KEMBALI ke OS — mengurangi high-water RSS &
/// swap pada sim design besar (OpenTitan arena +1.6GB dr kurva monitoring).
/// M_MMAP_THRESHOLD default 128KB terlalu besar utk transient array besar.
pub fn tune_heap_allocator() {
    #[cfg(all(target_os = "linux", not(target_env = "musl")))]
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 64 * 1024);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 1024 * 1024);
    }
}

/// Statistik memori IR hasil elaborasi — dipakai diagnosis OOM (MIVON_MEM_STATS=1):
/// jumlah module/signal/bit flatten + hier_signal_map + RSS saat ini. Ringan
/// (iterasi signal refs), aman dipanggil sebelum simulasi.
fn print_ir_mem_stats(ir: &mivon_ir::IrDesign) {
    if std::env::var("MIVON_MEM_STATS").is_err() {
        return;
    }
    let mut n_sig = 0usize;
    let mut n_bits: u64 = 0;
    let mut top: Vec<(String, String, usize, usize, Vec<usize>)> = Vec::new();
    for (modname, m) in ir.modules.iter() {
        for s in &m.signals {
            n_sig += 1;
            n_bits += s.width as u64;
            if top.len() < 5 || top.iter().any(|(_, _, w, _, _)| s.width > *w) {
                top.push((
                    modname.as_str().to_string(),
                    s.name.as_str().to_string(),
                    s.width,
                    s.array_depth,
                    s.array_dims.clone(),
                ));
                top.sort_by_key(|t| std::cmp::Reverse(t.2));
                top.truncate(5);
            }
        }
    }
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|k| k.parse::<u64>().ok())
        })
        .unwrap_or(0);
    eprintln!(
        "[MEM] modules={} signals={} total_bits={} hier_map={} rss_mb={}",
        ir.modules.len(),
        n_sig,
        n_bits,
        ir.hier_signal_map.len(),
        rss / 1024
    );
    // Komposisi IR: jumlah proses & statement — biang ukuran body IR.
    let (n_proc, n_stmt) = {
        let mut p = 0usize;
        let mut s = 0usize;
        for m in ir.modules.values() {
            for proc in &m.processes {
                p += 1;
                s += ir_stmt_count(match proc {
                    mivon_ir::Process::Combinational { body, .. } => body,
                    mivon_ir::Process::CombReactive { body, .. } => body,
                    mivon_ir::Process::Sequential { body, .. } => body,
                    mivon_ir::Process::Initial { body, .. } => body,
                    mivon_ir::Process::Final { body, .. } => body,
                    mivon_ir::Process::AlwaysWithDelay { body, .. } => body,
                });
            }
        }
        (p, s)
    };
    eprintln!("[MEM]   processes={} stmts={}", n_proc, n_stmt);
    for (mname, sname, w, ad, dims) in &top {
        eprintln!(
            "[MEM]   top {}.{} width={} array_depth={} array_dims={:?}",
            mname, sname, w, ad, dims
        );
    }
}

/// Jumlah statement IR pada body proses (top-level) — indikator ukuran body.
fn ir_stmt_count(body: &[mivon_ir::IrStmt]) -> usize {
    body.len()
}

/// Body utama program (dijalankan di thread dengan stack besar oleh `main`).
fn real_main() {
    // Tune glibc allocator SEJAK AWAL — mmap utk alok besar (RSS balik ke OS
    // saat free) & trim threshold 1MB (anti-high-water arena design besar).
    tune_heap_allocator();
    let mut cli = Cli::parse();

    // ── Config file TOML (configs/*.toml) ──
    // `--config <path>` eksplisit; tanpa itu, auto-load `configs/compiler.toml`
    // bila ada. Field config diterapkan HANYA bila CLI tidak menyetelnya
    // (CLI menang). Jobs di-terapkan ke rayon pool di bawah.
    let cfg = match mivon_core::config::MivonConfig::load_auto(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("warning: {}", e);
            mivon_core::config::MivonConfig::default()
        }
    };
    apply_config_to_cli(&mut cli, &cfg);

    // Initialize warning filter from CLI flags
    init_warn_filter(&cli);

    // Configure rayon thread pool with larger stack for deep recursion in parser
    // Some SV files have deeply nested blocks that need more than the default 2MB stack
    let mut pool = rayon::ThreadPoolBuilder::new().stack_size(16 * 1024 * 1024); // 16MB stack
    if let Some(jobs) = cfg.compiler.jobs {
        if jobs > 0 {
            pool = pool.num_threads(jobs);
        }
    }
    pool.build_global().ok();

    // ── Subcommand: bersihkan database MICD (seperti `cargo clean`) ──
    if let Some(cmd) = &cli.cmd {
        match cmd {
            crate::cli::MivonCmd::Clean => run_clean(),
            crate::cli::MivonCmd::Inspect(a) => dispatch_inspect(a),
            crate::cli::MivonCmd::Lint(a) => dispatch_lint(a),
            crate::cli::MivonCmd::Elab(a) => dispatch_elab(a),
            crate::cli::MivonCmd::Sim(a) => dispatch_sim(a),
            crate::cli::MivonCmd::Cov(a) => dispatch_cov(a),
            crate::cli::MivonCmd::Wave(a) => dispatch_wave(a),
            crate::cli::MivonCmd::Fmt(a) => dispatch_fmt(a),
            crate::cli::MivonCmd::Gen(a) => dispatch_gen(a),
            crate::cli::MivonCmd::Prof(a) => dispatch_prof(a),
            crate::cli::MivonCmd::Check(a) => dispatch_check(a),
            crate::cli::MivonCmd::Bench(a) => dispatch_bench(a),
            crate::cli::MivonCmd::Synth(a) => dispatch_synth(a),
            crate::cli::MivonCmd::Update(a) => dispatch_update(a),
            crate::cli::MivonCmd::Emu(a) => dispatch_emu(a),
            crate::cli::MivonCmd::Batch(a) => dispatch_batch(a),
            crate::cli::MivonCmd::Memcheck(a) => dispatch_memcheck(a),
            crate::cli::MivonCmd::Tbgen(a) => dispatch_tbgen(a),
            crate::cli::MivonCmd::Waiver(a) => dispatch_waiver(a),
            crate::cli::MivonCmd::Vault(a) => dispatch_vault(a),
            crate::cli::MivonCmd::Ipxact(a) => dispatch_ipxact(a),
            crate::cli::MivonCmd::DesignRepo(a) => dispatch_design_repo(a),
            crate::cli::MivonCmd::Project(a) => dispatch_project(a),
            crate::cli::MivonCmd::Sdc(a) => dispatch_sdc(a),
            crate::cli::MivonCmd::EquivCheck(a) => dispatch_equiv_check(a),
            crate::cli::MivonCmd::Regression(a) => dispatch_regression(a),
            crate::cli::MivonCmd::Eco(a) => dispatch_eco(a),
            crate::cli::MivonCmd::CovClosure(a) => dispatch_cov_closure(a),
        }
    }

    // ── GUI mode: launch the native egui application ──
    #[cfg(feature = "gui")]
    if cli.gui {
        if let Err(e) = mivon_api::gui::run() {
            eprintln!("GUI error: {}", e);
            process::exit(1);
        }
        return;
    }
    #[cfg(not(feature = "gui"))]
    if cli.gui {
        eprintln!("GUI tidak tersedia: compile dengan --features gui");
        process::exit(1);
    }

    // ── LSP mode: start language server (stdio transport) ──
    #[cfg(feature = "lsp")]
    if cli.lsp {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime for LSP");
        rt.block_on(mivon_api::lsp::run_lsp_server());
        return;
    }
    #[cfg(not(feature = "lsp"))]
    if cli.lsp {
        eprintln!("LSP server not available: compile with --features lsp");
        return;
    }

    // ── Bangun GlobalEnv (enterprise context architecture, doc/env.md) ──
    // ConfigContext memakai config yang sudah di-load (tanpa baca ulang);
    // CLI override diterapkan ke config (CLI menang atas file/env).
    let mut cfgctx = mivon_api::env::ConfigContext::from_loaded(cfg, cli.config.as_deref());
    let cli_overrides = mivon_api::env::EnvCliOptions {
        max_time: cli.max_time,
        force_sim: Some(cli.force_sim),
        recompile: cli.recompile,
        elab_mode: cli.config_elab_mode.clone(),
        coverage_threshold: cli.coverage_threshold,
        deep_debug: Some(cli.deep_debug),
        snap_interval: Some(cli.snap_interval),
        ..Default::default()
    };
    cli_overrides.apply(&mut cfgctx);

    // Workspace di-seed dari CLI (sumber eksplisit) — menghindari scan
    // direktori penuh yang lambat di env startup.
    let mut cli_sources: Vec<std::path::PathBuf> =
        cli.files.iter().map(std::path::PathBuf::from).collect();
    if let Some(ref fpath) = cli.filelist {
        match read_project_file(fpath) {
            Ok(flist) => cli_sources.extend(flist.into_iter().map(std::path::PathBuf::from)),
            Err(e) => eprintln!("warning: filelist '{}': {}", fpath, e),
        }
        // [foreign] di-load di run()/run_fast() (pipeline aktual) — bukan di
        // sini (workspace setup hanya seed sources; menghindari load ganda).
    }
    let mut ws =
        mivon_api::env::WorkspaceContext::open_in(&std::env::current_dir().unwrap_or_default());
    ws.set_explicit_sources(cli_sources);
    for d in &cli.incdirs {
        ws.add_incdir(d);
    }
    for def in &cli.defines {
        if let Some((k, v)) = def.split_once('=') {
            ws.add_define(k, v);
        } else {
            ws.add_define(def, "");
        }
    }
    for d in &cli.libdirs {
        ws.add_libdir(d);
    }
    for f in &cli.libfiles {
        ws.add_libfile(f);
    }

    let mut env = match mivon_api::env::for_cli(cfgctx, ws) {
        Ok(env) => env,
        Err(e) => {
            eprintln!("warning: startup env: {} — memakai env minimal", e);
            mivon_api::env::GlobalEnv::minimal()
        }
    };

    let result = run(cli.clone(), &mut env);
    if cli.gdiag {
        eprintln!(
            "{}",
            mivon_core::diagnostics::diag_global().coverage_report()
        );
    }

    // ── Lifecycle shutdown + ringkasan telemetry ──
    if !cli.quiet {
        eprintln!("[env] {}", env.telemetry().summary());
        eprintln!("[env] uptime={:?}", env.uptime());
    }
    mivon_api::env::shutdown(&mut env);

    if let Err(e) = result {
        // Use TerminalEmitter for pretty diagnostic output
        let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
        let diag = e.to_diagnostic();
        let _ = emitter.emit(&diag);
        process::exit(e.exit_code());
    }
}

/// Flag mesin yang harus berlaku di SEMUA jalur eksekusi (legacy `run` dan
/// `run_fast`). Dulu `--jit-body` hanya dipasang di jalur legacy dan
/// `--cycle-fusion` hanya di jalur fast → flag diabaikan diam-diam.
#[derive(Debug, PartialEq, Eq)]
struct EngineFlags {
    mir_jit: bool,
    cycle_fusion: bool,
}

fn engine_flags(cli: &Cli) -> EngineFlags {
    EngineFlags {
        mir_jit: cli.jit_body,
        cycle_fusion: cli.cycle_fusion,
    }
}

/// Pasang flag mesin dari CLI — dipanggil kedua jalur agar tak ada yang
/// tertinggal saat pipeline diubah.
fn apply_engine_flags(engine: &mut SimulationEngine, cli: &Cli) {
    let f = engine_flags(cli);
    engine.use_mir_jit = f.mir_jit;
    engine.use_cycle_fusion = f.cycle_fusion;
}

/// Run compilation + simulation using the new parallel pipeline (CompileSession + FastLexer).
fn run_fast(
    cli: Cli,
    _timescale: Option<(String, String)>,
    env: &mut mivon_api::env::GlobalEnv,
) -> Result<(), SimError> {
    env.telemetry()
        .trace("run_fast", "pipeline paralel dimulai");
    let mut sources: Vec<PathBuf> = cli.files.iter().map(PathBuf::from).collect();
    if let Some(ref fpath) = cli.filelist {
        let flist = read_project_file(fpath)?;
        sources.extend(flist.into_iter().map(PathBuf::from));
        // [foreign] dari file project (VHPI/PLI/DPI).
        match mivon_api::read_project_with_foreign(fpath) {
            Ok(proj) => load_project_foreign_libs(&proj, &cli),
            Err(e) => eprintln!("warning: [foreign] '{}': {}", fpath, e),
        }
    }

    let config = SessionConfig {
        sources,
        incdirs: cli.incdirs.iter().map(PathBuf::from).collect(),
        defines: cli
            .defines
            .iter()
            .filter_map(|d| d.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        top_module: cli.top.clone(),
        auto_incdirs: cli.files.is_empty(),
        libdirs: cli.libdirs.iter().map(PathBuf::from).collect(),
        libfiles: cli.libfiles.iter().map(PathBuf::from).collect(),
        use_fast_lexer: !cli.legacy_lexer,
        use_lazy_elab: cli.lazy,
        // run_fast tidak pernah dipakai untuk `.mv` (jalur legacy + inline F8),
        // jadi inline_sources selalu kosong di sini.
        inline_sources: std::collections::HashMap::new(),
    };

    let mut session = CompileSession::new(config);

    // ── Animasi pipeline (baris status gaya Cargo) ──
    // Dideklarasikan dulu, di-START nanti tepat sebelum compile() — setup MICD
    // dan mode khusus (compile-only/lazy) tidak menggambar animasi apa pun.
    let mut anim: Option<PipelineAnimator> = None;

    // ── MICD: persistent incremental compilation database ──
    // Otomatis (bukan flag tambahan): restore AST/combined-source file yang
    // tidak berubah → lex/parse di-skip. Saat --recompile, database tetap
    // dibuka (agar save menulis hasil full-rebuild) tapi restore di-skip.
    // MICD scoped per project (ProjectID): OpenTitan dan test/counter.sv
    // tidak pernah berbagi database → tidak ada kontaminasi lintas project.
    {
        let micd_root = micd_root_for(&cli);
        let proot = std::env::current_dir().unwrap_or_default();
        // Fase 6: pid mencakup libfiles (sebelumnya sources saja → dua run
        // dengan libfile beda berbagi pid = kontaminasi lintas project).
        let mut pid_sources = session.config.sources.clone();
        for lib in &session.config.libfiles {
            if !pid_sources.contains(lib) {
                pid_sources.push(lib.clone());
            }
        }
        let pid = mivon_compiler::micd::MicdDatabase::project_id(
            &proot,
            &pid_sources,
            &session.config.incdirs,
            &session.config.defines,
        );
        let mut db = mivon_compiler::micd::MicdDatabase::open_project_with_context(
            &micd_root,
            &pid,
            &proot,
            &pid_sources,
        );
        // `--cache-clear` HARUS menghapus database SEBELUM restore/compile —
        // clear di akhir run (lihat bawah) telat: restore sudah memakai data
        // stale, dan bila run gagal (error elaborasi) clear tidak pernah
        // dieksekusi sama sekali (run abort sebelum sampai ke sana).
        if cli.cache_clear {
            let _ = db.clear();
        }
        if cli.recompile {
            // Full rebuild: buka db tanpa restore; hasil parse tetap disimpan
            // supaya run berikutnya (tanpa --recompile) bisa restore.
            session.open_micd_no_restore(db);
        } else {
            let t0 = std::time::Instant::now();
            let restored = session.attach_micd(db);
            if restored > 0 && !cli.quiet && !anim_active(&anim) {
                eprintln!(
                    "[MICD] restored {} cached design(s) in {:?} ({} file(s) parse di-skip)",
                    restored,
                    t0.elapsed(),
                    restored
                );
            }
        }
    }

    if cli.profile {
        session.enable_profiling();
    }

    // ── Compile-only mode: parse only, skip elaboration & simulation ──
    if cli.compile_only {
        let (design, module_index) = if cli.lazy {
            let (_design, hir_count, index_len) = session.compile_lazy_only()?;
            if !cli.quiet {
                session.print_timing();
                println!("Modules indexed: {}", index_len);
                println!("Lazy-elaborated modules (HIR): {}", hir_count);
                if let Some(_top) = &session.config.top_module {
                    println!("HIR query ready: session.elaborate_lazy_module(...)");
                }
            }
            micd_save_and_print(&mut session, cli.quiet, anim_active(&anim));
            return Ok(());
        } else {
            session.compile()?
        };
        let index_len = module_index.len();
        if !cli.quiet {
            session.print_timing();
            println!("Modules indexed: {}", index_len);
        }
        micd_save_and_print(&mut session, cli.quiet, anim_active(&anim));
        if cli.print_ast {
            println!("{:#?}", design);
        }
        return Ok(());
    }
    // ── Lazy mode: skip full elaboration ──
    let top_name = cli.top.as_deref();
    if cli.lazy {
        let (_design, _ir_design, index_len) = session.compile_and_elaborate(top_name)?;
        if !cli.quiet {
            session.print_timing();
            println!(
                "Modules indexed: {}, lazy HIR modules: {}",
                index_len,
                session.lazy_elaborated_count()
            );
        }
        micd_save_and_print(&mut session, cli.quiet, anim_active(&anim));
        return Ok(());
    }

    // ── Full pipeline: compile + elaborate ──
    // START animasi di sini: tepat sebelum compile() dikerjakan — setelah semua
    // setup (MICD, mode) selesai. Baris pertama muncul bersama mulai kerja.
    anim = PipelineAnimator::start(anim_enabled(&cli));
    if anim_active(&anim) {
        anim_set_exts(&anim, &session.config.sources);
        anim_phase_running(&anim, Phase::Lex);
        anim_phase_running(&anim, Phase::Par);
        anim_set_files(&anim, session.config.sources.len() as u64, 0);
        // Progres per-file dari rayon: nama file .sv/.svh/.v/.vh terakhir +
        // hitungan done/cached, di-update dari worker tanpa clone per-file.
        if let Some(a) = anim.as_ref() {
            let h = a.handle();
            session.set_file_progress(move |path, cached| h.file_done(path, cached));
        }
    }
    let (design, index_len) = if cli.recompile {
        if !cli.quiet && !anim_active(&anim) {
            eprintln!("Forcing full recompile...");
        }
        let all_sources: Vec<PathBuf> = session.config.sources.clone();
        let (design, module_index) = session.compile_incremental(&all_sources)?;
        let index_len = module_index.len();
        if !cli.quiet && !anim_active(&anim) {
            session.print_timing();
        }
        (design, index_len)
    } else {
        let (design, module_index) = session.compile()?;
        let index_len = module_index.len();
        if !cli.quiet && !anim_active(&anim) {
            session.print_timing();
        }
        (design, index_len)
    };

    // Emit parse errors collected during compilation
    // (animasi masih aktif → pakai wrapper suspend agar teks tidak menyambung
    // dengan baris status)
    if !session.parse_errors.is_empty() {
        emit_diags_anim(&anim, &session.parse_errors);
    }

    let mut pmark = std::time::Instant::now();
    let mut mark = |label: &str| {
        if std::env::var("MIVON_PHASE_TIMING").is_ok() {
            eprintln!("[PHASE] {} = {:?}", label, pmark.elapsed());
        }
        pmark = std::time::Instant::now();
    };
    mark("after-compile-parsed");

    if anim_active(&anim) {
        anim_phase_done(&anim, Phase::Lex);
        anim_phase_done(&anim, Phase::Par);
        anim_set_modules(&anim, design.modules.len() as u64);
        anim_set_files(
            &anim,
            session.config.sources.len() as u64,
            session.config.sources.len() as u64,
        );
    }

    // ── MICD: simpan hasil parse SEGERA (sebelum elaborate) agar cache
    // parse tetap tersimpan walau elaborasi gagal. ──
    micd_save_and_print(&mut session, cli.quiet, anim_active(&anim));

    // ── Optimasi memori: lepas AST parse (prev_designs + combined sources)
    // yang sudah di-<persist ke MICD — elaborasi design besar (OpenTitan)
    // butuh RAM; menahan clone AST penuh kedua selama elaborate = OOM pada
    // mesin kecil (sebelumnya peak ~2.9GB parse, lalu elaborate +AST → swap). ──
    session.release_parse_cache();
    // malloc_trim: glibc menahan free heap (AST parse ~2GB) sebagai RSS —
    // mesin kecil (RAM 6GB) tidak punya ruang utk elaborasi+simulasi.
    // Trim mengembalikan page free ke OS → baseline RSS turun drastis.
    trim_heap();
    mark("after-save-release");

    if design.modules.is_empty() {
        // Tidak fatal bila ada package/interface/class — mode analisis:
        // tidak ada top yang bisa disimulasikan, tapi parse sudah valid.
        // Samakan dengan perilaku `run` (analisis sukses, simulasi skip).
        if !design.packages.is_empty()
            || !design.interfaces.is_empty()
            || !design.classes.is_empty()
        {
            if cli.print_ast {
                println!("{:#?}", design);
            }
            if !cli.quiet && !anim_active(&anim) {
                eprintln!("note: no modules found in design (packages/interfaces present, skipping simulation)");
            }
            micd_save_and_print(&mut session, cli.quiet, anim_active(&anim));
            return Ok(());
        }
        return Err(SimError::with_diag(
            DiagCode::ModuleNotFound,
            "no modules found in design",
        ));
    }
    if cli.print_ast {
        println!("{:#?}", design);
    }

    // Elaborate (source info dari merged source — sudah tersimpan di MICD).
    if anim_active(&anim) {
        // Subjek fase ELA: HANYA top eksplisit (`--top`) — tanpa fallback
        // diam-diam ke module pertama (bukan top sebenarnya).
        anim_set_top(&anim, top_name.unwrap_or(""));
        anim_phase_running(&anim, Phase::Ela);
    }
    // ── Reuse IR cache (db.md "5. elaborate/"): bila SELURUH file di-restore
    // dari MICD (tidak ada yang berubah) dan hasil elaborasi sebelumnya
    // tersimpan di cache, langsung pakai IR — elaborator di-skip penuh.
    // Hanya aman bila run sebelumnya elaborasi BERSIH (cache elaborate hanya
    // disimpan setelah gate error/recovery lolos di run itu).
    let mut restored_ir: Option<mivon_ir::IrDesign> = None;
    // `micd_restored_count()` di-reset `save_micd()`; set path tidak → pakai
    // `micd_restore_complete()` (Fase 6: seluruh restorable incl. libfiles)
    // untuk deteksi warm run setelah save parse.
    if !cli.recompile && session.micd_restore_complete() {
        // Top eksplisit (`--top`) atau default (module pertama — sama dengan
        // pilihan elaborator). Cache IR disimpan per top (`ir:<top>`) dan
        // diikat sidik input (Fase 6: anti stale bila input berubah).
        let top = top_name.or_else(|| design.modules.first().map(|m| m.name.as_str()));
        if let Some(top) = top {
            if let Some(ih) = session.micd_elaboration_inputs_hash(top) {
                let ctx = format!("{:?}", pick_elab_mode(&cli));
                restored_ir = session.restore_elaborate_ir(top, ih, &ctx);
            }
        }
    }

    let (ir_design, elab_diags, recovered, from_cache);
    let mut elab_opt: Option<Elaborator> = None;
    if let Some(ir) = restored_ir {
        ir_design = ir;
        elab_diags = Vec::new();
        recovered = false;
        from_cache = true;
        if anim_active(&anim) {
            anim_phase_done(&anim, Phase::Ela);
            anim_phase_done(&anim, Phase::Opt);
            anim_phase_done(&anim, Phase::Ver);
            let e = 0;
            anim_finish(&mut anim, true, e, 0);
        }
        if !cli.quiet && !anim_active(&anim) {
            eprintln!(
                "[MICD] IR cache hit — elaborator di-skip (top '{}')",
                ir_design.top.name
            );
        }
    } else {
        // Parsing menghasilkan error → desain parsial; elaborasi penuh
        // sia-sia (3888 file = ~6 menit). Perilaku terminal SAMA (readiness
        // "Simulasi: TIDAK SIAP" + exit error) tanpa membayar biaya
        // elaborate pada desain yang sudah rusak.
        if session.parse_errors.iter().any(|d| d.is_error()) {
            ir_design = mivon_ir::IrDesign::default();
            elab_diags = Vec::new();
            recovered = false;
            from_cache = false;
            elab_opt = None;
            if anim_active(&anim) {
                anim_phase_done(&anim, Phase::Ela);
                anim_phase_done(&anim, Phase::Opt);
                anim_phase_done(&anim, Phase::Ver);
                // Elaborasi di-skip karena parse error. Ringkasan animasi harus
                // mencerminkan penyebabnya — kalau di-hardcode 0, header
                // "Compile Failed · 0 errors" bertentangan dengan blok
                // "Kesiapan Simulasi" (✗ Parse (N error)). Hitung dari
                // parse_errors yang sama, dengan aturan yang sama (hanya
                // level ERROR yang dihitung, warning dihitung terpisah).
                let e = session.parse_errors.iter().filter(|d| d.is_error()).count();
                let w = session
                    .parse_errors
                    .iter()
                    .filter(|d| d.level == DiagLevel::Warning)
                    .count();
                anim_finish(&mut anim, false, e, w);
            }
        } else {
            let (source_lines, source_file) = session.source_info().unwrap_or_default();
            mark("before-elaborator");
            let mut elab = if source_lines.is_empty() {
                Elaborator::new(design)
            } else {
                Elaborator::with_source(design, source_lines, source_file)
            };
            // Mode elaborasi: dengan `--top` eksplisit → StrictSimulation (top wajib).
            // Tanpa `--top` (run file satu-per-satu) → AnalysisRecovery: top yang tidak
            // unik (multiple candidate / circular / missing root) TIDAK menggagalkan
            // analisis — diagnostik dilaporkan, simulasi & VCD dinonaktifkan (Rule 4).
            let elab_mode = pick_elab_mode(&cli);
            ir_design = match elab.elaborate(top_name, elab_mode) {
                Ok(d) => d,
                Err(e) => {
                    let diags = elab.flush_diagnostics();
                    if anim_active(&anim) {
                        // Elaborasi gagal → jumlah error nyata, bukan konstanta 1
                        // (alasan sama dengan jalur legacy di atas).
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
            elab_diags = elab.flush_diagnostics();
            mark("after-elaborate");
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
            emit_diags(&elab_diags);

            // Mode analisis (recovery): top-level tidak dapat ditentukan secara unik
            // karena `--top` tidak diberikan. Semua diagnostik sudah dilaporkan —
            // simulasi & VCD TIDAK dijalankan (Rule 2/4).
            recovered = elab.recovered;
            from_cache = false;
            elab_opt = Some(elab);
        }
    }

    // ── Simulation Readiness Check (Rule 6) — full validation ──
    let elab_errs = elab_diags.iter().filter(|d| d.is_error()).count();
    let has_elab_errors = elab_errs > 0;
    // Hanya diagnostic level ERROR yang menggagalkan kesiapan simulasi.
    // Paket warning/notice parse (mis. E1005 "skipping top-level construct")
    // tetap masuk parse_errors utk transparansi, tapi TIDAK dihitung sbg
    // error dan TIDAK memblokir sim — sebelumnya parse_errors.len() ikut
    // menghitung warning + duplikat → "✗ Parse (234 error)" padahal error
    // nyata cuma 1-2 (OpenTitan DV filelist). (sesi opentitan fiks)
    let parse_errs = session.parse_errors.iter().filter(|d| d.is_error()).count();
    let has_parse_errors = parse_errs > 0;

    // Per-tahap validation: hitung error per kategori dari elab_diags
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

    use mivon_core::diagnostics::diagnostic::{
        DiagCode as DCode, DiagLevel as DLevel, Diagnostic as DDiag,
    };

    /// Ekstrak lokasi error dari Diagnostic: source_snippet → spans → message parsing.
    fn diag_loc(d: &mivon_core::diagnostics::diagnostic::Diagnostic) -> Option<String> {
        if let Some(ss) = &d.source_snippet {
            return Some(format!("{}:{}:{}", ss.file, ss.line, ss.col));
        }
        if let Some(span) = d.spans.first() {
            return Some(format!("{}:{}:{}", span.file, span.start, span.end));
        }
        // Coba parse format "line N: msg" dari message
        let msg = d.message.to_string();
        if let Some(rest) = msg.strip_prefix("line ") {
            if let Some(end) = rest.find(':') {
                if let Ok(line) = rest[..end].trim().parse::<usize>() {
                    return Some(format!("baris {}", line));
                }
            }
        }
        None
    }

    /// Cetak maks N error per kategori dengan lokasi + pesan.
    fn print_err_category(
        label: &str,
        count: usize,
        errors: &[&mivon_core::diagnostics::diagnostic::Diagnostic],
        max_show: usize,
    ) {
        if count == 0 {
            println!("✓ {}", label);
            return;
        }
        println!("✗ {} ({} error)", label, count);
        for (shown, d) in errors.iter().enumerate() {
            if shown >= max_show {
                break;
            }
            let loc = diag_loc(d).unwrap_or_else(|| "?".into());
            // Potong message jika terlalu panjang
            let msg = d.message.to_string();
            let msg_short = if msg.len() > 80 {
                truncate_ellipsis(&msg, 77)
            } else {
                msg
            };
            println!("  {} | {}", loc, msg_short);
        }
        if count > max_show {
            println!("  ... dan {} error lainnya", count - max_show);
        }
    }

    if !cli.quiet {
        println!("\nKesiapan Simulasi");

        // Kumpulkan error per kategori
        let parse_err_diags: Vec<_> = session
            .parse_errors
            .iter()
            .filter(|d| d.is_error())
            .collect();
        let sem_err_diags: Vec<_> = elab_diags
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
        let hier_err_diags: Vec<_> = elab_diags
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
        let top_err_diags: Vec<_> = elab_diags
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
        let dpi_err_diags: Vec<_> = elab_diags
            .iter()
            .filter(|d| {
                d.is_error()
                    && matches!(
                        d.code,
                        DiagCode::DpiImportNotFound | DiagCode::DpiError | DiagCode::DpiScopeError
                    )
            })
            .collect();

        const MAX_SHOW: usize = 10;

        // 1. Parse
        print_err_category("Parse", parse_errs, &parse_err_diags, MAX_SHOW);
        // 2. Semantik
        print_err_category("Semantik", sem_errs, &sem_err_diags, MAX_SHOW);
        // 3. Hierarki
        print_err_category("Hierarki", hier_errs, &hier_err_diags, MAX_SHOW);
        // 4. Resolusi Top
        if has_elab_errors || recovered {
            print_err_category("Resolusi Top", top_errs, &top_err_diags, MAX_SHOW);
        } else {
            println!("✓ Resolusi Top");
        }
        // 5. Penghubung DPI
        print_err_category("Penghubung DPI", dpi_errs, &dpi_err_diags, MAX_SHOW);
        println!();

        // Kesiapan keseluruhan
        let any_ready =
            has_parse_errors || sem_errs > 0 || hier_errs > 0 || has_elab_errors || dpi_errs > 0;
        if any_ready && !cli.force_sim {
            println!("Simulasi: TIDAK SIAP");
            println!("Simulasi dibatalkan.");
            let total_errs = parse_errs + elab_errs;
            let first_err = parse_err_diags
                .first()
                .or_else(|| sem_err_diags.first())
                .or_else(|| hier_err_diags.first())
                .or_else(|| top_err_diags.first())
                .or_else(|| dpi_err_diags.first());
            let loc_str = first_err.and_then(|d| diag_loc(d));
            let msg = match &loc_str {
                Some(loc) => format!(
                    "simulasi dibatalkan: {} error ({} parse + {} semantik + {} hierarki + {} top + {} dpi) — error pertama di {}",
                    total_errs, parse_errs, sem_errs, hier_errs, top_errs, dpi_errs, loc
                ),
                None => format!(
                    "simulasi dibatalkan: {} error ({} parse + {} semantik + {} hierarki + {} top + {} dpi)",
                    total_errs, parse_errs, sem_errs, hier_errs, top_errs, dpi_errs
                ),
            };
            let mut diag = DDiag::new(DLevel::Error, DCode::UnexpectedToken, msg);
            if let Some(d) = first_err {
                if let Some(ss) = &d.source_snippet {
                    diag = diag.with_source_snippet(ss.clone());
                }
            }
            return Err(SimError::Diagnostic(diag));
        } else if recovered {
            println!("Simulasi: TIDAK SIAP (mode analisis)");
            println!("Top-level design tidak bisa ditentukan secara unik — simulasi & VCD dinonaktifkan.");
        } else {
            println!("Simulasi: SIAP");
        }
    }

    // Giá stats IR sebelum sim (debug OOM, MIVON_MEM_STATS=1).
    print_ir_mem_stats(&ir_design);

    // ── Gate: jangan simulasikan bila masih ada error ──
    use mivon_core::diagnostics::diagnostic::Diagnostic;
    if (has_elab_errors || has_parse_errors) && !cli.force_sim {
        if !cli.quiet {
            if has_parse_errors {
                eprintln!("\n⚠  Simulasi DIBATALKAN: {} parse error.", parse_errs);
            }
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
        let total_errs = parse_errs + elab_errs;
        let first_err = session
            .parse_errors
            .iter()
            .find(|d| d.is_error())
            .or_else(|| elab_diags.iter().find(|d| d.is_error()));
        let loc_str = first_err.and_then(|d| {
            d.source_snippet
                .as_ref()
                .map(|ss| format!("{}:{}:{}", ss.file, ss.line, ss.col))
        });
        let msg = match &loc_str {
            Some(loc) => format!(
                "simulasi dibatalkan: {} error total ({} parse + {} elaborasi) — error pertama di {}",
                total_errs, parse_errs, elab_errs, loc
            ),
            None => format!(
                "simulasi dibatalkan: {} error total ({} parse + {} elaborasi) — design belum 100% bersih",
                total_errs, parse_errs, elab_errs
            ),
        };
        let mut diag = Diagnostic::error(DiagCode::ModuleNotFound, msg);
        if let Some(ss) = first_err.and_then(|d| d.source_snippet.as_ref()) {
            diag = diag.with_source_snippet(ss.clone());
        }
        return Err(SimError::Diagnostic(diag));
    }

    // ── Recovery/analisis mode (tanpa `--top`): jangan simulasikan modul tebakan ──
    if recovered && !cli.force_sim {
        if !cli.quiet {
            println!("\nTop-level design tidak dapat ditentukan secara unik — mode analisis.");
            println!("Simulation & VCD dinonaktifkan.\n");
        }
        micd_save_and_print(&mut session, cli.quiet, anim_active(&anim));
        return Ok(());
    }

    // ── MICD: tandai elaborasi sukses (verify-only save, ringan) ──
    session.micd_mark_elaborated();
    // Isi kategori elaborate/ + generate/ dari IR (db.md "5. elaborate/",
    // "16. generate/") — save_micd di atas dipanggil sebelum elaborate, jadi
    // kategori ini baru bisa diisi setelah IR tersedia. Design post-expansion
    // (elab.design) dipakai fallback elaborate/ untuk module top yang
    // sub-instance-nya dikonsumsi flatten IR. Statistik optimasi elaborator
    // ikut disimpan (db.md "6. optimize/", "10. expression/"). Jalur cache
    // hit melewati blok ini — IR sudah tersimpan dari run sebelumnya.
    if !from_cache {
        let elab = elab_opt
            .as_ref()
            .expect("elaborator pada jalur elaborasi penuh");
        session.save_elaborate_cache(
            &ir_design,
            Some(&elab.design),
            Some(elab.opt_stats.snapshot()),
            &format!("{:?}", pick_elab_mode(&cli)),
        );
    }

    if !cli.quiet {
        println!("Modules indexed: {}", index_len);
    }

    // Configure remote cache backend
    if let Some(ref remote_dir) = cli.cache_remote_dir {
        let sync_mode =
            mivon_compiler::cache::cache_manager::RemoteSyncMode::from_name(&cli.cache_remote_sync);
        match mivon_compiler::cache::FilesystemCache::new(remote_dir) {
            Ok(backend) => {
                let remote: std::sync::Arc<dyn mivon_compiler::cache::RemoteCacheBackend> =
                    std::sync::Arc::new(backend);
                session.set_remote_cache(remote, sync_mode);
                if !cli.quiet {
                    eprintln!(
                        "Remote cache enabled: {} (sync: {})",
                        remote_dir,
                        sync_mode.as_str()
                    );
                }
            }
            Err(e) => {
                eprintln!("Warning: cannot open remote cache '{}': {}", remote_dir, e);
            }
        }
    }

    // Clear all cache if requested (MICD sudah dihapus di awal run — clear
    // di sini hanya membersihkan cache compile/JIT/remote, MICD fresh hasil
    // rebuild run ini dipertahankan agar run berikutnya cepat).
    if cli.cache_clear {
        session.clear_cache(false);
        if !cli.quiet {
            eprintln!("All caches cleared.");
        }
    }

    // Show cache stats if enabled
    if cli.cache_stats {
        let stats = session.cache_stats();
        eprintln!("{}", stats);
    }

    // Show profile report if enabled
    if cli.profile {
        if let Some(report) = session.profile_report() {
            eprintln!("{}", report);
        }
    }

    // ── Lazy mode info (when not compile-only) ──
    if cli.lazy && !cli.quiet {
        let lazy_count = session.lazy_elaborated_count();
        if let Some(ir) = session.get_cached_ir() {
            println!(
                "Module '{}': {} signals, {} processes | Lazy HIR: {} modules elapsed",
                ir.top.name,
                ir.top.signals.len(),
                ir.top.processes.len(),
                lazy_count
            );
        }
    } else if !cli.quiet {
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

    if cli.compile_only {
        if !cli.quiet {
            println!("Compile-only mode: skipping simulation");
        }
        return Ok(());
    }

    // ── Setup simulation ──
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

    // Default: sim dibatasi `DEFAULT_MAX_TIME_NS` (anti-OOM). Override via
    // `-T` / `--max-time <n>`; tanpa itu memakai default finite.
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
    // SIM-20: cycle-based mode (--cycle / --cycle-period)
    engine.set_cycle_based(cli.cycle_mode);
    engine.set_cycle_period(cli.cycle_period);
    engine.use_timing_wheel = cli.use_timing_wheel;
    engine.glitch_window = cli.glitch_window;
    if cli.glitch_window > 0 && !cli.quiet {
        println!(
            "Glitch detection enabled (window = {} time units)",
            cli.glitch_window
        );
    }

    // ── SIM-18: auto-checkpoint berkala (crash recovery) ──
    if let Some(ref cp_path) = cli.auto_checkpoint {
        let interval = cli.checkpoint_interval.max(1);
        engine.set_auto_checkpoint(cp_path, interval);
        if !cli.quiet {
            println!("Auto-checkpoint setiap {} cycle → '{}'", interval, cp_path);
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

    // Load VHPI shared libraries (IEEE 1076-2008) — jalur run_fast.
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

    // Load PLI shared libraries (IEEE 1364) — jalur run_fast.
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

    for pa in &cli.plusargs {
        if let Some((key, val)) = pa.split_once('=') {
            engine.plusargs.insert(key.to_string(), val.to_string());
        } else {
            engine.plusargs.insert(pa.clone(), String::new());
        }
    }

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
    for name in &cli.watch {
        engine.watchpoints.push(Watchpoint::Signal(name.clone()));
        if !cli.quiet {
            println!("  watchpoint: {}", name);
        }
    }

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

    // CSV waveform setup (fast path)
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

    // Flush runtime diagnostics (warnings) SEGERA setelah sim — SEBELUM
    // output hasil (coverage/completed/VCD): baris akhir murni hasil sim.
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
            println!("\nDebug events:\n{}", debugger.print_event_log());
        }
    }

    sim_finish::finish_simulation(&cli, &mut debugger, &vcd_path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::truncate_ellipsis;

    /// Refactor stage: adaptor `dispatch_*` tool tambahan dipindah ke
    /// `dispatch/extra.rs` — pastikan varian CLI yang dilayani masih
    /// ter-parse ke tipe args yang dipakai adaptor (regresi wiring).
    #[test]
    fn dispatch_extra_subcommands_still_parse() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(["mivon", "mbatch", "status"])
            .expect("mbatch status harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Batch(_))));
        let cli = crate::cli::Cli::try_parse_from(["mivon", "mvault", "list"])
            .expect("mvault list harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Vault(_))));
        let cli = crate::cli::Cli::try_parse_from(["mivon", "mipxact", "summary"])
            .expect("mipxact summary harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Ipxact(_))));
        // Sisa adaptor: pastikan semua varian yang dilayani extra.rs tercakup
        // (memcheck & tbgen sempat lolos dari test pertama).
        let cli = crate::cli::Cli::try_parse_from(["mivon", "memcheck", "/bin/true"])
            .expect("memcheck harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Memcheck(_))));
        let cli = crate::cli::Cli::try_parse_from(["mivon", "mtbgen", "-m", "dut"])
            .expect("mtbgen harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Tbgen(_))));
        let cli = crate::cli::Cli::try_parse_from([
            "mivon", "mwaiver", "add", "-r", "W1001", "-R", "alasan", "-o", "budi",
        ])
        .expect("mwaiver add harus parse");
        match cli.cmd {
            Some(crate::cli::MivonCmd::Waiver(crate::cli::MwaiverArgs {
                cmd: crate::cli::MwaiverCmd::Add { rule, .. },
            })) => assert_eq!(rule, "W1001"),
            _ => panic!("bukan Waiver::Add"),
        }
    }

    /// Refactor stage: adaptor manajemen desain & kualitas dipindah ke
    /// `dispatch/manage.rs` — regresi wiring parse CLI.
    #[test]
    fn dispatch_manage_subcommands_still_parse() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(["mivon", "mdesign-repo", "summary"])
            .expect("mdesign-repo summary harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::DesignRepo(_))));
        let cli = crate::cli::Cli::try_parse_from(["mivon", "mproject", "list"])
            .expect("mproject list harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Project(_))));
        let cli = crate::cli::Cli::try_parse_from(["mivon", "mregression", "summary"])
            .expect("mregression summary harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Regression(_))));
        let cli = crate::cli::Cli::try_parse_from(["mivon", "meco", "summary"])
            .expect("meco summary harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Eco(_))));
        let cli = crate::cli::Cli::try_parse_from([
            "mivon",
            "mcov-closure",
            "analyze",
            "--input",
            "x.json",
        ])
        .expect("mcov-closure analyze harus parse");
        match cli.cmd {
            Some(crate::cli::MivonCmd::CovClosure(crate::cli::McovClosureArgs {
                cmd: crate::cli::McovClosureCmd::Analyze { input },
            })) => assert_eq!(input, "x.json"),
            _ => panic!("bukan CovClosure::Analyze"),
        }
        let cli = crate::cli::Cli::try_parse_from(["mivon", "msdc", "x.sdc"])
            .expect("msdc harus parse");
        assert!(matches!(cli.cmd, Some(crate::cli::MivonCmd::Sdc(_))));
        // Field-level: menukar -g/-i akan tertangkap (assert varian saja tidak).
        let cli =
            crate::cli::Cli::try_parse_from(["mivon", "mequiv", "-g", "g.json", "-i", "i.json"])
                .expect("mequiv harus parse");
        match cli.cmd {
            Some(crate::cli::MivonCmd::EquivCheck(a)) => {
                assert_eq!(a.golden, "g.json");
                assert_eq!(a.impl_file, "i.json");
            }
            _ => panic!("bukan EquivCheck"),
        }
    }

    /// Refactor stage: adaptor tool inti dipindah ke `dispatch/tools.rs` —
    /// semua subcommand yang dilayani harus tetap ter-parse.
    #[test]
    fn dispatch_tools_subcommands_still_parse() {
        use clap::Parser;
        let cases: [&[&str]; 13] = [
            &["mivon", "minspect", "stats", "rtl"],
            &["mivon", "mlint", "rtl"],
            &["mivon", "melab", "rtl"],
            &["mivon", "msim", "rtl"],
            &["mivon", "mcov", "rtl"],
            &["mivon", "mwave", "tree", "x.vcd"],
            &["mivon", "mfmt", "a.sv"],
            &["mivon", "mgen", "a.mv"],
            &["mivon", "mprof", "rtl"],
            &["mivon", "mcheck", "rtl"],
            &["mivon", "mbench", "rtl"],
            &["mivon", "synth", "rtl"],
            &["mivon", "mupdate", "check"],
        ];
        for argv in cases {
            assert!(
                crate::cli::Cli::try_parse_from(argv).is_ok(),
                "gagal parse: {:?}",
                argv
            );
        }
    }

    /// Refactor stage: dispatch_emu + helper snapshot pindah ke
    /// `dispatch/emu.rs` — argumen CLI-nya harus tetap ter-parse ke field
    /// yang dipakai adaptor (config/run/snapshot).
    #[test]
    fn dispatch_emu_args_still_parse() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "mivon",
            "emu",
            "--config",
            "mesin.meu",
            "--run",
            "--snapshot-save",
            "snap.bin",
            "top.sv",
        ])
        .expect("emu harus parse");
        match cli.cmd {
            Some(crate::cli::MivonCmd::Emu(a)) => {
                assert_eq!(a.config.as_deref(), Some("mesin.meu"));
                assert!(a.run, "--run harus terbaca");
                assert_eq!(a.snapshot_save.as_deref(), Some("snap.bin"));
                assert_eq!(a.targets, vec!["top.sv".to_string()]);
            }
            _ => panic!("bukan Emu"),
        }
    }

    /// Regresi bug wiring (temuan reviewer-2): kedua flag mesin harus
    /// berlaku di SEMUA jalur. Dulu `--jit-body` hanya dipasang di jalur
    /// legacy dan `--cycle-fusion` hanya di jalur fast → diabaikan diam-diam.
    #[test]
    fn engine_flags_cover_both_pipelines() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "mivon",
            "--jit-body",
            "--cycle-fusion",
            "a.sv",
        ])
        .expect("flag harus parse");
        let f = crate::engine_flags(&cli);
        assert!(f.mir_jit, "--jit-body harus terbaca");
        assert!(f.cycle_fusion, "--cycle-fusion harus terbaca");

        let plain = crate::cli::Cli::try_parse_from(["mivon", "a.sv"]).expect("parse");
        let f = crate::engine_flags(&plain);
        assert!(!f.mir_jit && !f.cycle_fusion, "default harus mati");
    }

    /// Refactor stage: glue animasi pindah ke `anim_glue.rs` — gerbang
    /// animasi (quiet/step) harus berperilaku sama seperti sebelumnya.
    #[test]
    fn anim_enabled_gate_unchanged() {
        use clap::Parser;
        // MIVON_NO_ANIM mematikan paksa; jangan bergantung env mesin.
        if std::env::var("MIVON_NO_ANIM").is_ok() {
            return;
        }
        let plain = crate::cli::Cli::try_parse_from(["mivon", "a.sv"]).expect("parse");
        assert!(crate::anim_enabled(&plain), "default harus aktif");
        let quiet = crate::cli::Cli::try_parse_from(["mivon", "-q", "a.sv"]).expect("parse");
        assert!(!crate::anim_enabled(&quiet), "quiet harus mematikan animasi");
        let step = crate::cli::Cli::try_parse_from(["mivon", "--step", "a.sv"]).expect("parse");
        assert!(!crate::anim_enabled(&step), "step harus mematikan animasi");
    }

    /// Fuzz `verify_bug_0228`: pesan diagnostik >80 byte yang memuat karakter
    /// multi-byte (`—`) tak boleh panic saat dipotong (byte index di tengah
    /// karakter). Dulu `&msg[..77]` panic "not a char boundary".
    #[test]
    fn truncate_ellipsis_is_char_boundary_safe() {
        // '—' = 3 byte. Susun string sehingga byte ke-77 jatuh di dalamnya.
        let msg = format!("{}—{}", "x".repeat(76), "y".repeat(10));
        assert_eq!(msg.len(), 76 + 3 + 10);
        let out = truncate_ellipsis(&msg, 77);
        assert!(out.ends_with('…'));
        // 76 'x' (batas 76 aman) → dipotong ke 76 byte.
        assert_eq!(out, format!("{}…", "x".repeat(76)));
    }

    #[test]
    fn truncate_ellipsis_short_passthrough() {
        // Tidak dipanggil utk pesan pendek di call-site, tapi helper harus
        // mengembalikan utuh bila <= max.
        assert_eq!(truncate_ellipsis("halo", 77), "halo");
    }
}
