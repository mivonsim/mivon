// Allow large Result Err variant for SimError (intentional)
#![allow(clippy::result_large_err, clippy::type_complexity)]

mod anim_glue;
mod cli;
#[cfg(feature = "formal")]
mod formal_run;
mod pipeline_config;
mod report_glue;
mod run_fast;
mod run_legacy;
mod sim_finish;
mod source_io;
mod dispatch;

use clap::Parser as ClapParser;
use cli::Cli;
use run_legacy::run;
use dispatch::{
    dispatch_batch, dispatch_bench, dispatch_check, dispatch_cov, dispatch_cov_closure,
    dispatch_design_repo, dispatch_eco, dispatch_elab, dispatch_emu, dispatch_equiv_check,
    dispatch_fmt,
    dispatch_gen, dispatch_inspect, dispatch_ipxact, dispatch_lint, dispatch_memcheck,
    dispatch_prof, dispatch_project, dispatch_regression, dispatch_requirements,
    dispatch_sdc, dispatch_sim, dispatch_synth, dispatch_tbgen, dispatch_tcl,
    dispatch_update, dispatch_vault, dispatch_waiver, dispatch_wave,
};
use std::process;

use mivon_api::read_project_file;
use mivon_api::simulator::SimulationEngine;
use mivon_compiler::frontend::CompileSession;

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
            crate::cli::MivonCmd::Tcl(a) => dispatch_tcl(a),
            crate::cli::MivonCmd::Requirements(a) => dispatch_requirements(a),
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

#[cfg(test)]
mod tests {
    use super::truncate_ellipsis;
    use crate::anim_glue::anim_enabled;

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

    /// FEAT-09: wiring CLI requirements traceability.
    #[test]
    fn requirements_subcommand_parses() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "mivon",
            "mrequirements",
            "report",
            "-m",
            "req.csv",
            "-r",
            "res.json",
        ])
        .expect("mrequirements report harus parse");
        match cli.cmd {
            Some(crate::cli::MivonCmd::Requirements(crate::cli::MrequirementsArgs {
                cmd: crate::cli::MrequirementsCmd::Report {
                    mappings,
                    results,
                    format,
                    ..
                },
            })) => {
                assert_eq!(mappings, "req.csv");
                assert_eq!(results, "res.json");
                assert_eq!(format, "csv");
            }
            _ => panic!("bukan Requirements::Report"),
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
        assert!(anim_enabled(&plain), "default harus aktif");
        let quiet = crate::cli::Cli::try_parse_from(["mivon", "-q", "a.sv"]).expect("parse");
        assert!(!anim_enabled(&quiet), "quiet harus mematikan animasi");
        let step = crate::cli::Cli::try_parse_from(["mivon", "--step", "a.sv"]).expect("parse");
        assert!(!anim_enabled(&step), "step harus mematikan animasi");
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
