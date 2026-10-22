//! Pipeline cepat `run_fast()` — CompileSession + FastLexer + MICD penuh
//! (restore AST/incremental) + reuse IR elaborasi. Dipindah verbatim dari
//! `main.rs` monolitik; 1 file = 1 tanggung jawab (jalur cepat saja).
//! Jalur legacy (`run`) ada di `run_legacy.rs`.

use std::path::PathBuf;

use crate::anim_glue::{
    anim_abort, anim_active, anim_enabled, anim_finish, anim_phase_done, anim_phase_running,
    anim_set_exts, anim_set_files, anim_set_modules, anim_set_top, emit_diags_anim,
};
use crate::cli::Cli;
#[cfg(feature = "formal")]
use crate::formal_run::run_formal;
use crate::pipeline_config::{micd_root_for, pick_elab_mode};
use crate::sim_finish;
use crate::source_io::load_project_foreign_libs;
use crate::{
    DEFAULT_MAX_TIME_NS, apply_engine_flags, emit_diags, micd_save_and_print, print_ir_mem_stats,
    trim_heap, truncate_ellipsis,
};

use mivon_api::SessionConfig;
use mivon_api::debugger::Debugger;
use mivon_api::read_project_file;
use mivon_api::simulator::Breakpoint;
use mivon_api::simulator::DebugMode;
use mivon_api::simulator::SimulationEngine;
use mivon_api::simulator::Watchpoint;
use mivon_api::waveform::VcdWriter;
use mivon_compiler::frontend::CompileSession;
use mivon_core::animasi::{Phase, PipelineAnimator};
use mivon_core::diagnostics::DiagCode;
use mivon_core::diagnostics::DiagLevel;
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::Elaborator;
use mivon_ir::LogicVec;

/// Run compilation + simulation using the new parallel pipeline (CompileSession + FastLexer).
pub(crate) fn run_fast(
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
    use super::*;

    /// Regresi wiring pemisahan run_fast(): fn `run_fast` tetap bisa dipanggil
    /// dari jalur legacy (run_legacy::run → super::run_fast) dengan tanda
    /// tangan sama — antarmuka antar-jalur dijaga setelah pemecahan.
    #[test]
    fn run_fast_signature_unchanged() {
        let _f: fn(
            Cli,
            Option<(String, String)>,
            &mut mivon_api::env::GlobalEnv,
        ) -> Result<(), SimError> = self::run_fast;
        assert!(crate::DEFAULT_MAX_TIME_NS > 0);
    }
}
