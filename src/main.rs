// Allow large Result Err variant for SimError (intentional)
#![allow(clippy::result_large_err, clippy::type_complexity)]

mod cli;
use clap::Parser as ClapParser;
use cli::Cli;
use std::path::{Path, PathBuf};
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
use mivon_elaboration::elaborator::{ElaborateMode, Elaborator};
use mivon_ir::LogicVec;
use mivon_parser::lexer::Lexer;
use mivon_parser::preprocessor::Preprocessor;
use mivon_parser::Parser;
use rayon::prelude::*;

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

/// Initialize global warning filter from CLI args (call once at startup)
fn init_warn_filter(cli: &Cli) {
    WARN_FILTER_NO_WARN.store(cli.no_warn, std::sync::atomic::Ordering::Relaxed);
    let _ = WARN_FILTER_ALLOW.set(cli.allow_codes.clone());
    let _ = WARN_FILTER_MAX.set(cli.max_warnings.unwrap_or(usize::MAX));
}

fn emit_diags(diags: &[mivon_core::diagnostics::diagnostic::Diagnostic]) {
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

/// Evaluasi ambang branch coverage (CI gate). Dipanggil SETELAH simulasi
/// selesai, terlepas dari `--coverage-ucdb` — agar config `coverage.branch_threshold`
/// benar-benar berlaku (sebelumnya hanya dicek saat menyimpan UCDB, sehingga
/// gate CI dari config tidak pernah aktif).
fn check_coverage_threshold(
    engine: &mivon_api::simulator::SimulationEngine,
    threshold: f64,
    quiet: bool,
) -> Result<(), SimError> {
    let stats = engine.coverage_stats();
    let branch_pct = stats.get("branch_percent").copied().unwrap_or(0.0);
    if branch_pct < threshold {
        let msg = format!(
            "COVERAGE FAILED: branch coverage {:.1}% < threshold {:.1}%",
            branch_pct, threshold
        );
        eprintln!("warning: {}", msg);
        return Err(SimError::with_diag(DiagCode::SimulationError, msg));
    } else if !quiet {
        println!(
            "Coverage threshold: {:.1}% >= {:.1}% ✅",
            branch_pct, threshold
        );
    }
    Ok(())
}

/// Pilih ElaborateMode: config `[elaborate] mode` menang, lalu `--top` eksplisit
/// (StrictSimulation), lalu AnalysisRecovery (tanpa top — analisis saja).
/// Nilai config yang tidak dikenal memicu warning lalu fallback ke logika CLI.
fn pick_elab_mode(cli: &Cli) -> ElaborateMode {
    match cli.config_elab_mode.as_deref() {
        Some(m) if m.eq_ignore_ascii_case("analysisrecovery") => ElaborateMode::AnalysisRecovery,
        Some(m) if m.eq_ignore_ascii_case("strictsimulation") => ElaborateMode::StrictSimulation,
        Some(m) => {
            eprintln!("warning: elaborate.mode '{}' tidak dikenal (pakai StrictSimulation / AnalysisRecovery) — memakai mode default", m);
            if cli.top.is_some() {
                ElaborateMode::StrictSimulation
            } else {
                ElaborateMode::AnalysisRecovery
            }
        }
        None => {
            if cli.top.is_some() {
                ElaborateMode::StrictSimulation
            } else {
                ElaborateMode::AnalysisRecovery
            }
        }
    }
}

/// Path root database MICD, dengan support `--target-dir`.
/// `--target-dir <path>` override path default (`.mivon/database`).
fn micd_root_for(cli: &Cli) -> std::path::PathBuf {
    use mivon_compiler::micd::MicdDatabase;
    if let Some(ref dir) = cli.target_dir {
        std::path::PathBuf::from(dir).join("database")
    } else {
        MicdDatabase::default_root()
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

/// Apakah animasi pipeline aktif (menggambar di terminal).
fn anim_active(anim: &Option<PipelineAnimator>) -> bool {
    anim.as_ref().map(|a| a.is_active()).unwrap_or(false)
}

/// Kapan animasi pipeline diizinkan: bukan quiet, bukan mode debug/step,
/// bukan compile-only/lazy (output verbose akan merusak area animasi).
/// `MIVON_NO_ANIM` menonaktifkan animasi paksa (dipakai saat profiling/debug
/// dengan gdb/scripting — area animasi bisa mengganggu output dan thread render).
fn anim_enabled(cli: &Cli) -> bool {
    if std::env::var("MIVON_NO_ANIM").is_ok() {
        return false;
    }
    !cli.quiet
        && !cli.compile_only
        && !cli.lazy
        && !cli.debug
        && !cli.step
        && cli.break_cycle.is_empty()
        && cli.break_change.is_empty()
        && cli.break_eq.is_empty()
        && cli.watch.is_empty()
}

/// Helpers ringkas untuk memanggil method animator tanpa unwrap.
fn anim_phase_running(anim: &Option<PipelineAnimator>, phase: Phase) {
    if let Some(a) = anim.as_ref() {
        a.phase_running(phase);
    }
}

fn anim_phase_done(anim: &Option<PipelineAnimator>, phase: Phase) {
    if let Some(a) = anim.as_ref() {
        a.phase_done(phase);
    }
}

fn anim_set_files(anim: &Option<PipelineAnimator>, total: u64, done: u64) {
    if let Some(a) = anim.as_ref() {
        a.set_files(total, done);
    }
}

fn anim_set_modules(anim: &Option<PipelineAnimator>, n: u64) {
    if let Some(a) = anim.as_ref() {
        a.set_modules(n);
    }
}

fn anim_finish(anim: &mut Option<PipelineAnimator>, ok: bool, errors: usize, warnings: usize) {
    if let Some(a) = anim.as_mut() {
        a.finish(ok, errors, warnings);
    }
}

fn anim_abort(anim: &mut Option<PipelineAnimator>, errors: usize, warnings: usize) {
    if let Some(a) = anim.as_mut() {
        a.abort(errors, warnings);
    }
}

/// Run formal verification (BMC) and print results.
/// Returns Err if any assertion fails (counterexample found — for CI/CD integration).
#[cfg(feature = "formal")]
fn run_formal(
    ir_design: &mivon_ir::IrDesign,
    bound: u64,
    quiet: bool,
    induction: bool,
    connect_pairs: &[String],
) -> Result<(), SimError> {
    use mivon_api::formal::*;
    let formal_cfg = FormalConfig {
        bound,
        induction,
        ..Default::default()
    };
    let mut formal_engine = FormalEngine::new(formal_cfg);
    let results = formal_engine.check_assertions_bmc(ir_design);

    // FORMAL-13: connectivity check (statis, tanpa solver).
    let conn_results = if !connect_pairs.is_empty() {
        let pairs: Vec<(String, String)> = connect_pairs
            .iter()
            .filter_map(|s| {
                let (a, b) = s.split_once(',')?;
                Some((a.trim().to_string(), b.trim().to_string()))
            })
            .collect();
        Some(mivon_api::formal::connectivity::check_connectivity(
            ir_design, &pairs,
        ))
    } else {
        None
    };

    if !quiet {
        println!("\n── Formal Verification Results (BMC bound={}) ──", bound);
    }

    let has_fail = results
        .iter()
        .any(|(_, r)| matches!(r, FormalResult::Counterexample(_)));
    let mut has_error = results
        .iter()
        .any(|(_, r)| matches!(r, FormalResult::Error(_)));

    // FORMAL-13: laporan konektivitas.
    if let Some(conn) = &conn_results {
        if !quiet {
            println!("\n── Connectivity Check ──");
            for r in conn {
                if let Some(err) = &r.error {
                    println!("  ! ERROR: {} → {} — {}", r.src, r.dst, err);
                } else if r.connected {
                    let path = r.path.join(" → ");
                    match r.path_len {
                        Some(0) => println!("  ✓ CONNECTED (self): {}", r.src),
                        Some(n) => {
                            println!(
                                "  ✓ CONNECTED: {} → {} via {} hop{} ({})",
                                r.src,
                                r.dst,
                                n,
                                if n == 1 { "" } else { "s" },
                                path
                            )
                        }
                        None => println!("  ✗ NOT CONNECTED: {} → {}", r.src, r.dst),
                    }
                } else {
                    println!("  ✗ NOT CONNECTED: {} → {}", r.src, r.dst);
                }
            }
        }
        // Pasangan dengan error ikut dihitung sebagai kegagalan CI gate.
        if conn_results
            .as_ref()
            .is_some_and(|c| c.iter().any(|r| r.error.is_some()))
        {
            has_error = true;
        }
    }

    for (name, result) in &results {
        if quiet {
            continue;
        }
        match result {
            FormalResult::Pass => println!("  ✓ PASS: {}", name),
            FormalResult::Counterexample(d) => {
                println!("  ✗ FAIL: {} — counterexample at depth {}", name, d)
            }
            FormalResult::Unknown => println!("  ? UNKNOWN: {}", name),
            FormalResult::Error(e) => println!("  ! ERROR: {} — {}", name, e),
            FormalResult::InductiveProof(k) => {
                println!(
                    "  ✓ INDUCTIVE PROOF (k={}): {} — holds for ALL depths",
                    k, name
                )
            }
        }
    }

    if !quiet {
        if results.is_empty() {
            println!("  (no assertions found)");
        }
        println!(
            "── End of Formal Results ({}/{} passed) ──\n",
            results
                .iter()
                .filter(|(_, r)| matches!(r, FormalResult::Pass))
                .count(),
            results.len()
        );
    }

    if has_error {
        return Err(SimError::with_diag(
            DiagCode::InternalError,
            "formal verification encountered errors",
        ));
    }
    if has_fail {
        return Err(SimError::with_diag(
            DiagCode::AssertionFailed,
            "formal verification FAILED — counterexample(s) found",
        ));
    }
    Ok(())
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

/// Baca byte sumber sebuah file. Untuk file `.mv` (F8) byte berasal dari
/// buffer hasil transpile on-the-fly; untuk file lain langsung dari disk.
/// Load library foreign dari bagian `[foreign]` file project .mivon
/// (arsitektur poin 9). Load VHPI/PLI/DPI; error → warning (bukan gagal
/// compile) agar project tetap bisa jalan tanpa library opsional.
fn load_project_foreign_libs(proj: &mivon_api::ProjectFile, cli: &Cli) {
    for lib_path in &proj.vhpi_libs {
        match mivon_api::vhpi::loader::load_vhpi_library(lib_path) {
            Ok(vhpi) => {
                if !cli.quiet {
                    println!(
                        "  [foreign] VHPI library loaded: {} (abi {:?})",
                        vhpi.path.display(),
                        vhpi.abi
                    );
                }
                if let Err(e) = mivon_api::vhpi::loader::call_vhpi_startup(&vhpi) {
                    eprintln!("warning: vhpi_startup '{}': {}", lib_path, e);
                }
            }
            Err(e) => eprintln!(
                "warning: [foreign] failed to load VHPI library '{}': {}",
                lib_path, e
            ),
        }
    }
    for lib_path in &proj.pli_libs {
        match mivon_api::pli::loader::load_pli_library(lib_path) {
            Ok(pli) => {
                if !cli.quiet {
                    println!(
                        "  [foreign] PLI library loaded: {} (abi {:?})",
                        pli.path.display(),
                        pli.abi
                    );
                }
                if let Err(e) = mivon_api::pli::loader::call_pli_startup(&pli) {
                    eprintln!("warning: vpi_startup (PLI) '{}': {}", lib_path, e);
                }
            }
            Err(e) => eprintln!(
                "warning: [foreign] failed to load PLI library '{}': {}",
                lib_path, e
            ),
        }
    }
    #[cfg(feature = "dpi")]
    for lib_path in &proj.dpi_libs {
        use mivon_api::simulator::dpi::DpiEngine;
        let mut eng = DpiEngine::new();
        match eng.load_library(lib_path) {
            Ok(_) => {
                if !cli.quiet {
                    println!("  [foreign] DPI library loaded: {}", lib_path);
                }
            }
            Err(e) => eprintln!(
                "warning: [foreign] failed to load DPI library '{}': {}",
                lib_path, e
            ),
        }
    }
}

fn read_source_bytes(
    path: &Path,
    inline: &std::collections::HashMap<PathBuf, Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    if let Some(bytes) = inline.get(path) {
        return Ok(bytes.clone());
    }
    std::fs::read(path)
}

fn run(cli: Cli, env: &mut mivon_api::env::GlobalEnv) -> Result<(), SimError> {
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
    // File `.mv` di-transpile (lex → parse → check → codegen) menjadi satu
    // buffer SV (svh + sv, baris `` `include `` di-strip) lalu disuntikkan
    // sebagai sumber inline; pipeline normal berjalan tanpa menyentuh disk.
    let mut inline_src: std::collections::HashMap<PathBuf, Vec<u8>> =
        std::collections::HashMap::new();
    let mv_files: Vec<String> = sources
        .iter()
        .filter(|s| Path::new(s).extension().map(|e| e == "mv").unwrap_or(false))
        .cloned()
        .collect();
    // ── F9: transpile SEMUA .mv sekaligus (konteks gabungan lintas file) ──
    // `types.mv` mendefinisikan package, `counter.mv` memakainya — keduanya
    // di-transpile bersama agar `use pkg::*` antar-file lolos type-check.
    if !mv_files.is_empty() {
        let mut items: Vec<(String, String)> = Vec::with_capacity(mv_files.len());
        for p in &mv_files {
            let base = Path::new(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("design")
                .to_string();
            let src = std::fs::read_to_string(p)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", p, e)))?;
            items.push((src, base));
        }
        let results = mivon_api::mv::transpile_many(&items).map_err(|(i, e)| {
            SimError::with_diag(
                DiagCode::InvalidSyntax,
                mivon_api::mv::format_error(&mv_files[i], &items[i].0, &e),
            )
        })?;
        // Defensif: hasil batch harus sejajar dengan input (jangan zip-truncate).
        assert_eq!(
            results.len(),
            mv_files.len(),
            "transpile_many harus mengembalikan hasil sejajar dengan input"
        );
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
            "[MV] transpiled {} .mv file(s) on-the-fly (F9)",
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
    if inline_src.is_empty()
        && !cli.print_tokens
        && !cli.print_ast
        && (cli.fast || cli.filelist.is_some() || total_src_bytes > 256 * 1024)
    {
        return run_fast(cli, None, env);
    }

    // ── Animasi pipeline (terminal EDA) ──
    let mut anim = PipelineAnimator::start(anim_enabled(&cli));

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
    let pid = mivon_compiler::micd::MicdDatabase::project_id(
        &proot,
        &src_paths,
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

    // Slot per source; diisi dari MICD bila cache valid (konten + include sama).
    let mut pp_combined: Vec<Option<Result<(String, Option<(String, String)>), String>>> =
        vec![None; sources.len()];
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
                    pp_combined[idx] = Some(Ok((entry.combined, entry.timescale)));
                    micd_reused += 1;
                    continue;
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
        anim_set_files(&anim, sources.len() as u64, 0);
    }
    let pp_start = std::time::Instant::now();
    let pp_for_parallel = &base_pp;
    let fresh_results: Vec<
        Result<(usize, String, Option<(String, String)>, Vec<PathBuf>), String>,
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
                        let combined_str = format!("`line 1 \"{}\"\n{}\n", path, processed);
                        Ok((*idx, combined_str, pp.timescale.clone(), Vec::new()))
                    }
                    Err(e) => Err(format!("preprocessor '{}': {}", path, e)),
                };
            }
            match pp.preprocess_file(path) {
                Ok(processed) => {
                    let combined_str = format!("`line 1 \"{}\"\n{}\n", path, processed);
                    let includes: Vec<PathBuf> = pp.resolved_includes.iter().cloned().collect();
                    Ok((*idx, combined_str, pp.timescale.clone(), includes))
                }
                Err(e) => Err(format!("preprocessor '{}': {}", path, e)),
            }
        })
        .collect();
    if !anim_active(&anim) {
        eprintln!("[TIMING] Preprocessing done in {:?}", pp_start.elapsed());
    }

    for r in &fresh_results {
        match r {
            Ok((idx, combined_str, ts, _includes)) => {
                pp_combined[*idx] = Some(Ok((combined_str.clone(), ts.clone())))
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
            Some(Ok((combined_str, ts))) => {
                if let Some(ts) = ts {
                    design_timescale = Some(ts.clone());
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
    for (idx, combined_str, ts, includes) in fresh_results.iter().flatten() {
        if let Ok(content) = read_source_bytes(Path::new(&sources[*idx]), &inline_src) {
            let h = mivon_compiler::cache::compute_checksum(&content);
            let path = std::path::PathBuf::from(&sources[*idx]);
            micd.cache_preprocessed(
                path.clone(),
                mivon_compiler::micd::PreprocEntry {
                    content_hash: h,
                    combined: combined_str.clone(),
                    timescale: ts.clone(),
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
    // ── MICD: cache lexer payload (tokens summary + stream) ──
    // Legacy path sebelumnya tidak menyimpan token data → lexer/ selalu 0.
    {
        use mivon_compiler::micd::cache::pipeline::{
            token_family, LexerPayload, LexerSummary, TokenRecord,
        };
        let mut summary = LexerSummary {
            token_count: tokens.len() as u64,
            identifiers: 0,
            numbers: 0,
            strings: 0,
            errors: 0,
            source_bytes: combined.len() as u64,
        };
        let mut records = Vec::with_capacity(tokens.len());
        for (tok, line, col) in &tokens {
            summary.observe(tok);
            records.push(TokenRecord {
                kind: token_family(tok),
                line: *line as u32,
                col: *col as u32,
            });
        }
        // Cache ke pipeline: kunci = combined source path (fallback: first source).
        let key = sources.first().map(|s| s.to_string()).unwrap_or_default();
        if let Some(layer) = micd.cache_layer.as_mut() {
            if let Ok(b) = bincode::serialize(&LexerPayload {
                summary,
                tokens: records,
            }) {
                let _ = layer.put(mivon_compiler::micd::CacheCategory::Lexer, &key, &b);
            }
        }
    }
    if !anim_active(&anim) {
        eprintln!(
            "[TIMING] Lexer done: {} tokens in {:?}",
            tokens.len(),
            lex_start.elapsed()
        );
    }
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
    let mut design = match parser.parse_design() {
        Ok(d) => {
            if anim_active(&anim) {
                anim_phase_done(&anim, Phase::Par);
                anim_set_modules(&anim, d.modules.len() as u64);
                anim_set_files(&anim, sources.len() as u64, sources.len() as u64);
            }
            if !anim_active(&anim) {
                eprintln!("[TIMING] Parser done in {:?}", parse_start.elapsed());
            }
            d
        }
        Err(e) => {
            // Emit hanya WARNING di sini; error dicetak SATU kali oleh
            // top-level handler (hindari duplikasi diagnostic line:col).
            if parser.errors.iter().any(|d| !d.is_error()) {
                let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
                for diag in &parser.errors {
                    if !diag.is_error() {
                        let _ = emitter.emit(diag);
                    }
                }
            }
            return Err(e);
        }
    };
    // Emit parser diagnostics (warnings) — errors ditangani top-level sekali.
    // In compile-only mode, individual construct errors are non-fatal.
    if parser.errors.iter().any(|d| !d.is_error()) {
        let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
        for diag in &parser.errors {
            if !diag.is_error() {
                let _ = emitter.emit(diag);
            }
        }
    }
    if parser.errors.iter().any(|d| d.is_error()) && !cli.compile_only {
        return Err(mivon_core::error::SimError::from_parse_diagnostic(
            parser.errors[0].clone(),
        ));
    }
    let ts_for_ir = design_timescale.clone();
    design.timescale = design_timescale;

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
                    Err(e) => eprintln!("warning: library file '{}' parse error: {}", libfile, e),
                }
            }
            Err(e) => eprintln!(
                "warning: library file '{}' preprocess error: {}",
                libfile, e
            ),
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
            let cli_defines: Vec<(String, String)> = cli
                .defines
                .iter()
                .map(|d| {
                    if let Some((k, v)) = d.split_once('=') {
                        (k.to_string(), v.to_string())
                    } else {
                        (d.clone(), String::new())
                    }
                })
                .collect();
            let mut combined_map = std::collections::HashMap::new();
            for (i, src) in sources.iter().enumerate() {
                if let Some(Ok((combined_str, _))) = &pp_combined[i] {
                    combined_map.insert(std::path::PathBuf::from(src), combined_str.clone());
                }
            }
            let include_deps = micd
                .files
                .iter()
                .filter(|(_, m)| !m.include_hashes.is_empty())
                .map(|(p, m)| {
                    (
                        p.clone(),
                        m.include_hashes.iter().map(|(d, _)| d.clone()).collect(),
                    )
                })
                .collect();
            let symbols: Vec<(String, String, std::path::PathBuf)> = syms
                .iter()
                .map(|(name, kind)| {
                    let file = def_file
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| fallback.clone());
                    (name.clone(), kind.clone(), file)
                })
                .collect();
            let type_entries: Vec<(String, u64)> = micd
                .type_index
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            let verify: Vec<mivon_compiler::micd::VerifyResult> =
                micd.verify.values().cloned().collect();
            let input = CachePopulateInput {
                designs: vec![(&fallback, &design)],
                combined: &combined_map,
                defines: &cli_defines,
                include_deps: &include_deps,
                include_hashes: &std::collections::HashMap::new(),
                lexer_payloads: vec![],
                symbols,
                type_entries,
                verify,
                module_file: def_file.clone(),
                profile: micd.stats_db.last().cloned(),
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
        anim_phase_running(&anim, Phase::Ela);
    }
    if !anim_active(&anim) {
        eprintln!("[TIMING] Starting elaboration...");
    }
    let elab_start = std::time::Instant::now();
    // ── Reuse IR cache (db.md "5. elaborate/"): seluruh source di-reuse dari
    // MICD preprocess cache (konten tidak berubah) → coba restore IR hasil
    // elaborasi sebelumnya, skip elaborator.─
    let mut restored_legacy_ir: Option<mivon_ir::IrDesign> = None;
    if !cli.recompile && micd_reused == sources.len() && !sources.is_empty() {
        let top = top_name.or_else(|| design.modules.first().map(|m| m.name.as_str()));
        if let Some(top) = top {
            restored_legacy_ir = micd.restore_elaborate_ir(top);
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
                        let w = diags
                            .iter()
                            .filter(|d| d.level == DiagLevel::Warning)
                            .count();
                        anim_abort(&mut anim, 1, w);
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
            if !anim_active(&anim) {
                eprintln!("[TIMING] Elaboration done in {:?}", elab_start.elapsed());
            }
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
                    format!("{}…", &msg[..77])
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

        let any_errors = has_elab_errors || recovered;
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
            println!("Top-level design tidak bisa ditentukan secara unik — simulasi & VCD dinonaktifkan.");
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

    ir_design.timescale = ts_for_ir;

    // ── Lapisan cache pipeline: isi kategori elaborate/ + generate/ dari IR
    // hasil elaborasi (db.md "5. elaborate/", "16. generate/") — dipanggil
    // di sini (setelah elaborasi sukses) karena populate di atas belum punya
    // IR. Design diambil dari elaborator (design asli sudah dipindah saat
    // konstruksi). Best-effort; store disimpan via layer.save(). ──
    if micd.cache_layer.is_some() {
        use mivon_compiler::micd::cache::pipeline::{CachePopulateInput, CachePopulator};
        let empty_combined: std::collections::HashMap<PathBuf, String> =
            std::collections::HashMap::new();
        let empty_deps: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
            std::collections::HashMap::new();
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
            combined: &empty_combined,
            defines: &[],
            include_deps: &empty_deps,
            include_hashes: &std::collections::HashMap::new(),
            lexer_payloads: vec![],
            symbols: vec![],
            type_entries: vec![],
            verify: vec![],
            module_file: std::collections::HashMap::new(),
            profile: None,
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
        // "5. elaborate/"). Best-effort.
        micd.store_elaborate_ir(&ir_design);
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
    engine.use_mir_jit = cli.jit_body;
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

    // Print signals
    if cli.print_state {
        println!("\n{}", debugger.print_all_signals());
    }
    for name in &cli.print_signal {
        println!("  {}", debugger.print_signal(name));
    }
    for name in &cli.timeline {
        println!("\n{}", debugger.timeline(name, cli.timeline_len));
    }
    if cli.mem.len() == 2 {
        if let (Ok(addr), Ok(len)) = (
            u64::from_str_radix(
                cli.mem[0].trim_start_matches("0x").trim_start_matches("0X"),
                16,
            ),
            cli.mem[1].parse::<usize>(),
        ) {
            println!("\n{}", debugger.memory_inspect(addr, len));
        }
    }

    if !cli.quiet {
        println!("VCD waveform written to '{}'", vcd_path);
    }

    // ── CSV close ──
    let _ = debugger.engine.close_csv();

    // ── Signal statistics ──
    if let Some(ref stats_path) = cli.signal_stats {
        let path = if stats_path.is_empty() {
            format!("{}.stats.txt", debugger.engine.design.top.name)
        } else {
            stats_path.clone()
        };
        if let Some(ref stats) = debugger.engine.signal_stats {
            if let Err(e) = stats.write_to_file(&path) {
                eprintln!("warning: signal stats write failed: {}", e);
            } else if !cli.quiet {
                println!("Signal statistics written to '{}'", path);
            }
        }
    }

    // ── GTKWave save file ──
    if let Some(ref gtkw_path) = cli.gtkw {
        let path = if gtkw_path.is_empty() {
            format!("{}.gtkw", debugger.engine.design.top.name)
        } else {
            gtkw_path.clone()
        };
        match mivon_api::waveform::save_gtkw(&path, &vcd_path, &debugger.engine.design) {
            Ok(()) => {
                if !cli.quiet {
                    println!("GTKWave save file written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: GTKW save failed: {}", e),
        }
    }

    // ── HTML waveform viewer ──
    if let Some(ref html_path) = cli.waveform_html_viewer {
        let csv_ref = cli.waveform_csv.as_deref().unwrap_or("output.csv");
        let path = if html_path.is_empty() {
            format!("{}.html", debugger.engine.design.top.name)
        } else {
            html_path.clone()
        };
        match mivon_api::waveform::save_html_viewer(&path, csv_ref, &debugger.engine.design) {
            Ok(()) => {
                if !cli.quiet {
                    println!("HTML waveform viewer written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: HTML viewer failed: {}", e),
        }
    }

    // UCIS coverage export
    if let Some(ref ucis_path) = cli.coverage_ucis {
        let path = if ucis_path.is_empty() {
            format!("{}.ucis.xml", debugger.engine.design.top.name)
        } else {
            ucis_path.clone()
        };
        match debugger.engine.export_coverage_ucis(&path) {
            Ok(()) => {
                if !cli.quiet {
                    println!("UCIS coverage written to '{}'", path);
                }
            }
            Err(e) => eprintln!("UCIS export failed: {}", e),
        }
    }

    // Signal history stats
    if !cli.quiet && cli.signal_history_spill.is_some() {
        let stats = debugger.engine.signal_history.stats();
        println!(
            "Signal history: {} mem entries, {} spilled to disk",
            stats.total_memory_entries, stats.total_spilled_entries
        );
    }

    // Save coverage database if requested
    if let Some(ref covdb_path) = cli.coverage_ucdb {
        let mut covdb = mivon_api::simulator::coverage_db::CoverageDatabase::with_path(covdb_path);
        covdb.merge_from_engine(&debugger.engine);
        if let Err(e) = covdb.save() {
            eprintln!("warning: coverage DB save failed: {}", e);
        } else if !cli.quiet {
            println!("Coverage database saved to '{}'", covdb_path);
        }
    }

    // Coverage threshold check (CI gate) — berlaku walau tanpa --coverage-ucdb,
    // sehingga config coverage.branch_threshold benar-benar dievaluasi.
    if let Some(threshold) = cli.coverage_threshold {
        check_coverage_threshold(&debugger.engine, threshold, cli.quiet)?;
    }

    // Export HTML coverage report
    if let Some(ref html_path) = cli.coverage_html {
        // String kosong → nama top (konsisten dengan UCIS).
        let path = if html_path.is_empty() {
            format!("{}.coverage.html", debugger.engine.design.top.name)
        } else {
            html_path.clone()
        };
        let mut covdb = mivon_api::simulator::coverage_db::CoverageDatabase::new();
        covdb.merge_from_engine(&debugger.engine);
        if let Err(e) = covdb.export_html(&path) {
            eprintln!("warning: HTML coverage report failed: {}", e);
        } else if !cli.quiet {
            println!("HTML coverage report written to '{}'", html_path);
        }
    }

    // Save checkpoint to file if requested
    if let Some(save_path) = &cli.save {
        let _ = debugger.engine.signal_history.flush();
        let path = std::path::Path::new(save_path);
        debugger.engine.save_checkpoint(path).map_err(|e| {
            SimError::with_diag(DiagCode::IoError, format!("checkpoint save failed: {}", e))
        })?;
        if !cli.quiet {
            println!("Checkpoint saved to '{}'", save_path);
        }
    }

    // CDC (Clock-Domain Crossing) analysis
    if let Some(ref cdc_path) = cli.cdc_report {
        if !cli.quiet {
            println!("Running CDC analysis...");
        }
        let cdc_analysis = mivon_api::scheduler::cdc::CdcAnalysis::analyze(&debugger.engine.design);

        // Print summary to console
        if !cli.quiet {
            println!(
                "CDC: {} crossings — {} unsynchronized, {} single-flop, {} OK (2+ flops)",
                cdc_analysis.total_crossings,
                cdc_analysis.unsynchronized_count,
                cdc_analysis.single_flop_count,
                cdc_analysis.sync_ok_count,
            );
            if cdc_analysis.unsynchronized_count > 0 {
                eprintln!(
                    "  ⚠️ {} unsynchronized crossing(s) detected — check CDC report for details",
                    cdc_analysis.unsynchronized_count
                );
            }
        }

        // Write detailed report
        let path = if cdc_path.is_empty() {
            format!(
                "{}_cdc_report.txt",
                debugger.engine.design.top.name.as_str()
            )
        } else {
            cdc_path.clone()
        };
        match cdc_analysis.write_report(&path) {
            Ok(()) => {
                if !cli.quiet {
                    println!("CDC report written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: CDC report write failed: {}", e),
        }
    }

    // F15: $fatal menghentikan sim dengan kegagalan → exit code non-zero.
    if debugger.engine.sev_fatal_count > 0 {
        return Err(SimError::with_diag(
            DiagCode::AssertionFailed,
            format!(
                "$fatal: simulasi dihentikan ({})",
                debugger.engine.sev_fatal_count
            ),
        ));
    }

    Ok(())
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

    // ── Animasi pipeline (terminal EDA) ──
    let mut anim = PipelineAnimator::start(anim_enabled(&cli));

    // ── MICD: persistent incremental compilation database ──
    // Otomatis (bukan flag tambahan): restore AST/combined-source file yang
    // tidak berubah → lex/parse di-skip. Saat --recompile, database tetap
    // dibuka (agar save menulis hasil full-rebuild) tapi restore di-skip.
    // MICD scoped per project (ProjectID): OpenTitan dan test/counter.sv
    // tidak pernah berbagi database → tidak ada kontaminasi lintas project.
    {
        let micd_root = micd_root_for(&cli);
        let proot = std::env::current_dir().unwrap_or_default();
        let pid = mivon_compiler::micd::MicdDatabase::project_id(
            &proot,
            &session.config.sources,
            &session.config.incdirs,
            &session.config.defines,
        );
        let mut db = mivon_compiler::micd::MicdDatabase::open_project_with_context(
            &micd_root,
            &pid,
            &proot,
            &session.config.sources,
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
    if anim_active(&anim) {
        anim_phase_running(&anim, Phase::Lex);
        anim_phase_running(&anim, Phase::Par);
        anim_set_files(&anim, session.config.sources.len() as u64, 0);
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
    if !session.parse_errors.is_empty() {
        emit_diags(&session.parse_errors);
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
        anim_phase_running(&anim, Phase::Ela);
    }
    // ── Reuse IR cache (db.md "5. elaborate/"): bila SELURUH file di-restore
    // dari MICD (tidak ada yang berubah) dan hasil elaborasi sebelumnya
    // tersimpan di cache, langsung pakai IR — elaborator di-skip penuh.
    // Hanya aman bila run sebelumnya elaborasi BERSIH (cache elaborate hanya
    // disimpan setelah gate error/recovery lolos di run itu).
    let mut restored_ir: Option<mivon_ir::IrDesign> = None;
    // `micd_restored_count()` di-reset `save_micd()`; set path tidak → pakai
    // `micd_restored_paths_count()` untuk deteksi warm run setelah save parse.
    if !cli.recompile
        && session.micd_restored_paths_count() == session.config.sources.len()
        && !session.config.sources.is_empty()
    {
        // Top eksplisit (`--top`) atau default (module pertama — sama dengan
        // pilihan elaborator). Cache IR disimpan per top (`ir:<top>`).
        let top = top_name.or_else(|| design.modules.first().map(|m| m.name.as_str()));
        if let Some(top) = top {
            restored_ir = session.restore_elaborate_ir(top);
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
                anim_finish(&mut anim, false, 0, 0);
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
                        let w = diags
                            .iter()
                            .filter(|d| d.level == DiagLevel::Warning)
                            .count();
                        anim_abort(&mut anim, 1, w);
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
                format!("{}…", &msg[..77])
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
        let any_ready = has_parse_errors
            || sem_errs > 0
            || hier_errs > 0
            || has_elab_errors
            || recovered
            || dpi_errs > 0;
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
    engine.use_cycle_fusion = cli.cycle_fusion;
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

    if cli.print_state {
        println!("\n{}", debugger.print_all_signals());
    }
    for name in &cli.print_signal {
        println!("  {}", debugger.print_signal(name));
    }
    for name in &cli.timeline {
        println!("\n{}", debugger.timeline(name, cli.timeline_len));
    }

    if !cli.quiet {
        println!("VCD waveform written to '{}'", vcd_path);
    }

    // ── CSV close ──
    let _ = debugger.engine.close_csv();

    // ── Signal statistics ──
    if let Some(ref stats_path) = cli.signal_stats {
        let path = if stats_path.is_empty() {
            format!("{}.stats.txt", debugger.engine.design.top.name)
        } else {
            stats_path.clone()
        };
        if let Some(ref stats) = debugger.engine.signal_stats {
            if let Err(e) = stats.write_to_file(&path) {
                eprintln!("warning: signal stats write failed: {}", e);
            } else if !cli.quiet {
                println!("Signal statistics written to '{}'", path);
            }
        }
    }

    // ── GTKWave save file ──
    if let Some(ref gtkw_path) = cli.gtkw {
        let path = if gtkw_path.is_empty() {
            format!("{}.gtkw", debugger.engine.design.top.name)
        } else {
            gtkw_path.clone()
        };
        match mivon_api::waveform::save_gtkw(&path, &vcd_path, &debugger.engine.design) {
            Ok(()) => {
                if !cli.quiet {
                    println!("GTKWave save file written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: GTKW save failed: {}", e),
        }
    }

    // ── HTML waveform viewer ──
    if let Some(ref html_path) = cli.waveform_html_viewer {
        let csv_ref = cli.waveform_csv.as_deref().unwrap_or("output.csv");
        let path = if html_path.is_empty() {
            format!("{}.html", debugger.engine.design.top.name)
        } else {
            html_path.clone()
        };
        match mivon_api::waveform::save_html_viewer(&path, csv_ref, &debugger.engine.design) {
            Ok(()) => {
                if !cli.quiet {
                    println!("HTML waveform viewer written to '{}'", path);
                }
            }
            Err(e) => eprintln!("warning: HTML viewer failed: {}", e),
        }
    }

    // Signal history stats
    if !cli.quiet && cli.signal_history_spill.is_some() {
        let stats = debugger.engine.signal_history.stats();
        println!(
            "Signal history: {} mem entries, {} spilled to disk",
            stats.total_memory_entries, stats.total_spilled_entries
        );
    }

    // Save coverage database if requested
    if let Some(ref covdb_path) = cli.coverage_ucdb {
        let mut covdb = mivon_api::simulator::coverage_db::CoverageDatabase::with_path(covdb_path);
        covdb.merge_from_engine(&debugger.engine);
        if let Err(e) = covdb.save() {
            eprintln!("warning: coverage DB save failed: {}", e);
        } else if !cli.quiet {
            println!("Coverage database saved to '{}'", covdb_path);
        }
    }

    // Coverage threshold check (CI gate) — berlaku walau tanpa --coverage-ucdb,
    // sehingga config coverage.branch_threshold benar-benar dievaluasi.
    if let Some(threshold) = cli.coverage_threshold {
        check_coverage_threshold(&debugger.engine, threshold, cli.quiet)?;
    }

    // Export HTML coverage report
    if let Some(ref html_path) = cli.coverage_html {
        // String kosong → nama top (konsisten dengan UCIS).
        let path = if html_path.is_empty() {
            format!("{}.coverage.html", debugger.engine.design.top.name)
        } else {
            html_path.clone()
        };
        let mut covdb = mivon_api::simulator::coverage_db::CoverageDatabase::new();
        covdb.merge_from_engine(&debugger.engine);
        if let Err(e) = covdb.export_html(&path) {
            eprintln!("warning: HTML coverage report failed: {}", e);
        } else if !cli.quiet {
            println!("HTML coverage report written to '{}'", html_path);
        }
    }

    // Save checkpoint to file if requested
    if let Some(save_path) = &cli.save {
        let _ = debugger.engine.signal_history.flush();
        let path = std::path::Path::new(save_path);
        debugger.engine.save_checkpoint(path).map_err(|e| {
            SimError::with_diag(DiagCode::IoError, format!("checkpoint save failed: {}", e))
        })?;
        if !cli.quiet {
            println!("Checkpoint saved to '{}'", save_path);
        }
    }

    // F15: $fatal menghentikan sim dengan kegagalan → exit code non-zero.
    if debugger.engine.sev_fatal_count > 0 {
        return Err(SimError::with_diag(
            DiagCode::AssertionFailed,
            format!(
                "$fatal: simulasi dihentikan ({})",
                debugger.engine.sev_fatal_count
            ),
        ));
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Subcommand tools (tools.md) — dispatch dari main()
// ═══════════════════════════════════════════════════════════════════════════

fn dispatch_inspect(a: &crate::cli::MinspectArgs) -> ! {
    // Subcommand output bisa di posisi pertama (tools.md: `minspect stats`)
    const CMDS: [&str; 9] = [
        "stats",
        "modules",
        "hierarchy",
        "packages",
        "classes",
        "interfaces",
        "parameters",
        "deps",
        "cache",
    ];
    let (command, targets): (Option<String>, Vec<String>) = match (
        a.targets.first().map(|s| s.as_str()),
        a.targets.last().map(|s| s.as_str()),
    ) {
        // Command di posisi pertama: `minspect stats rtl/`
        (Some(first), _) if CMDS.contains(&first) => {
            (Some(first.to_string()), a.targets[1..].to_vec())
        }
        // Command di posisi terakhir: `minspect rtl/ stats`
        (_, Some(last)) if CMDS.contains(&last) && a.targets.len() > 1 => {
            let mut t = a.targets.clone();
            t.pop();
            (Some(last.to_string()), t)
        }
        _ => (None, a.targets.clone()),
    };
    let args = mivon_api::tools::inspect::InspectArgs {
        targets: &targets,
        command,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        json: a.json,
    };
    exit_tool(mivon_api::tools::inspect::run(&args));
}

fn dispatch_lint(a: &crate::cli::MlintArgs) -> ! {
    let args = mivon_api::tools::lint::LintArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        all: a.all,
        unused: a.unused,
        width: a.width,
        latch: a.latch,
        loop_check: a.loop_check,
        fsm: a.fsm,
        case_analysis: a.case_analysis,
        clock_gating: a.clock_gating,
        power: a.power,
        memory: a.memory,
        gate_opt: a.gate_opt,
        quiet: a.quiet,
    };
    exit_tool(mivon_api::tools::lint::run(&args));
}

fn dispatch_elab(a: &crate::cli::MelabArgs) -> ! {
    let args = mivon_api::tools::elab::ElabArgs {
        files: &a.files,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        tree: a.tree,
        params: a.params,
        signals: a.signals,
        reset_domain: a.reset_domain,
        from_cache: a.from_cache,
    };
    exit_tool(mivon_api::tools::elab::run(&args));
}

fn dispatch_sim(a: &crate::cli::MsimArgs) -> ! {
    let args = mivon_api::tools::sim::SimArgs {
        files: &a.files,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        max_time: a.max_time.unwrap_or(DEFAULT_MAX_TIME_NS),
        output: a.output.as_deref(),
        fst: a.fst,
        assertions: a.assertions,
        coverage: a.coverage,
    };
    exit_tool(mivon_api::tools::sim::run(&args));
}

fn dispatch_cov(a: &crate::cli::McovArgs) -> ! {
    let args = mivon_api::tools::cov::CovArgs {
        files: &a.files,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        max_time: a.max_time.unwrap_or(DEFAULT_MAX_TIME_NS),
        output: a.output.as_deref(),
        json: a.json,
        html: a.html,
        threshold: a.threshold,
    };
    exit_tool(mivon_api::tools::cov::run(&args));
}

fn dispatch_wave(a: &crate::cli::MwaveArgs) -> ! {
    let args = match &a.cmd {
        crate::cli::MwaveCmd::Merge { inputs, output } => mivon_api::tools::wave::WaveArgs::Merge {
            inputs: inputs.clone(),
            output: output.clone(),
        },
        crate::cli::MwaveCmd::Export {
            input,
            format,
            output,
        } => mivon_api::tools::wave::WaveArgs::Export {
            input: input.clone(),
            format: format.clone(),
            output: output.clone(),
        },
        crate::cli::MwaveCmd::Filter {
            input,
            signals,
            output,
        } => mivon_api::tools::wave::WaveArgs::Filter {
            input: input.clone(),
            signals: signals.clone(),
            output: output.clone(),
        },
        crate::cli::MwaveCmd::Compare { a, b } => mivon_api::tools::wave::WaveArgs::Compare {
            a: a.clone(),
            b: b.clone(),
        },
        crate::cli::MwaveCmd::Search { input, patterns } => {
            mivon_api::tools::wave::WaveArgs::Search {
                input: input.clone(),
                patterns: patterns.clone(),
            }
        }
        crate::cli::MwaveCmd::Tree { input } => mivon_api::tools::wave::WaveArgs::Tree {
            input: input.clone(),
        },
        crate::cli::MwaveCmd::Stats { input } => mivon_api::tools::wave::WaveArgs::Stats {
            input: input.clone(),
        },
        crate::cli::MwaveCmd::Get {
            input,
            signals,
            at,
            range,
        } => mivon_api::tools::wave::WaveArgs::Get {
            input: input.clone(),
            signals: signals.clone(),
            at: *at,
            range: *range,
        },
        crate::cli::MwaveCmd::Decode { input, proto } => mivon_api::tools::wave::WaveArgs::Decode {
            input: input.clone(),
            proto: proto.clone(),
        },
    };
    exit_tool(mivon_api::tools::wave::run(&args));
}

fn dispatch_fmt(a: &crate::cli::MfmtArgs) -> ! {
    let args = mivon_api::tools::fmt::FmtArgs {
        files: &a.files,
        inplace: a.inplace,
        check: a.check,
        indent: a.indent,
    };
    exit_tool(mivon_api::tools::fmt::run(&args));
}

fn dispatch_gen(a: &crate::cli::MgenArgs) -> ! {
    let args = mivon_api::tools::gen::GenArgs {
        targets: &a.targets,
        output: a.output.clone(),
        stdout: a.stdout,
        check: a.check,
        svh_only: a.svh_only,
        sv_only: a.sv_only,
        no_check: a.no_check,
        verbose: a.verbose,
    };
    exit_tool(mivon_api::tools::gen::run(&args));
}

fn dispatch_prof(a: &crate::cli::MprofArgs) -> ! {
    let args = mivon_api::tools::prof::ProfArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        max_time: a.max_time.unwrap_or(DEFAULT_MAX_TIME_NS),
        cached: a.cached,
    };
    exit_tool(mivon_api::tools::prof::run(&args));
}

fn dispatch_check(a: &crate::cli::McheckArgs) -> ! {
    let args = mivon_api::tools::check::CheckArgs {
        targets: &a.targets,
        all: a.all,
        missing: a.missing,
        circular: a.circular,
        deps: a.deps,
        cycles: a.cycles,
        timescale: a.timescale,
        ast_diff: a.ast_diff.as_deref(),
        sv_version: a.sv_version,
    };
    exit_tool(mivon_api::tools::check::run(&args));
}

fn dispatch_bench(a: &crate::cli::MbenchArgs) -> ! {
    let args = mivon_api::tools::bench::BenchArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        runs: a.runs,
    };
    exit_tool(mivon_api::tools::bench::run(&args));
}

fn dispatch_synth(a: &crate::cli::SynthArgs) -> ! {
    let args = mivon_api::tools::synth::SynthArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        output: a.output.clone(),
        check_only: a.check_only,
        device: a.device.clone(),
        preset: a.preset.clone(),
        emit_mvnet: a.emit_mvnet,
        dump_sir: a.dump_sir,
        dump_sir_opt: a.dump_sir_opt,
        dump_netlist: a.dump_netlist,
        emit_netlist: a.emit_netlist,
        tech_map: a.tech_map,
        report_util: a.report_util.clone(),
        constraint: a.constraint.clone(),
        timing: a.timing,
        fsm_report: a.fsm_report,
        quiet: a.quiet,
    };
    exit_tool(mivon_api::tools::synth::run(&args));
}

fn dispatch_update(a: &crate::cli::UpdateArgs) -> ! {
    let check_only = matches!(a.cmd, Some(crate::cli::UpdateCmd::Check));
    let args = mivon_api::tools::update::UpdateArgs {
        check_only,
        channel: Some(a.channel.as_str()),
        version: a.version.as_deref(),
        rollback: a.rollback,
        yes: a.yes,
        manifest_url: a.manifest_url.as_deref(),
        exe_path: None,
    };
    exit_tool(mivon_api::tools::update::run(&args));
}

fn dispatch_emu(a: &crate::cli::EmuArgs) -> ! {
    use mivon_api::emu::config::EmuConfig;
    use mivon_api::emu::cpu::{CpuCore, RtlLinkedCpu};
    use mivon_api::emu::machine::Machine;
    use mivon_api::emu::mem::{MemoryMap, MemoryPort, RamRegion, RegionKind};
    use mivon_api::emu::mhir::types::AddressRegion;
    use mivon_core::intern::Symbol;
    use mivon_elaboration::elaborator::ElaborateMode;

    let result: Result<(), SimError> = (|| {
        // ── Konfigurasi emulator dari file TOML terpisah (`--config`, .meu) ──
        // BUKAN section di project .mivon — ekstensi .mivon dipakai MICD
        // dan file list; konfigurasi emulator hidup di file sendiri.
        let cfg: EmuConfig = match &a.config {
            Some(path) => EmuConfig::load_file(std::path::Path::new(path))
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?,
            None => EmuConfig::default(),
        };

        // ── Memory map (R1): config ram → region RAM host (mmap) ──
        // Dibuat lebih awal agar bisa dipakai jalur --boot-iso (x86) juga.
        let mut memmap = MemoryMap::new();
        if let Some(ram) = cfg.ram {
            let (base, size) = (ram.base, ram.size);
            let region = RamRegion::new(Symbol::intern("ram"), base, size, RegionKind::Ram, true)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
            memmap
                .add(region)
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
        }

        // ── Boot ISO x86 real-mode (R6): interpreter x86 + ISO sebagai disk.
        // Tanpa RTL — jalur boot BIOS: MBR → INT 13h → El Torito → GRUB. ──
        if let Some(iso_path) = &a.boot_iso {
            let mut out = String::new();
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--boot-iso butuh region RAM — definisikan ram = { base, size } di config .meu (mis. 0x0:0x100000)",
                ));
            }
            let bytes = {
                let mut f = std::fs::File::open(iso_path).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("{}: {}", iso_path, e))
                })?;
                // Jalur boot CD yang benar: El Torito boot catalog → boot image
                // (cdboot/GRUB) — BUKAN MBR. Booting MBR hybrid = jalur USB/HDD
                // yang berujung salah unit LBA saat GRUB baca filesystem CD.
                let eltorito = mivon_api::emu::iso::parse_eltorito(&mut f)
                    .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                if eltorito.entry.media_type != 0 {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!(
                            "boot image ISO media type {} bukan no-emulation (0)",
                            eltorito.entry.media_type
                        ),
                    ));
                }
                // no-emul: BIOS muat boot image (cdboot, ~512-2048 byte).
                mivon_api::emu::iso::read_boot_image(&mut f, &eltorito.entry, 0x10000)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
            };
            if bytes.len() < 512 {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "ISO terlalu kecil (min 512 byte)",
                ));
            }
            // Boot image CD (El Torito no-emul) ke 0x7c00, DL = 0xE0
            let mut cpu = mivon_api::emu::cpu::x86::X86Cpu::new();
            cpu.disk = Some(Box::new(
                mivon_api::emu::cpu::x86::FileDisk::open(iso_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?,
            ));
            cpu.load_boot_image(&mut memmap, &bytes, 0xE0)
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e.reason))?;
            let max_steps = a.max_steps.unwrap_or(50_000);
            let mut machine = Machine::new(Box::new(cpu), memmap, max_steps);
            emu_snapshot_preflight(a, &machine)?;
            if let Some(line) = emu_snapshot_load(a, &mut machine)? {
                out.push_str(&line);
            }
            let result = match machine.run() {
                Ok(result) => {
                    out.push_str(&format!("\n{}", result.summary()));
                    out.push_str(&format!(
                        "\nBIOS console: {}\n",
                        String::from_utf8_lossy(&result.console)
                    ));
                    result
                }
                Err(e) => {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("x86 fault: {:?}", e),
                    ))
                }
            };
            if let Some(line) = emu_snapshot_save(a, &machine)? {
                out.push_str(&line);
            }
            // ── Window display (opsional) ──
            if let Some(ref win_arg) = a.window {
                let cfg = mivon_api::emu::display::DisplayConfig::from_wh(win_arg)
                    .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                let mut disp = mivon_api::emu::display::VgaDisplay::new(cfg);
                disp.open_window()
                    .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                // Render console output ke VGA display
                let console_str = String::from_utf8_lossy(&result.console);
                let mut col = 0;
                let mut row = 0;
                for ch in console_str.bytes() {
                    match ch {
                        b'\n' => {
                            col = 0;
                            row += 1;
                            if row >= 25 {
                                disp.scroll_up();
                                row = 24;
                            }
                        }
                        b'\r' => col = 0,
                        b'\t' => col = (col + 8) & !7,
                        _ => {
                            if col < 80 && row < 25 {
                                disp.write_char(col, row, ch, 0x0A);
                                col += 1;
                            }
                        }
                    }
                }
                disp.update();
                // Tunggu user tekan key atau tutup jendela
                eprintln!("[Mivon] Window aktif. Tekan ESC atau tutup jendela untuk keluar.");
                loop {
                    if let Some(ch) = disp.poll_input() {
                        if ch == '\x1b' {
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                return Ok(());
            }
            print!("{}", out);
            return Ok(());
        }

        // Top untuk open_elaborated (targets): --top > config > --rtl-cpu-top
        // (saat mesin dijalankan dari RTL, top SoC sudah jelas dari flag CPU).
        let top = a
            .top
            .as_deref()
            .or(cfg.top.as_deref())
            .or(a.rtl_cpu_top.as_deref());
        // MHIR butuh RTL target (.sv/.v). Jalur interpreter (`--run` +
        // `--load-elf`) jalan TANPA file source → elaborasi dilewati bila
        // tak ada target (dump/`--addr` tetap butuh target → error jelas).
        let mhir = if a.targets.is_empty() {
            if a.dump_mhir || a.dump_memory_map || !a.addr.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--dump-mhir/--dump-memory-map/--addr butuh target RTL (.sv) — berikan \
                     file source (jalur interpreter cukup --config + --load-elf + --run)",
                ));
            }
            if !a.run && a.load_elf.is_none() && a.dump_memory.is_none() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "tidak ada yang dijalankan — berikan target RTL (.sv), atau \
                     --run --load-elf <elf> (interpreter RV32), atau --boot-iso <iso>",
                ));
            }
            None
        } else {
            let (_session, _design, ir) = mivon_api::tools::open_elaborated(
                &a.targets,
                &a.incdirs,
                &a.defines,
                top,
                ElaborateMode::StrictSimulation,
            )?;
            let mut m = mivon_api::emu::mhir::extract(&ir);

            // ── Address map: --addr + [emu] devices (Direct RTL Device) ──
            let mut entries: Vec<(Symbol, AddressRegion)> = Vec::new();
            for s in &a.addr {
                let Some((name, rest)) = s.split_once('=') else {
                    eprintln!("warning: --addr '{}': format NAME=BASE:SIZE", s);
                    continue;
                };
                let Some((base_s, size_s)) = rest.split_once(':') else {
                    eprintln!("warning: --addr '{}': format NAME=BASE:SIZE", s);
                    continue;
                };
                let parse_hex = |v: &str| u64::from_str_radix(v.trim_start_matches("0x"), 16);
                match (parse_hex(base_s), parse_hex(size_s)) {
                    (Ok(base), Ok(size)) => {
                        entries.push((Symbol::intern(name.trim()), AddressRegion { base, size }));
                    }
                    _ => eprintln!("warning: --addr '{}': base/size harus hex (0x...)", s),
                }
            }
            for d in &cfg.devices {
                if let (Some(base), Some(size)) = (d.mmio, d.size) {
                    let name = d.name.as_deref().unwrap_or("device");
                    entries.push((Symbol::intern(name), AddressRegion { base, size }));
                }
            }
            mivon_api::emu::mhir::extract::apply_address_map(&mut m, &entries);
            Some(m)
        };

        let mut out = String::new();
        if let Some(mhir) = &mhir {
            if a.dump_memory_map {
                out.push_str(&mivon_api::emu::dump::dump_memory_map(mhir));
                if !memmap.regions.is_empty() {
                    out.push_str("\nMemory regions (host):\n");
                    for r in &memmap.regions {
                        out.push_str(&format!(
                            "  0x{:08x}-0x{:08x}  {:<12} {} ({})\n",
                            r.base,
                            r.base + r.size - 1,
                            r.name.as_str(),
                            match r.kind {
                                RegionKind::Ram => "ram",
                                RegionKind::Rom => "rom",
                                RegionKind::Mmio => "mmio",
                            },
                            r.size
                        ));
                    }
                }
            }
            // Tanpa flag → default MHIR; `--dump-memory-map` saja → map saja.
            if a.dump_mhir || !a.dump_memory_map {
                out.push_str(&mivon_api::emu::dump::dump_mhir(mhir));
            }
        }

        // ── ELF loader (R1): muat kernel/bare-metal ke memory map ──
        // `elf_entry`/`elf_class` dipakai jalur `--run` interpreter (tanpa
        // `--rtl-cpu`): PC mulai dari entry, ISA mengikuti kelas ELF.
        let mut elf_entry: Option<u64> = None;
        let mut elf_class: Option<u8> = None;
        if let Some(path) = &a.load_elf {
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--load-elf butuh region RAM — definisikan [emu] ram = { base, size } di project file .mivon",
                ));
            }
            let bytes = std::fs::read(path)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", path, e)))?;
            let (hdr, _) = mivon_api::emu::elf::parse_elf(&bytes).map_err(|e| {
                SimError::with_diag(DiagCode::InvalidSyntax, format!("ELF '{}': {}", path, e))
            })?;
            let entry = mivon_api::emu::elf::load_elf(&bytes, &mut memmap).map_err(|e| {
                SimError::with_diag(DiagCode::InvalidSyntax, format!("ELF '{}': {}", path, e))
            })?;
            elf_entry = Some(entry);
            elf_class = Some(hdr.class);
            out.push_str(&format!(
                "\nELF loaded: '{}' entry=0x{:x} ({} region(s))\n",
                path,
                entry,
                memmap.regions.len()
            ));
        }

        // ── Direct RTL CPU (EMULATOR.md §7.2 mode 3): jalankan mesin dari
        // RTL .sv/.v user, BUKAN model software Rust. Rust hanya menyediakan
        // memori + orkestrasi bus; register file/ALU/control dieksekusi engine
        // RTL mivon (picorv32-style kontrak bus). ──

        let mem_final: MemoryMap = if !a.rtl_cpu.is_empty() {
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--rtl-cpu butuh region RAM — definisikan [emu] ram = { base, size } di file .meu",
                ));
            }
            let top = a.rtl_cpu_top.as_deref().unwrap_or("rv32_bus_wrapper");
            let mut cpu = RtlLinkedCpu::from_files(&a.rtl_cpu, top)
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
            cpu.reset();
            let max_steps = a.max_steps.unwrap_or(10_000);
            let mut machine = Machine::new(Box::new(cpu), memmap, max_steps);
            emu_snapshot_preflight(a, &machine)?;
            if let Some(line) = emu_snapshot_load(a, &mut machine)? {
                out.push_str(&line);
            }
            match machine.run() {
                Ok(result) => {
                    // ── Window display (opsional) ──
                    if let Some(ref win_arg) = a.window {
                        let cfg = mivon_api::emu::display::DisplayConfig::from_wh(win_arg)
                            .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                        let mut disp = mivon_api::emu::display::VgaDisplay::new(cfg);
                        disp.open_window()
                            .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                        // Render summary + console ke VGA display
                        let summary = result.summary();
                        let console_str = String::from_utf8_lossy(&result.console);
                        let full = format!("{}\n\n{}", summary, console_str);
                        let mut col = 0;
                        let mut row = 0;
                        for ch in full.bytes() {
                            match ch {
                                b'\n' => {
                                    col = 0;
                                    row += 1;
                                    if row >= 25 {
                                        disp.scroll_up();
                                        row = 24;
                                    }
                                }
                                b'\r' => col = 0,
                                b'\t' => col = (col + 8) & !7,
                                _ => {
                                    if col < 80 && row < 25 {
                                        disp.write_char(col, row, ch, 0x0A);
                                        col += 1;
                                    }
                                }
                            }
                        }
                        disp.update();
                        eprintln!(
                            "[Mivon] Window aktif. Tekan ESC atau tutup jendela untuk keluar."
                        );
                        loop {
                            if let Some(ch) = disp.poll_input() {
                                if ch == '\x1b' {
                                    break;
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        print!("{}", out);
                        return Ok(());
                    }
                    out.push_str(&format!("\n{}", result.summary()));
                }
                Err(e) => {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("RTL CPU fault: {:?}", e),
                    ))
                }
            }
            if let Some(line) = emu_snapshot_save(a, &machine)? {
                out.push_str(&line);
            }
            machine.mem
        } else if a.run {
            // ── Interpreter RISC-V32 (R1/R2): ELF jalan tanpa RTL ──
            // `--run` tanpa `--rtl-cpu` = CPU software (benar dulu untuk
            // bring-up/verifikasi — EMULATOR.md §7.2 mode Interpreter).
            let Some(entry) = elf_entry else {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--run tanpa --rtl-cpu butuh --load-elf <PATH> (interpreter RISC-V32) \
                     — atau jalankan CPU dari RTL dengan --rtl-cpu <file.sv>",
                ));
            };
            if elf_class == Some(2) {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "ELF64 (RV64): interpreter RV64 belum tersedia (fase R2) — pakai \
                     --rtl-cpu (Direct RTL CPU) atau tunggu JIT (R3)",
                ));
            }
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--run butuh region RAM — definisikan ram = { base, size } di config .meu",
                ));
            }
            let mut cpu = mivon_api::emu::cpu::Rv32Cpu::new();
            cpu.set_pc(entry);
            let max_steps = a.max_steps.unwrap_or(1_000_000);
            let mut machine = Machine::new(Box::new(cpu), memmap, max_steps);
            emu_snapshot_preflight(a, &machine)?;
            if let Some(line) = emu_snapshot_load(a, &mut machine)? {
                out.push_str(&line);
            }
            match machine.run() {
                Ok(result) => out.push_str(&format!("\n{}", result.summary())),
                Err(e) => {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("interpreter fault: {}", e),
                    ))
                }
            }
            if let Some(line) = emu_snapshot_save(a, &machine)? {
                out.push_str(&line);
            }
            machine.mem
        } else {
            memmap
        };

        // ── Hex dump memori guest ──
        if let Some(dm) = &a.dump_memory {
            let Some((addr_s, len_s)) = dm.split_once(':') else {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--dump-memory format ADDR:LEN (hex)",
                ));
            };
            let parse_hex = |v: &str| u64::from_str_radix(v.trim_start_matches("0x"), 16);
            let (Ok(addr), Some(len)) = (
                parse_hex(addr_s),
                parse_hex(len_s).ok().and_then(|l| usize::try_from(l).ok()),
            ) else {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--dump-memory: ADDR dan LEN harus hex",
                ));
            };
            out.push_str(&format!("\nMemory @0x{:x} ({} bytes):\n", addr, len));
            let mut cur = addr;
            let mut remain = len;
            while remain > 0 {
                let n = remain.min(16);
                let mut line = format!("0x{:08x}: ", cur);
                let mut hexs: Vec<String> = Vec::new();
                let mut ascii = String::new();
                for i in 0..n {
                    match mem_final.read(cur + i as u64, 1) {
                        Ok(b) => {
                            hexs.push(format!("{:02x}", b));
                            ascii.push(if (32..=126).contains(&(b as u8)) {
                                b as u8 as char
                            } else {
                                '.'
                            });
                        }
                        Err(_) => {
                            hexs.push("??".into());
                            ascii.push('?');
                        }
                    }
                }
                line.push_str(&hexs.join(" "));
                line.push_str(&format!("  {}", ascii));
                out.push_str(&line);
                out.push('\n');
                cur += n as u64;
                remain -= n;
            }
        }

        print!("{}", out);
        Ok(())
    })();
    exit_tool(result);
}

// ── Snapshot mesin (EMULATOR.md §14, fase R5) — helper CLI `mivon emu` ──

/// `--snapshot-save`: pastikan CPU mendukung snapshot SEBELUM run — gagal
/// cepat, jangan buang waktu run panjang lalu error di akhir.
fn emu_snapshot_preflight(
    a: &crate::cli::EmuArgs,
    machine: &mivon_api::emu::machine::Machine,
) -> Result<(), SimError> {
    if a.snapshot_save.is_some() {
        machine.cpu.snapshot().map_err(|e| {
            SimError::with_diag(DiagCode::InvalidSyntax, format!("--snapshot-save: {}", e))
        })?;
    }
    Ok(())
}

/// `--snapshot-load`: muat file + restore SEBELUM run. Kembalikan baris
/// output bila flag dipakai.
fn emu_snapshot_load(
    a: &crate::cli::EmuArgs,
    machine: &mut mivon_api::emu::machine::Machine,
) -> Result<Option<String>, SimError> {
    let Some(path) = a.snapshot_load.as_deref() else {
        return Ok(None);
    };
    let snap = mivon_api::emu::snapshot::MachineSnapshot::load_file(std::path::Path::new(path))
        .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
    machine.restore(&snap).map_err(|e| {
        SimError::with_diag(DiagCode::InvalidSyntax, format!("--snapshot-load: {}", e))
    })?;
    Ok(Some(format!(
        "\nsnapshot loaded: '{}' ({} instr / {} cycle kumulatif)\n",
        path, snap.steps, snap.cycles
    )))
}

/// `--snapshot-save`: simpan SETELAH run (CPU + memori sparse + counter).
fn emu_snapshot_save(
    a: &crate::cli::EmuArgs,
    machine: &mivon_api::emu::machine::Machine,
) -> Result<Option<String>, SimError> {
    let Some(path) = a.snapshot_save.as_deref() else {
        return Ok(None);
    };
    let snap = machine.snapshot().map_err(|e| {
        SimError::with_diag(DiagCode::InvalidSyntax, format!("--snapshot-save: {}", e))
    })?;
    let nbytes = snap.to_bytes().len();
    snap.save_file(std::path::Path::new(path))
        .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
    Ok(Some(format!(
        "\nsnapshot saved: '{}' ({} byte sparse, {} instr / {} cycle kumulatif)\n",
        path, nbytes, snap.steps, snap.cycles
    )))
}

/// Jalankan tool, cetak error via TerminalEmitter, exit dengan kode.
fn exit_tool(result: Result<(), SimError>) -> ! {
    if let Err(e) = result {
        let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
        let diag = e.to_diagnostic();
        let _ = emitter.emit(&diag);
        process::exit(e.exit_code());
    }
    process::exit(0);
}

// ══════════════════════════════════════════════════════════════════════
// Additional tools dispatch functions
// ══════════════════════════════════════════════════════════════════════

fn dispatch_batch(a: &crate::cli::MbatchArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        match &a.cmd {
            crate::cli::MbatchCmd::Run { config } => {
                use mivon_api::tools::batch::BatchConfig;
                let cfg = BatchConfig::from_file(std::path::Path::new(config))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let mut runner = cfg.to_runner();
                let results = runner.run();
                let summary = runner.summary();
                println!("{}", summary);
                for r in &results {
                    let status = match &r.status {
                        mivon_api::tools::batch::JobStatus::Completed => "✓ OK".to_string(),
                        mivon_api::tools::batch::JobStatus::Failed(e) => format!("✗ FAILED: {}", e),
                        mivon_api::tools::batch::JobStatus::Skipped => "⊘ SKIPPED".to_string(),
                        _ => "? PENDING".to_string(),
                    };
                    println!(
                        "  {}: {} ({:.2}s)",
                        r.name,
                        status,
                        r.duration.as_secs_f64()
                    );
                }
            }
            crate::cli::MbatchCmd::Status => {
                println!("Batch status: no active batch runs");
            }
            crate::cli::MbatchCmd::Summary => {
                println!("Batch summary: use 'mbatch run <config.toml>' to run");
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_memcheck(a: &crate::cli::MmemcheckArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        let binary = a.binary.clone();
        let args: Vec<String> = a.args.clone();
        let tool_result = match a.tool.as_str() {
            "valgrind" => mivon_api::tools::memcheck::run_valgrind(&binary, &args, &[])
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?,
            "heaptrack" => mivon_api::tools::memcheck::run_heaptrack(&binary, &args)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?,
            _ => {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    format!(
                        "tool '{}' tidak dikenal (gunakan 'valgrind' atau 'heaptrack')",
                        a.tool
                    ),
                ))
            }
        };
        println!("Tool: {}", tool_result.tool);
        println!("Exit code: {}", tool_result.exit_code);
        println!("Summary: {}", tool_result.summary);
        if tool_result.has_leaks() {
            println!("⚠ Memory leaks detected!");
        }
        if tool_result.has_errors() {
            println!("✗ {} errors found", tool_result.errors);
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_tbgen(a: &crate::cli::MtbgenArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::tbgen;
        let mut inputs: Vec<(&str, u32)> = Vec::new();
        let mut outputs: Vec<(&str, u32)> = Vec::new();
        if let Some(ref input_str) = a.inputs {
            for part in input_str.split(',') {
                if let Some((name, width)) = part.split_once('=') {
                    inputs.push((name.trim(), width.trim().parse().unwrap_or(1)));
                }
            }
        }
        if let Some(ref output_str) = a.outputs {
            for part in output_str.split(',') {
                if let Some((name, width)) = part.split_once('=') {
                    outputs.push((name.trim(), width.trim().parse().unwrap_or(1)));
                }
            }
        }
        let module_name = a.module.as_deref().unwrap_or("dut");
        let tb = tbgen::quick_tb(module_name, &inputs, &outputs);
        if let Some(ref output) = a.output {
            std::fs::write(output, &tb).map_err(|e| {
                SimError::with_diag(DiagCode::IoError, format!("{}: {}", output, e))
            })?;
            println!("Testbench written to {}", output);
        } else {
            print!("{}", tb);
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_waiver(a: &crate::cli::MwaiverArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::waiver::WaiverStore;
        let mut store = WaiverStore::new();
        let db_path = std::path::Path::new(".mivon/waivers.json");
        if db_path.exists() {
            store = WaiverStore::load(db_path)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
        }
        match &a.cmd {
            crate::cli::MwaiverCmd::Add {
                rule,
                file_pattern,
                reason,
                owner,
            } => {
                let id = store.add(rule, file_pattern.as_deref(), reason, owner);
                store
                    .save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Waiver {} added: {}", id, rule);
            }
            crate::cli::MwaiverCmd::List { rule, json } => {
                let waivers: Vec<_> = store
                    .waivers
                    .iter()
                    .filter(|w| rule.as_ref().map(|r| w.rule == *r).unwrap_or(true))
                    .collect();
                if *json {
                    let json_str = serde_json::to_string_pretty(&waivers)
                        .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
                    println!("{}", json_str);
                } else {
                    for w in &waivers {
                        println!("{}: {} ({})", w.id, w.rule, w.reason);
                    }
                }
            }
            crate::cli::MwaiverCmd::Check { rule, file } => {
                if let Some(m) = store.is_waived(rule, file.as_deref(), None) {
                    println!(
                        "✓ Waived: {} (confidence: {:.0}%)",
                        m.waiver.id,
                        m.confidence * 100.0
                    );
                } else {
                    println!("✗ Not waived: {}", rule);
                }
            }
            crate::cli::MwaiverCmd::Export { output } => {
                store
                    .save(std::path::Path::new(output))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Waivers exported to {}", output);
            }
            crate::cli::MwaiverCmd::Import { input } => {
                let imported = WaiverStore::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                for w in imported.waivers {
                    store.add(&w.rule, w.file_pattern.as_deref(), &w.reason, &w.owner);
                }
                store
                    .save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Waivers imported from {}", input);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_vault(a: &crate::cli::MvaultArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::vault::SecureVault;
        let vault = SecureVault::new();
        match &a.cmd {
            crate::cli::MvaultCmd::Register { file, user } => {
                let entry = vault
                    .register(std::path::Path::new(file), user)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!(
                    "Registered: {} (owner: {})",
                    entry.path, entry.permissions.owner
                );
            }
            crate::cli::MvaultCmd::Lock { file, user } => {
                vault
                    .lock(file, user)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Locked: {} by {}", file, user);
            }
            crate::cli::MvaultCmd::Unlock { file, user } => {
                vault
                    .unlock(file, user)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Unlocked: {} by {}", file, user);
            }
            crate::cli::MvaultCmd::Verify { file } => {
                let ok = vault
                    .verify(std::path::Path::new(file))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                if ok {
                    println!("✓ Integrity OK: {}", file);
                } else {
                    println!("✗ Integrity FAILED: {}", file);
                }
            }
            crate::cli::MvaultCmd::List => {
                let entries = vault.list();
                for e in &entries {
                    println!(
                        "{} (owner: {}, locked: {})",
                        e.path,
                        e.permissions.owner,
                        e.locked_by.as_deref().unwrap_or("no")
                    );
                }
            }
            crate::cli::MvaultCmd::Summary => {
                println!("{}", vault.summary());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_ipxact(a: &crate::cli::MipxactArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::ipxact::IpxactComponent;
        match &a.cmd {
            crate::cli::MipxactCmd::Generate {
                module,
                vendor,
                library,
                version,
                output,
                inputs,
                outputs,
            } => {
                let mut ports: Vec<(String, String, Option<u32>)> = Vec::new();
                if let Some(ref input_str) = inputs {
                    for part in input_str.split(',') {
                        if let Some((name, width)) = part.split_once('=') {
                            ports.push((
                                name.trim().to_string(),
                                "in".to_string(),
                                width.trim().parse().ok(),
                            ));
                        }
                    }
                }
                if let Some(ref output_str) = outputs {
                    for part in output_str.split(',') {
                        if let Some((name, width)) = part.split_once('=') {
                            ports.push((
                                name.trim().to_string(),
                                "out".to_string(),
                                width.trim().parse().ok(),
                            ));
                        }
                    }
                }
                let mut comp = IpxactComponent::from_module(module, &ports);
                comp.vendor = vendor.clone();
                comp.library = library.clone();
                comp.version = version.clone();
                let xml = comp.to_xml();
                if let Some(ref out) = output {
                    comp.save_xml(std::path::Path::new(out))
                        .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                    println!("IP-XACT written to {}", out);
                } else {
                    print!("{}", xml);
                }
            }
            crate::cli::MipxactCmd::Summary => {
                println!("IP-XACT: use 'mipxact generate' to create XML");
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_design_repo(a: &crate::cli::MdesignRepoArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::design_repo::{DesignFileInfo, DesignRepository};
        match &a.cmd {
            crate::cli::MdesignRepoCmd::Init { root } => {
                let _repo = DesignRepository::open(std::path::PathBuf::from(root));
                println!("Design repository initialized at {}", root);
            }
            crate::cli::MdesignRepoCmd::Commit {
                author,
                message,
                files,
            } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                let file_infos: Vec<DesignFileInfo> = files
                    .iter()
                    .map(|f| DesignFileInfo {
                        path: f.clone(),
                        checksum: "auto".to_string(),
                        size: 0,
                    })
                    .collect();
                let commit = repo.commit(author, message, file_infos);
                println!("Committed: {} by {}", commit.hash, commit.author);
            }
            crate::cli::MdesignRepoCmd::Log { max } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                let commits = repo.log(*max);
                for c in &commits {
                    println!("{}: {} ({})", &c.hash[..12], c.message, c.author);
                }
            }
            crate::cli::MdesignRepoCmd::Tag {
                name,
                commit,
                description,
            } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                let tag = repo
                    .tag(name, commit, description)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Tagged: {} -> {}", tag.name, tag.commit_hash);
            }
            crate::cli::MdesignRepoCmd::Diff { a, b } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                if let Some(diffs) = repo.diff(a, b) {
                    for d in &diffs {
                        println!("{}", d);
                    }
                } else {
                    println!("Commits not found");
                }
            }
            crate::cli::MdesignRepoCmd::Summary => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                println!("{}", repo.summary());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_project(a: &crate::cli::MprojectArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::project::{ProjectEntry, WorkspaceConfig};
        let config_path = std::path::Path::new(".mivon/workspace.toml");
        let mut config = if config_path.exists() {
            WorkspaceConfig::load(config_path)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
        } else {
            WorkspaceConfig::default_config()
        };
        match &a.cmd {
            crate::cli::MprojectCmd::Init { root } => {
                let _ = std::fs::create_dir_all(format!("{}/.mivon", root));
                config
                    .save(config_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Workspace initialized at {}", root);
            }
            crate::cli::MprojectCmd::Add {
                name,
                path,
                top,
                depends,
            } => {
                let deps: Vec<String> = depends
                    .as_ref()
                    .map(|d| d.split(',').map(|s| s.trim().to_string()).collect())
                    .unwrap_or_default();
                config.add_project(ProjectEntry {
                    name: name.clone(),
                    path: path.clone(),
                    top: top.clone(),
                    depends: deps,
                    incdirs: Vec::new(),
                    defines: Vec::new(),
                    features: Vec::new(),
                });
                config
                    .save(config_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Project '{}' added", name);
            }
            crate::cli::MprojectCmd::Remove { name } => {
                if config.remove_project(name) {
                    config
                        .save(config_path)
                        .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                    println!("Project '{}' removed", name);
                } else {
                    println!("Project '{}' not found", name);
                }
            }
            crate::cli::MprojectCmd::List => {
                for p in &config.projects {
                    println!(
                        "{}: {} (top: {})",
                        p.name,
                        p.path,
                        p.top.as_deref().unwrap_or("-")
                    );
                }
            }
            crate::cli::MprojectCmd::Analyze => {
                let analysis = config.analyze(std::path::Path::new("."));
                println!("Dependency order: {:?}", analysis.dependency_order);
                if !analysis.errors.is_empty() {
                    println!("Errors: {:?}", analysis.errors);
                }
            }
            crate::cli::MprojectCmd::Summary => {
                println!("Workspace: {} projects", config.projects.len());
                for p in &config.projects {
                    println!("  {} -> {}", p.name, p.path);
                }
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_sdc(a: &crate::cli::MsdcArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::sdc::SdcDocument;
        let doc = SdcDocument::load(std::path::Path::new(&a.file))
            .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
        if a.clocks_only {
            for c in doc.clocks() {
                println!("{:?}", c);
            }
        } else if a.json {
            let json = serde_json::to_string_pretty(&doc)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
            println!("{}", json);
        } else {
            println!("{}", doc.summary());
            for c in &doc.constraints {
                println!("  {:?}", c);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_equiv_check(a: &crate::cli::MequivCheckArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::equiv_check::EquivChecker;
        let checker = EquivChecker::new(&a.method);
        // Load golden and impl from JSON files
        let golden_content = std::fs::read_to_string(&a.golden)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", a.golden, e)))?;
        let impl_content = std::fs::read_to_string(&a.impl_file).map_err(|e| {
            SimError::with_diag(DiagCode::IoError, format!("{}: {}", a.impl_file, e))
        })?;
        let golden: Vec<(String, Vec<u64>)> = serde_json::from_str(&golden_content)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
        let impl_vals: Vec<(String, Vec<u64>)> = serde_json::from_str(&impl_content)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
        let mapping = Vec::new();
        let result = checker.check_combinational(&mapping, &golden, &impl_vals);
        if result.equivalent {
            println!(
                "✓ EQUIVALENT (method: {}, time: {}ms)",
                result.method, result.proof_time_ms
            );
        } else {
            println!(
                "✗ NOT EQUIVALENT (method: {}, time: {}ms)",
                result.method, result.proof_time_ms
            );
            if let Some(ce) = &result.counter_example {
                println!("  Counter-example at cycle {}", ce.cycle);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_regression(a: &crate::cli::MregressionArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::regression::{RegressionDb, RegressionRun, TestResult};
        use std::time::{SystemTime, UNIX_EPOCH};
        let db_path = std::path::Path::new(".mivon/regression.json");
        let mut db = if db_path.exists() {
            RegressionDb::load(db_path).map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
        } else {
            RegressionDb::new()
        };
        match &a.cmd {
            crate::cli::MregressionCmd::Record {
                input,
                branch,
                commit,
            } => {
                let content = std::fs::read_to_string(input).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("{}: {}", input, e))
                })?;
                let results: Vec<TestResult> = serde_json::from_str(&content)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let run = RegressionRun {
                    id: format!("run-{}", ts),
                    timestamp: ts,
                    branch: branch.clone(),
                    commit: commit.clone(),
                    results,
                    total_duration_ms: 0,
                };
                db.record(run);
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Regression run recorded");
            }
            crate::cli::MregressionCmd::Summary => {
                println!("{}", db.summary());
            }
            crate::cli::MregressionCmd::Flaky => {
                let flaky = db.flaky_tests();
                if flaky.is_empty() {
                    println!("No flaky tests detected");
                } else {
                    for (name, rate) in &flaky {
                        println!("  {}: {:.0}% pass rate", name, rate * 100.0);
                    }
                }
            }
            crate::cli::MregressionCmd::Trend => {
                let trend = db.trend();
                println!("Trend: {:?}", trend);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_eco(a: &crate::cli::MecoArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::eco::{EcoDb, EcoSeverity, EcoStatus};
        let db_path = std::path::Path::new(".mivon/eco.json");
        let mut db = if db_path.exists() {
            EcoDb::load(db_path).map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
        } else {
            EcoDb::new()
        };
        match &a.cmd {
            crate::cli::MecoCmd::Create {
                title,
                description,
                severity,
                author,
            } => {
                let sev = match severity.as_str() {
                    "critical" => EcoSeverity::Critical,
                    "major" => EcoSeverity::Major,
                    "minor" => EcoSeverity::Minor,
                    "cosmetic" => EcoSeverity::Cosmetic,
                    _ => {
                        return Err(SimError::with_diag(
                            DiagCode::InvalidSyntax,
                            format!(
                                "severity '{}' tidak dikenal (critical/major/minor/cosmetic)",
                                severity
                            ),
                        ))
                    }
                };
                let id = db.create(title, description, sev, author);
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("ECO created: {}", id);
            }
            crate::cli::MecoCmd::List { status, severity } => {
                for e in &db.entries {
                    if let Some(s) = status {
                        if format!("{:?}", e.status).to_lowercase() != s.to_lowercase() {
                            continue;
                        }
                    }
                    if let Some(s) = severity {
                        if format!("{:?}", e.severity).to_lowercase() != s.to_lowercase() {
                            continue;
                        }
                    }
                    println!("{}: {} [{:?}] ({:?})", e.id, e.title, e.severity, e.status);
                }
            }
            crate::cli::MecoCmd::Transition { id, new_status } => {
                let status = match new_status.as_str() {
                    "draft" => EcoStatus::Draft,
                    "submitted" => EcoStatus::Submitted,
                    "reviewed" => EcoStatus::Reviewed,
                    "approved" => EcoStatus::Approved,
                    "implemented" => EcoStatus::Implemented,
                    "verified" => EcoStatus::Verified,
                    "closed" => EcoStatus::Closed,
                    "rejected" => EcoStatus::Rejected,
                    _ => {
                        return Err(SimError::with_diag(
                            DiagCode::InvalidSyntax,
                            format!("status '{}' tidak dikenal", new_status),
                        ))
                    }
                };
                db.transition(id, status)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("ECO {} transitioned to {}", id, new_status);
            }
            crate::cli::MecoCmd::Comment { id, author, text } => {
                db.comment(id, author, text)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Comment added to {}", id);
            }
            crate::cli::MecoCmd::Summary => {
                println!("{}", db.summary());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

fn dispatch_cov_closure(a: &crate::cli::McovClosureArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::cov_closure::CoverageClosure;
        match &a.cmd {
            crate::cli::McovClosureCmd::Analyze { input } => {
                let cc = CoverageClosure::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("{}", cc.summary());
                println!("\nCoverage by type:");
                for (t, pct) in cc.coverage_by_type() {
                    println!("  {}: {:.1}%", t, pct);
                }
            }
            crate::cli::McovClosureCmd::Critical { input } => {
                let cc = CoverageClosure::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let critical = cc.critical_tests();
                if critical.is_empty() {
                    println!("No critical tests");
                } else {
                    for tc in &critical {
                        println!("  {} (unique: {})", tc.test_name, tc.unique_points.len());
                    }
                }
            }
            crate::cli::McovClosureCmd::Uncovered { input } => {
                let cc = CoverageClosure::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let uncovered = cc.find_uncovered();
                if uncovered.is_empty() {
                    println!("All points covered!");
                } else {
                    for p in &uncovered {
                        println!("  {} ({}:{})", p.id, p.file, p.line);
                    }
                }
            }
        }
        Ok(())
    })();
    exit_tool(result);
}
