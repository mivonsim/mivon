//! Oracle bug detection (O1-O5 style, diadaptasi untuk mivon).
#![allow(clippy::result_large_err)] // SimError besar (diagnostik) — fuzz sengaja.

use crate::{CaseResult, Category, Oracle, Target};

use mivon_api::{
    simulate_signals_with_flags_quiet, simulate_signals_with_trace_quiet, EngineFlags,
};

/// Evaluasi satu source terhadap target.
///
/// Pendekatan: in-process melalui mivon-api (bukan subprocess), karena API
/// sudah menyediakan jalur quiet + differential. Panic/hang tetap ditangkap
/// via runner subprocess untuk target yang butuh izin eksekusi penuh.
pub fn evaluate(target: Target, source: &str, timeout_ms: u64) -> CaseResult {
    match target {
        Target::Lexer => evaluate_lexer(source, timeout_ms),
        Target::Vcd => evaluate_vcd(source, timeout_ms),
        Target::Parser | Target::Elaborator => evaluate_compile(target, source, timeout_ms),
        Target::Simulator => evaluate_sim(source, timeout_ms),
        Target::Fmt => evaluate_fmt(source, timeout_ms),
        Target::Cli => evaluate_cli(source, timeout_ms),
        Target::Preproc => evaluate_preproc(source, timeout_ms),
        Target::Mv => evaluate_mv(source, timeout_ms),
        Target::Sdf => evaluate_sdf(source, timeout_ms),
        Target::Micd => evaluate_micd(source, timeout_ms),
        Target::Synth => evaluate_synth(source, timeout_ms),
        Target::Astdiff => evaluate_astdiff(source, timeout_ms),
        Target::Judge => evaluate_judge(source, timeout_ms),
        Target::All => unreachable!("Target::All dipecah di run()"),
    }
}

fn mk(
    target: Target,
    oracle: Oracle,
    category: Category,
    detail: &str,
    source: &str,
) -> CaseResult {
    CaseResult {
        target,
        category,
        oracle: oracle.as_str(),
        detail: detail.to_string(),
        source: source.to_string(),
    }
}

/// Isolasi MICD DB per-case: `MIVON_MICD_DIR` → temp unik. Subprocess mivon
/// (sim/sdf/cli/vcd/micd) TIDAK menyentuh `.mivon/database` project →
/// bebas lock contention + stale-lock cascade dari subprocess yang di-kill
/// (terukur: 39 lock basi + hang palsu beruntun di kampanye SDF).
///
/// Lewat thread-local + `Command::env` (runner::set_micd_override), BUKAN
/// `std::env::set_var` process-global — worker watchdog yang dibiarkan hidup
/// setelah timeout masih menjalankan subprocess dgn env case lamanya saat
/// case berikut ganti env = data race antar thread (review sesi ini).
/// Dipakai juga oleh `judge::judge_single` (subprocess runner di sana).
pub(crate) fn with_micd_isolated<T>(f: impl FnOnce() -> T) -> T {
    let dir = std::env::temp_dir().join(format!(
        "mivonfz_iso_{}_{}",
        std::process::id(),
        crate::next_crash_seq()
    ));
    let _ = std::fs::create_dir_all(&dir);
    crate::runner::set_micd_override(Some(&dir));
    let r = f();
    crate::runner::set_micd_override(None);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

/// Error GLOBAL tanpa lokasi tunggal (by design) — bukan diag_missing.
fn is_global_error(msg: &str) -> bool {
    let pats = [
        "Unable to determine top-level design",
        "Top resolution failed",
        "no modules found",
        "No modules found",
        "no valid top",
        "Recovery mode enabled",
    ];
    pats.iter().any(|p| msg.contains(p))
}

/// Watchdog seragam untuk jalur evaluasi IN-PROCESS (lexer/preproc/mv/fmt/
/// sim): kerja dijalankan di thread stack besar 256MB (parser/evaluator
/// rekursif bisa overflow stack 8MB pada hasil mutasi — temuan fuzzer),
/// dibatasi `timeout_ms` lewat `recv_timeout`.
///
/// Lewat batas → `Category::Hang` dan kampanye LANJUT; worker dibiarkan
/// selesai di background (thread Rust tidak bisa di-kill). Tanpa watchdog,
/// satu loop tak-berujung di lexer/preproc/transpile menghentikan kampanye
/// selamanya tanpa baris progres — progres kampanye sempat hilang total dan
/// hanya ketahuan dari CPU 100% satu thread.
fn run_with_watchdog<F>(
    target: Target,
    source: &str,
    timeout_ms: u64,
    name: &'static str,
    f: F,
) -> CaseResult
where
    F: FnOnce(&str) -> CaseResult + Send + 'static,
{
    use std::time::Duration;
    let source_owned = source.to_string();
    let (tx, rx) = std::sync::mpsc::channel::<CaseResult>();
    let spawned = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .name(name.into())
        .spawn(move || {
            let r = f(&source_owned);
            let _ = tx.send(r);
        });
    if spawned.is_err() {
        return mk(
            target,
            Oracle::O1NoCrash,
            Category::Panic,
            "thread watchdog gagal spawn",
            source,
        );
    }
    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Grace 2×: kasus LAMBAT (seed besar + beban mesin) dulu langsung
            // → Hang → dihitung is_bug() → noise (terbukti: semua kasus
            // "hang" replay-ok saat mesin sepi). Worker yang selesai dalam
            // grace = Slow (bukan bug); tetap tak selesai = hang sungguhan.
            match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
                Ok(r) if r.category == Category::Ok => mk(
                    target,
                    Oracle::O1NoCrash,
                    Category::Slow,
                    &format!(
                        "lewat budget {} ms tapi selesai saat grace 2× — lambat, bukan hang",
                        timeout_ms
                    ),
                    source,
                ),
                Ok(r) => r,
                Err(_) => mk(
                    target,
                    Oracle::O1NoCrash,
                    Category::Hang,
                    &format!(
                        "hang > {} ms (grace 2× habis; worker dilanjutkan di background)",
                        timeout_ms.saturating_mul(2)
                    ),
                    source,
                ),
            }
        }
        Err(_) => mk(
            target,
            Oracle::O1NoCrash,
            Category::Panic,
            "thread watchdog disconnected",
            source,
        ),
    }
}

/// O1 + O2 untuk compile pipeline (lexer/parser/elaborator) + deteksi HANG.
///
/// Dijalankan dalam thread stack BESAR (256MB) — parser rekursif pada
/// expression dalam hasil mutasi (contoh: `x0|x1|...` rantai panjang) bisa
/// overflow stack default 8MB (ditemukan fuzzer seed 7: stack overflow di
/// parser). Konsisten dgn simulate_in_thread.
///
/// FILEZERO (fuzzer gap): sebelumnya TANPA watchdog — input blowup
/// (mutasi duplicate chunk → seed 13MB, 537 module) membuat compile
/// super-lambat/freeze dan kampanye macet total tanpa deteksi. Sekarang
/// watchdog `recv_timeout` → Hang terdeteksi, worker thread dibiarkan
/// selesai di background (tidak bisa di-kill di Rust) — kampanye lanjut.
fn evaluate_compile(target: Target, source: &str, timeout_ms: u64) -> CaseResult {
    // Kini lewat run_with_watchdog (dulu salinan sendiri tanpa grace):
    // kasus compile lambat (seed 360KB = 4.4s terukur, >5s saat beban mesin)
    // dulu → Hang = noise (parser hang=7 / elab=6 per 3000 case, semua
    // replay-ok). Grace 2× → Slow; hang sungguhan tetap tertangkap.
    run_with_watchdog(
        target,
        source,
        timeout_ms,
        "mivon-fuzz-compile",
        move |src| compile_in_thread(target, src),
    )
}

fn compile_in_thread(target: Target, source: &str) -> CaseResult {
    // O1: no-crash — panic di compile = bug
    let caught = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source));
    let ir_design = match caught {
        Ok(Ok(ir)) => ir,
        Ok(Err(err)) => {
            // Error bersih — O2: cek lokasi diagnostik.
            // LANGKAH ANALISA: error top-level/multi-modul tanpa top (E3006/
            // EL3001) TIDAK = source salah — seed RTL murni valid. Coba mode
            // recovery (AnalysisRecovery): kalau berhasil → Ok (analisis).
            let msg = err.to_string();
            if is_global_error(&msg) {
                let recovered = std::panic::catch_unwind(|| mivon_api::compile_str_analyze(source));
                if let Ok(Ok(_ir)) = recovered {
                    return mk(
                        target,
                        Oracle::O1NoCrash,
                        Category::Ok,
                        &format!("sim ok — analysis recovery (multi-modul tanpa top): {msg}"),
                        source,
                    );
                }
                return mk(
                    target,
                    Oracle::O1NoCrash,
                    Category::CleanError,
                    &msg,
                    source,
                );
            }
            let has_loc = extract_loc(&msg);
            if !has_loc {
                return mk(
                    target,
                    Oracle::O2DiagLocation,
                    Category::DiagMissing,
                    &format!("diagnostik tanpa lokasi: {msg}"),
                    source,
                );
            }
            return mk(
                target,
                Oracle::O1NoCrash,
                Category::CleanError,
                &msg,
                source,
            );
        }
        Err(_) => {
            return mk(
                target,
                Oracle::O1NoCrash,
                Category::Panic,
                "panic saat compile_str_quiet",
                source,
            );
        }
    };

    // Compile sukses — determinism check (O4): kompilasi ulang sama
    let second = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source));
    match second {
        Ok(Ok(ir2)) => {
            let diffs = mivon_api::compare_asts(&ir_design, &ir2);
            if !diffs.is_empty() {
                return mk(
                    target,
                    Oracle::O4Determinism,
                    Category::NonDeterministic,
                    &format!("AST berbeda antar run ({} diffs)", diffs.len()),
                    source,
                );
            }
            mk(
                target,
                Oracle::O1NoCrash,
                Category::Ok,
                "compile ok",
                source,
            )
        }
        _ => mk(
            target,
            Oracle::O4Determinism,
            Category::NonDeterministic,
            "compile kedua panic/error — non-deterministik",
            source,
        ),
    }
}

/// Fuzzing LEXER khusus (area belum tersentuh — target Lexer lama memakai
/// pipeline compile yang sama, bukan oracle token):
/// - lex dua kali segar → stream token (kind+line+col) harus identik
///   (non-determinisme token = bug)
/// - panic saat lex input gila = bug
fn evaluate_lexer(source: &str, timeout_ms: u64) -> CaseResult {
    // Watchdog: lexer infinite loop pada input mutasi = kampanye macet
    // total (tanpa baris progres, CPU 100%) — lex tetap di thread besar.
    run_with_watchdog(
        Target::Lexer,
        source,
        timeout_ms,
        "mivon-fuzz-lexer",
        lex_eval,
    )
}

fn lex_eval(source: &str) -> CaseResult {
    use mivon_parser::lexer::Lexer;
    let lex_once = || -> Vec<(String, usize, usize)> {
        let mut lx = Lexer::new(source);
        let mut toks = Vec::new();
        loop {
            let (tok, line, col) = lx.next_token();
            if tok == mivon_parser::lexer::Token::Eof {
                break;
            }
            toks.push((format!("{:?}", tok), line, col));
        }
        toks
    };
    let r1 = std::panic::catch_unwind(lex_once);
    let r2 = std::panic::catch_unwind(lex_once);
    match (r1, r2) {
        (Ok(t1), Ok(t2)) => {
            if t1 != t2 {
                return mk(
                    Target::Lexer,
                    Oracle::O4Determinism,
                    Category::NonDeterministic,
                    &format!(
                        "lexer non-deterministik: dua lex identik hasil beda ({} vs {} tokens)",
                        t1.len(),
                        t2.len()
                    ),
                    source,
                );
            }
            mk(
                Target::Lexer,
                Oracle::O1NoCrash,
                Category::Ok,
                &format!("lexer ok ({} token)", t1.len()),
                source,
            )
        }
        (Err(_), _) | (_, Err(_)) => mk(
            Target::Lexer,
            Oracle::O1NoCrash,
            Category::Panic,
            "panic saat lex",
            source,
        ),
    }
}

/// O1 + O4 + O5 untuk simulator — verifikasi lengkap, bukan sekadar jalan.
///
/// Alur verifikasi:
/// 1. Subprocess check (O1): crash/abort/hang via binary mivon black-box.
/// 2. In-process compile (O1): pastikan source compile tanpa panic.
/// 3. O4 determinism: jalankan 2× dengan flags sama → signal harus identik.
/// 4. O5 differential: jalankan default vs semua jalur alternatif → signal harus identik.
/// 5. Trace consistency: final state dan trace harus berhasil diambil serta konsisten.
/// 6. Assertion detection: deteksi assertion/violation di output.
fn evaluate_sim(source: &str, timeout_ms: u64) -> CaseResult {
    // ── 1. Subprocess check (O1) ──
    let outcome = with_micd_isolated(|| crate::runner::run_file(source, timeout_ms));
    match outcome.kind {
        crate::runner::Kind::CleanError => {
            return mk(
                Target::Simulator,
                Oracle::O1NoCrash,
                Category::CleanError,
                &outcome.stderr,
                source,
            );
        }
        crate::runner::Kind::Panic => {
            return mk(
                Target::Simulator,
                Oracle::O1NoCrash,
                Category::Panic,
                &outcome.stderr,
                source,
            );
        }
        crate::runner::Kind::Abort => {
            return mk(
                Target::Simulator,
                Oracle::O1NoCrash,
                Category::Abort,
                &outcome.stderr,
                source,
            );
        }
        crate::runner::Kind::Crash(code) => {
            return mk(
                Target::Simulator,
                Oracle::O1NoCrash,
                Category::Panic,
                &format!("crash code {code}: {}", outcome.stderr),
                source,
            );
        }
        crate::runner::Kind::Hang => {
            return mk(
                Target::Simulator,
                Oracle::O1NoCrash,
                Category::Hang,
                &format!("hang > {} ms", timeout_ms),
                source,
            );
        }
        crate::runner::Kind::Ok => {
            // Subprocess ok — deteksi assertion/violation di output
            if let Some(violation) = detect_violation(&outcome.stderr) {
                return mk(
                    Target::Simulator,
                    Oracle::O1NoCrash,
                    Category::GuardBypass,
                    &violation,
                    source,
                );
            }
            if let Some(violation) = detect_violation(&outcome.stdout) {
                return mk(
                    Target::Simulator,
                    Oracle::O1NoCrash,
                    Category::GuardBypass,
                    &violation,
                    source,
                );
            }
            // O6: bug menyamar sbg warning/error (HiddenBug = pola mustahil
            // by-design → dianggap bug; Degraded = mivon menyerah diam →
            // dihitung & tampil summary). Tanpa scan ini, degradasi senyap
            // tergolong Ok (kasus WR0102 menyembunyikan bug lebar dsb).
            let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
            if let Some((cat, detail)) = scan_hidden_diags(&combined) {
                return mk(Target::Simulator, Oracle::O1NoCrash, cat, &detail, source);
            }
        }
    }

    // ── 2-6. In-process compile/determinism/differential/trace/evidence
    //         dijalankan dalam thread stack BESAR (256MB, konsisten dgn
    //         main.rs yang membungkus sim dgn stack besar) — evaluator
    //         rekursif pada chain BinaryOp panjang (`x0|x1|...|x63` di
    //         OpenC910 ct_rtu_encode_64) overflow stack default 8MB
    //         ("thread 'main' has overflowed its stack", ditemukan fuzzer).
    let t_ms = timeout_ms;
    // Watchdog: `join()` tanpa batas dulu — satu sim loop tak-berujung di
    // jalur in-process menghentikan kampanye selamanya.
    // Budget in-process 6× subprocess: `simulate_in_thread` bukan SATU sim
    // tapi rantai (compile + 2× sim O4 + 4-6 jalur differential + trace +
    // evidence). Satu `timeout_ms` (5s) untuk seluruh rantai membuat kasus
    // berat kena Hang PALSU dan Hang dihitung `is_bug()` → file bug + count
    // kampanye menyimpang (review sesi ini).
    let budget_ms = timeout_ms.saturating_mul(6);
    run_with_watchdog(
        Target::Simulator,
        source,
        budget_ms,
        "mivon-fuzz-sim",
        move |src| {
            simulate_in_thread(src, t_ms).unwrap_or_else(|| {
                mk(
                    Target::Simulator,
                    Oracle::O1NoCrash,
                    Category::Panic,
                    "thread sim gagal (join err)",
                    src,
                )
            })
        },
    )
}

/// Jalankan seluruh validasi sim in-process dalam thread stack besar.
fn simulate_in_thread(source: &str, timeout_ms: u64) -> Option<CaseResult> {
    let mk_r = |c: Category, o: Oracle, d: &str| Some(mk(Target::Simulator, o, c, d, source));
    // ── 2. In-process compile check (O1) ──
    let compile_result = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source));
    match compile_result {
        Ok(Err(e)) => {
            return mk_r(
                Category::CleanError,
                Oracle::O1NoCrash,
                &format!("compile gagal: {e}"),
            );
        }
        Err(_) => {
            return mk_r(
                Category::Panic,
                Oracle::O1NoCrash,
                "panic saat compile in-process",
            );
        }
        _ => {} // compile ok
    }

    // ── 3. O4 determinism: jalankan 2× dengan flags sama ──
    let default_flags = EngineFlags {
        use_packed_eval: false,
        use_dag_parallel: false,
        use_timing_wheel: false,
        use_mir_jit: false,
    };

    let r1 = std::panic::catch_unwind(|| {
        simulate_signals_with_flags_quiet(source, 1_000, &default_flags)
    });
    let r2 = std::panic::catch_unwind(|| {
        simulate_signals_with_flags_quiet(source, 1_000, &default_flags)
    });

    match (&r1, &r2) {
        (Ok(Ok(sigs1)), Ok(Ok(sigs2))) => {
            // Compare sebagai MAP (nama→nilai) — urutan Vec bisa beda antar
            // run (iterasi HashMap) walau isi sama → false nondeterminism.
            let map1: std::collections::BTreeMap<&str, &mivon_ir::LogicVec> =
                sigs1.iter().map(|(n, v)| (n.as_str(), v)).collect();
            let map2: std::collections::BTreeMap<&str, &mivon_ir::LogicVec> =
                sigs2.iter().map(|(n, v)| (n.as_str(), v)).collect();
            if map1 != map2 {
                return mk_r(
                    Category::NonDeterministic,
                    Oracle::O4Determinism,
                    &format!(
                        "sim non-deterministik: {} != {} signal values (2 run identik)",
                        signal_summary(sigs1),
                        signal_summary(sigs2)
                    ),
                );
            }
        }
        (Ok(Err(e1)), Ok(Err(_e2))) => {
            // Keduanya error — error deterministik, bukan bug
            return mk_r(
                Category::CleanError,
                Oracle::O1NoCrash,
                &format!("sim error deterministik: {e1}"),
            );
        }
        (Ok(Err(e)), _) | (_, Ok(Err(e))) => {
            // Salah satu error, satu ok — non-deterministik error
            return mk_r(
                Category::NonDeterministic,
                Oracle::O4Determinism,
                &format!("sim error non-deterministik: {e}"),
            );
        }
        (Err(_), _) | (_, Err(_)) => {
            return mk_r(
                Category::NonDeterministic,
                Oracle::O4Determinism,
                "sim panic tidak deterministik",
            );
        }
    }

    // Ambil hasil valid dari run pertama
    let sigs_default = match &r1 {
        Ok(Ok(s)) => s.clone(),
        _ => unreachable!("sudah di-handle di atas"),
    };

    // ── 4. O5 differential: semua jalur engine yang tersedia ──
    // Hasil akhir yang sama pada satu jalur belum membuktikan semantik benar.
    // Jalur packed, DAG, timing-wheel, dan MIR JIT memberi implementasi
    // alternatif untuk menemukan bug yang konsisten pada jalur default.
    let alternate_flags = [
        (
            "packed",
            EngineFlags {
                use_packed_eval: true,
                ..default_flags
            },
        ),
        (
            "dag",
            EngineFlags {
                use_dag_parallel: true,
                ..default_flags
            },
        ),
        (
            "timing-wheel",
            EngineFlags {
                use_timing_wheel: true,
                ..default_flags
            },
        ),
        (
            "mir-jit",
            EngineFlags {
                use_mir_jit: true,
                ..default_flags
            },
        ),
        // ── Kombinasi (interaksi flag) — validate.rs sudah definisikan 8 jalur
        // tapi oracle lama hanya bandingkan single-flag → kombinasi
        // packed+dag/packed+timing/dag+timing TIDAK pernah di-fuzz. Tambah
        // sekarang: interaksi antar jalur engine bisa memicu race/ordering
        // yang tidak terlihat pada jalur tunggal.
        (
            "packed+dag",
            EngineFlags {
                use_packed_eval: true,
                use_dag_parallel: true,
                ..default_flags
            },
        ),
        (
            "packed+timing",
            EngineFlags {
                use_packed_eval: true,
                use_timing_wheel: true,
                ..default_flags
            },
        ),
        (
            "dag+timing",
            EngineFlags {
                use_dag_parallel: true,
                use_timing_wheel: true,
                ..default_flags
            },
        ),
    ];
    for (name, flags) in alternate_flags {
        let result =
            std::panic::catch_unwind(|| simulate_signals_with_flags_quiet(source, 1_000, &flags));
        match result {
            Ok(Ok(signals))
                if {
                    // Compare map (urutan-insensitive) — urutan Vec bisa beda
                    // antar jalur engine walau isi sama.
                    let m_def: std::collections::BTreeMap<&str, &mivon_ir::LogicVec> =
                        sigs_default.iter().map(|(n, v)| (n.as_str(), v)).collect();
                    let m_alt: std::collections::BTreeMap<&str, &mivon_ir::LogicVec> =
                        signals.iter().map(|(n, v)| (n.as_str(), v)).collect();
                    m_def == m_alt
                } => {}
            Ok(Ok(signals)) => {
                return mk_r(
                    Category::Differential,
                    Oracle::O5Differential,
                    &format!(
                        "differential default vs {name}: {} != {}",
                        signal_summary(&sigs_default),
                        signal_summary(&signals)
                    ),
                );
            }
            Ok(Err(e)) => {
                return mk_r(
                    Category::Differential,
                    Oracle::O5Differential,
                    &format!("{name} path error, default ok: {e}"),
                );
            }
            Err(_) => {
                return mk_r(
                    Category::Differential,
                    Oracle::O5Differential,
                    &format!("{name} path panic, default ok"),
                );
            }
        }
    }

    // ── 5. Trace quality: pastikan trace punya data bermakna ──
    let trace_result =
        std::panic::catch_unwind(|| simulate_signals_with_trace_quiet(source, 1_000, 100));

    match trace_result {
        Ok(Ok((sigs_trace, trace))) => {
            if trace.is_empty() {
                return mk_r(
                    Category::Ok,
                    Oracle::O1NoCrash,
                    &format!("sim ok, no trace ({} signals)", sigs_trace.len()),
                );
            }

            // Trace ada — cek kualitas: minimal ada 1 trace entry yang bukan empty string
            let meaningful_traces: Vec<&String> = trace
                .iter()
                .filter(|t| !t.is_empty() && !t.trim().is_empty())
                .collect();
            if meaningful_traces.is_empty() {
                return mk_r(
                    Category::Ok,
                    Oracle::O1NoCrash,
                    &format!(
                        "sim ok, trace kosong ({} entries, {} signals)",
                        trace.len(),
                        sigs_trace.len()
                    ),
                );
            }

            let m_tr: std::collections::BTreeMap<&str, &mivon_ir::LogicVec> =
                sigs_trace.iter().map(|(n, v)| (n.as_str(), v)).collect();
            let m_d: std::collections::BTreeMap<&str, &mivon_ir::LogicVec> =
                sigs_default.iter().map(|(n, v)| (n.as_str(), v)).collect();
            if m_tr != m_d {
                return mk_r(
                    Category::Differential,
                    Oracle::O5Differential,
                    &format!(
                        "trace final state berbeda: {} != {}",
                        signal_summary(&sigs_default),
                        signal_summary(&sigs_trace)
                    ),
                );
            }

            let ev = std::panic::catch_unwind(|| crate::validate::evidence_only(source, 1_000))
                .unwrap_or_default();

            // ── 7. External reference (Icarus) — bukti KEBENARAN sim ──
            // Opsional via env MIVON_FUZZ_ICARUS=1: jalankan source juga di
            // iverilog+vvp (reference eksternal independen). Mismatch marker
            // = semantic divergence NYATA (hasil mivon != reference).
            if std::env::var("MIVON_FUZZ_ICARUS")
                .map(|v| v == "1")
                .unwrap_or(false)
            {
                let ic = crate::oracle_icarus::evaluate_icarus(source, timeout_ms);
                match ic.verdict {
                    crate::oracle_icarus::Verdict::Mismatch => {
                        return mk_r(Category::Differential, Oracle::O5Differential, &ic.detail);
                    }
                    crate::oracle_icarus::Verdict::MivonBug => {
                        return mk_r(Category::Panic, Oracle::O1NoCrash, &ic.detail);
                    }
                    crate::oracle_icarus::Verdict::Match => {
                        return mk_r(
                            Category::Ok,
                            Oracle::O5Differential,
                            &format!(
                                "sim VERIFIED vs Icarus reference ({}) — hasil identik",
                                ic.detail
                            ),
                        );
                    }
                    crate::oracle_icarus::Verdict::RefUnavailable => {
                        // reference N/A (iverilog tak bisa compile source ini)
                        // — jatuh ke audit X/stimulus di bawah.
                    }
                }
            }

            // ── 8. X/stimulus audit — bagan verdict: pasif vs suspicious ──
            let is_passive = ev.process_count == 0;
            let x_heavy = ev.signal_count > 0
                && (ev.x_remain + ev.z_remain) * 100 >= ev.signal_count.max(1) * 60;

            let detail = format!(
                "sim verified: {} signals, {} trace entries ({} meaningful); \
                 default/packed/DAG/timing-wheel/MIR-JIT konsisten; [evidence] {}",
                sigs_trace.len(),
                trace.len(),
                meaningful_traces.len(),
                ev.summary(),
            );

            if is_passive {
                // Design TANPA blok prosedural — X/Z wajar (undriven).
                return mk_r(
                    Category::Ok,
                    Oracle::O1NoCrash,
                    &format!("{detail} — design pasif (0 proses), X/Z wajar"),
                );
            }
            if x_heavy {
                // Stimulus ada tapi signal dominan X → butuh perhatian:
                // bisa wajar (X-latch) atau bukti state-propagation bug.
                return mk_r(
                    Category::Suspicious,
                    Oracle::O1NoCrash,
                    &format!(
                        "{detail} — stimulus ada ({}) namun X/Z dominan ({}/{})",
                        ev.process_count,
                        ev.x_remain + ev.z_remain,
                        ev.signal_count
                    ),
                );
            }

            mk_r(Category::Ok, Oracle::O1NoCrash, &detail)
        }
        Ok(Err(e)) => mk_r(
            Category::Differential,
            Oracle::O5Differential,
            &format!("trace gagal setelah sim default sukses: {e}"),
        ),
        Err(_) => mk_r(
            Category::Differential,
            Oracle::O5Differential,
            "trace panic setelah sim default sukses",
        ),
    }
}

/// Cek apakah semua bit dalam LogicVec bernilai Zero.
fn is_all_zero(lv: &mivon_ir::LogicVec) -> bool {
    lv.bits.iter().all(|b| *b == mivon_ir::LogicVal::Zero)
}

/// Buat ringkasan signal untuk pesan error (nama + nilai non-zero).
fn signal_summary(sigs: &[(String, mivon_ir::LogicVec)]) -> String {
    if sigs.is_empty() {
        return "0 signals".to_string();
    }
    let nonzero: Vec<String> = sigs
        .iter()
        .filter(|(_, lv)| !is_all_zero(lv))
        .map(|(name, lv)| format!("{}={}", name, lv.to_u64()))
        .take(5)
        .collect();
    if nonzero.is_empty() {
        format!("{} signals (all zero)", sigs.len())
    } else {
        format!(
            "{} signals ({} nonzero: {})",
            sigs.len(),
            nonzero.len(),
            nonzero.join(", ")
        )
    }
}

/// Deteksi assertion/violation di output (stderr/stdout).
fn detect_violation(output: &str) -> Option<String> {
    let violation_pats = [
        ("Assertion failed", "assertion failed"),
        ("assertion violation", "assertion violation"),
        ("$fatal", "$fatal hit"),
    ];
    for (pat, desc) in violation_pats {
        if let Some(line) = output.lines().find(|l| l.contains(pat)) {
            return Some(format!("{desc}: {}", line.trim()));
        }
    }
    None
}

/// O3: fmt round-trip (jika tool fmt tersedia via mivon_api::tools).
fn evaluate_fmt(source: &str, timeout_ms: u64) -> CaseResult {
    // Fmt target dijalankan in-process — round-trip check. Thread stack besar
    // (lexer/parser rekursif bisa overflow pada input mutasi — ditemukan fuzzer
    // seed 7 stack overflow di fmt ~kasus 500, sama dgn compile/sim) +
    // watchdog agar fmt lambat/loop tak macetkan kampanye.
    run_with_watchdog(Target::Fmt, source, timeout_ms, "mivon-fuzz-fmt", |src| {
        fmt_in_thread(src)
    })
}

fn fmt_in_thread(source: &str) -> CaseResult {
    let caught = std::panic::catch_unwind(|| fmt_roundtrip(source));
    match caught {
        Ok(Ok(())) => mk(
            Target::Fmt,
            Oracle::O3Roundtrip,
            Category::Ok,
            "fmt ok",
            source,
        ),
        Ok(Err(e)) => match e.category {
            Category::RoundtripMismatch => mk(
                Target::Fmt,
                Oracle::O3Roundtrip,
                Category::RoundtripMismatch,
                &e.detail,
                source,
            ),
            Category::CleanError => mk(
                Target::Fmt,
                Oracle::O1NoCrash,
                Category::CleanError,
                &e.detail,
                source,
            ),
            _ => mk(
                Target::Fmt,
                Oracle::O1NoCrash,
                Category::Panic,
                &e.detail,
                source,
            ),
        },
        Err(_) => mk(
            Target::Fmt,
            Oracle::O3Roundtrip,
            Category::Panic,
            "panic di fmt",
            source,
        ),
    }
}

struct FmtError {
    category: Category,
    detail: String,
}

/// fmt(fmt(s)) == fmt(s)? Idempotensi.
///
/// Jangan skip hasil yang tidak parseable — itu PELUANG EMAS: jika INPUT
/// valid tapi output fmt rusak, formatter merusak source (bug nyata).
fn fmt_roundtrip(source: &str) -> Result<(), FmtError> {
    // mfmt berbasis lexer MURNI (tanpa preprocessing). Source dengan directive
    // backtick (`` `define ``/`` `include ``/`` `ifdef ``) — apalagi inline di
    // tengah literal — di luar kontrak mfmt; lexing mentah menghasilkan output
    // tak stabil. Skip (bukan bug fmt).
    if source.contains('`') {
        return Err(FmtError {
            category: Category::CleanError,
            detail: "source mengandung preprocessor directive — di luar kontrak mfmt".to_string(),
        });
    }

    let input_parses =
        std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source).is_ok()).unwrap_or(false);

    let once = mivon_api::tools::fmt::format_source(source, 4);
    if once.is_empty() && !source.trim().is_empty() {
        if input_parses {
            return Err(FmtError {
                category: Category::RoundtripMismatch,
                detail: "fmt output kosong padahal input VALID".to_string(),
            });
        }
        return Err(FmtError {
            category: Category::CleanError,
            detail: "input tidak valid — fmt output kosong wajar".to_string(),
        });
    }
    let once_parses =
        std::panic::catch_unwind(|| mivon_api::compile_str_quiet(&once).is_ok()).unwrap_or(false);
    if !once_parses {
        if input_parses {
            // GOLDEN: fmt merusak source yang VALID — bug nyata.
            return Err(FmtError {
                category: Category::RoundtripMismatch,
                detail: format!(
                    "fmt output tidak parseable padahal input valid (fmt merusak source):\n{once}"
                ),
            });
        }
        // Input rusak (mutasi) → output rusak wajar, bukan bug fmt.
        return Err(FmtError {
            category: Category::CleanError,
            detail: "input tidak valid — skip (bukan bug fmt)".to_string(),
        });
    }
    let twice = mivon_api::tools::fmt::format_source(&once, 4);
    if twice != once {
        return Err(FmtError {
            category: Category::RoundtripMismatch,
            detail: format!("fmt(fmt(s)) != fmt(s):\n--- once ---\n{once}\n--- twice ---\n{twice}"),
        });
    }
    Ok(())
}

/// Scan output subprocess utk BUG MENYAMAR sbg warning/error (oracle O6).
///
/// Dua tingkat:
/// - `HiddenBug` — pola yang MUSTAHIL keputusan by-design ("internal error",
///   "corrupt", "must not happen", severity terbalik `error[WR...`) →
///   dianggap BUG (is_bug, tersave). Menangkap kelas bug yang dulu lolos
///   karena cuma jadi warning (kasus WR0102 menyembunyikan bug lebar).
/// - `Degraded` — mivon MENYERAH diam ("fallback", "treated as", "taking
///   true branch", "using null default", "returning 0", stub, unknown
///   function/class, ...) → sebagian besar by-design TAPI wajib terhitung
///   (dulu kategori Ok → degradasi senyap; lonjakan = sinyal review).
pub(crate) fn scan_hidden_diags(output: &str) -> Option<(Category, String)> {
    const STRONG: &[&str] = &[
        "internal error",
        "corrupt",
        "must not happen",
        // Input di-lowercase dulu → pola ikut huruf kecil.
        "error[wr",
    ];
    const WEAK: &[&str] = &[
        "fallback",
        "treated as",
        "taking true branch",
        "taking first case",
        "using null default",
        "cannot be resolved",
        "returning 0",
        " expansion skipped",
        "belum didukung",
        "unknown function",
        "unknown class",
        "cannot resolve identifier",
        "cannot call method",
    ];
    let mut degraded: Option<String> = None;
    for line in output.lines() {
        let lower = line.to_lowercase();
        for p in STRONG {
            if lower.contains(p) {
                return Some((
                    Category::HiddenBug,
                    format!(
                        "bug menyamar sbg warning/error — pola '{p}': {}",
                        line.trim()
                    ),
                ));
            }
        }
        if degraded.is_none() {
            for p in WEAK {
                if lower.contains(p) {
                    degraded = Some(format!("degradasi diam — pola '{p}': {}", line.trim()));
                    break;
                }
            }
        }
    }
    degraded.map(|d| (Category::Degraded, d))
}

/// Target CLI: jalankan mivon binary dengan arg random di cwd temp.
fn evaluate_cli(source: &str, timeout_ms: u64) -> CaseResult {
    // CLI/tool dijalankan ATAS source mutasi nyata — sebelum ini argumen acak
    // tanpa file (`let _ = source`), jadi tools (mcheck/melab/msim/...) dan
    // flag pipeline tak pernah tersentuh source yang dimutasi. Tulis source ke
    // temp file → jalankan `mivon <file> <flags>` / `mivon <tool> <file>`.
    let mut path = std::env::temp_dir();
    path.push(format!(
        "mivonfzcli_{}_{}.sv",
        std::process::id(),
        crate::next_crash_seq()
    ));
    if std::fs::write(&path, source).is_err() {
        return mk(
            Target::Cli,
            Oracle::O1NoCrash,
            Category::CleanError,
            "gagal tulis temp file",
            source,
        );
    }

    // RNG deterministik per-case (hash source) → kombinasi flags reproducible.
    let mut h: u64 = 0x9E3779B97F4A7C15;
    for b in source.bytes() {
        h = h.rotate_left(5) ^ u64::from(b).wrapping_mul(0x100000001B3);
        h = h.wrapping_mul(0x9E3779B97F4A7C15);
    }
    let mut rng = crate::Rng::new(h);

    let args = gen_cli_args(&mut rng, &path, source);
    let outcome = with_micd_isolated(|| crate::runner::run_args(&args, timeout_ms));
    let _ = std::fs::remove_file(&path);

    let args_desc = args.join(" ");

    // Double-run determinisme utk tool STATIS (mfmt/mcheck/mlint/minspect —
    // output logis harus identik antar run; baris timing di-strip). Msm/melab
    // punya timing output — tidak di-double-run. Oracle O4 atas tool CLI.
    let is_static_tool = matches!(
        args.first().map(String::as_str),
        Some("mfmt")
            | Some("mcheck")
            | Some("mlint")
            | Some("minspect")
            | Some("tbgen")
            | Some("waiver")
            | Some("cov")
    );
    if is_static_tool && outcome.kind == crate::runner::Kind::Ok && rng.chance(40) {
        let second = with_micd_isolated(|| crate::runner::run_args(&args, timeout_ms));
        if second.kind == crate::runner::Kind::Ok {
            let strip = |s: &str| -> Vec<String> {
                s.lines()
                    .filter(|l| !l.contains("time") && !l.contains("µs") && !l.contains("ms)"))
                    .map(|l| l.trim().to_string())
                    .collect()
            };
            if strip(&outcome.stdout) != strip(&second.stdout) {
                return mk(
                    Target::Cli,
                    Oracle::O4Determinism,
                    Category::NonDeterministic,
                    &format!("cli {args_desc}: tool output beda antar 2 run identik"),
                    source,
                );
            }
        }
    }

    match outcome.kind {
        crate::runner::Kind::Ok => {
            // O6: bug menyamar sbg warning/error — scan stdout+stderr
            // (HiddenBug = pola mustahil by-design → bug; Degraded = mivon
            // menyerah diam → dihitung, tampil summary — dulu semua Ok/senyap).
            let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
            if let Some((cat, detail)) = scan_hidden_diags(&combined) {
                mk(Target::Cli, Oracle::O1NoCrash, cat, &detail, source)
            } else {
                mk(
                    Target::Cli,
                    Oracle::O1NoCrash,
                    Category::Ok,
                    &format!("cli ok: {args_desc}"),
                    source,
                )
            }
        }
        crate::runner::Kind::CleanError => mk(
            Target::Cli,
            Oracle::O1NoCrash,
            Category::CleanError,
            &format!("cli {args_desc}: {}", outcome.stderr),
            source,
        ),
        crate::runner::Kind::Panic => mk(
            Target::Cli,
            Oracle::O1NoCrash,
            Category::Panic,
            &format!("cli {args_desc}: {}", outcome.stderr),
            source,
        ),
        crate::runner::Kind::Abort => mk(
            Target::Cli,
            Oracle::O1NoCrash,
            Category::Abort,
            &format!("cli {args_desc}: {}", outcome.stderr),
            source,
        ),
        crate::runner::Kind::Crash(code) => mk(
            Target::Cli,
            Oracle::O1NoCrash,
            Category::Panic,
            &format!("cli {args_desc}: crash code {code}: {}", outcome.stderr),
            source,
        ),
        crate::runner::Kind::Hang => mk(
            Target::Cli,
            Oracle::O1NoCrash,
            Category::Hang,
            &format!("cli {args_desc}: hang > {} ms", timeout_ms),
            source,
        ),
    }
}

/// Fuzzing VCD WAVEFORM PIPELINE (area belum tersentuh — mwave/tools VCD
/// tidak pernah di-fuzz):
/// 1. Generate VCD base dari source yang sim-ok via `mivon msim <sv> -o <vcd>`.
/// 2. Mutasi VCD (teks) 0-1x — VCD parser stress (format rusak, scope tak
///    seimbang, timestamp acak, dll).
/// 3. Jalankan subcommand mwave (stats/tree/search/export/compare/filter/merge/
///    get) atas VCD mutasi → O1 no-crash (panic/abort/hang di tool VCD = bug).
/// 4. O4: stats double-run → output harus identik.
fn evaluate_vcd(source: &str, timeout_ms: u64) -> CaseResult {
    use std::path::PathBuf;
    let mk_v = |c: Category, o: Oracle, d: &str| mk(Target::Vcd, o, c, d, source);

    // Hash source → RNG deterministik per-case.
    let mut h: u64 = 0x9E3779B97F4A7C15;
    for b in source.bytes() {
        h = h.rotate_left(5) ^ u64::from(b).wrapping_mul(0x100000001B3);
        h = h.wrapping_mul(0x9E3779B97F4A7C15);
    }
    let mut rng = crate::Rng::new(h);

    // ── 1. Generate VCD base ──
    let dir = std::env::temp_dir();
    let stem = format!(
        "mivonfzv_{}_{}",
        std::process::id(),
        crate::next_crash_seq()
    );
    let sv = dir.join(format!("{stem}.sv"));
    let vcd_base = dir.join(format!("{stem}_base.vcd"));
    let vcd_mut = dir.join(format!("{stem}_mut.vcd"));
    let vcd_out = dir.join(format!("{stem}_out.vcd"));
    if std::fs::write(&sv, source).is_err() {
        return mk_v(
            Category::CleanError,
            Oracle::O1NoCrash,
            "gagal tulis temp sv",
        );
    }
    let args = vec![
        "msim".to_string(),
        sv.to_string_lossy().to_string(),
        "-T".to_string(),
        "200".to_string(),
        "-o".to_string(),
        vcd_base.to_string_lossy().to_string(),
    ];
    let outcome = crate::runner::run_args(&args, timeout_ms);
    let _ = std::fs::remove_file(&sv);
    if outcome.kind != crate::runner::Kind::Ok {
        // Source tidak sim-ok (mutasi merusak sintaks/elab) — bukan area VCD.
        let _ = std::fs::remove_file(&vcd_base);
        return mk_v(
            Category::CleanError,
            Oracle::O1NoCrash,
            "sim pendahulu gagal — VCD tidak dihasilkan",
        );
    }
    // O6: bug menyamar sbg warning/error (lihat scan_hidden_diags) — output
    // sim pendahulu (stderr) dgn degradasi internal tak boleh tergolong Ok.
    {
        let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
        if let Some((cat, detail)) = scan_hidden_diags(&combined) {
            let _ = std::fs::remove_file(&vcd_base);
            return mk_v(cat, Oracle::O1NoCrash, &detail);
        }
    }
    let vcd_src = match std::fs::read_to_string(&vcd_base) {
        Ok(s) if s.trim().len() >= 32 => s,
        _ => {
            let _ = std::fs::remove_file(&vcd_base);
            return mk_v(
                Category::CleanError,
                Oracle::O1NoCrash,
                "VCD kosong/tidak terbentuk",
            );
        }
    };

    // ── 2. Mutasi VCD 0-1x (tanpa splice corpus — Corpus kosong) ──
    let empty_corpus = crate::corpus::Corpus::load(Some(&PathBuf::from("/nonexistent-fz-vcd")));
    let mut vcd_target = vcd_src;
    if rng.chance(70) {
        let mut mutator = crate::mutator::Mutator::new(&mut rng);
        vcd_target = mutator.mutate(&vcd_target, &empty_corpus);
    }
    if std::fs::write(&vcd_mut, &vcd_target).is_err() {
        let _ = std::fs::remove_file(&vcd_base);
        return mk_v(
            Category::CleanError,
            Oracle::O1NoCrash,
            "gagal tulis VCD mutasi",
        );
    }

    let mcmd = |args2: Vec<String>| -> crate::runner::Outcome {
        with_micd_isolated(|| crate::runner::run_args(&args2, timeout_ms))
    };
    let vcd_mut_s = vcd_mut.to_string_lossy().to_string();
    let vcd_base_s = vcd_base.to_string_lossy().to_string();
    let vcd_out_s = vcd_out.to_string_lossy().to_string();

    // ── 3. Kombinasi mwave subcommand (2 acak dari 10) ──
    // WAV-16 decode (protokol APB/AXI4Lite/AHB) — area BELUM di-fuzz
    // sebelumnya (daftar lama 8 subcommand tanpa decode).
    let cmds: [Vec<String>; 10] = [
        vec!["mwave".into(), "stats".into(), vcd_mut_s.clone()],
        vec!["mwave".into(), "tree".into(), vcd_mut_s.clone()],
        vec![
            "mwave".into(),
            "search".into(),
            vcd_mut_s.clone(),
            "*".into(),
        ],
        vec!["mwave".into(), "export".into(), vcd_mut_s.clone()],
        vec![
            "mwave".into(),
            "compare".into(),
            vcd_base_s.clone(),
            vcd_mut_s.clone(),
        ],
        vec![
            "mwave".into(),
            "filter".into(),
            vcd_mut_s.clone(),
            "q".into(),
            "clk".into(),
        ],
        vec![
            "mwave".into(),
            "merge".into(),
            vcd_base_s.clone(),
            vcd_mut_s.clone(),
            "-o".into(),
            vcd_out_s.clone(),
        ],
        vec![
            "mwave".into(),
            "get".into(),
            vcd_mut_s.clone(),
            "--at".into(),
            "0".into(),
        ],
        vec![
            "mwave".into(),
            "decode".into(),
            vcd_mut_s.clone(),
            "--proto".into(),
            "apb".into(),
        ],
        vec![
            "mwave".into(),
            "decode".into(),
            vcd_mut_s.clone(),
            "--proto".into(),
            "axi4lite".into(),
        ],
    ];
    let n = 1 + rng.below(2);
    for _ in 0..n {
        let idx = rng.below(cmds.len());
        let o = mcmd(cmds[idx].clone());
        match o.kind {
            crate::runner::Kind::Ok => {}
            crate::runner::Kind::CleanError => {
                // VCD korup → error parser wajar (bukan bug).
                let _ = std::fs::remove_file(&vcd_base);
                let _ = std::fs::remove_file(&vcd_mut);
                return mk_v(
                    Category::CleanError,
                    Oracle::O1NoCrash,
                    "mwave clean error (VCD invalid)",
                );
            }
            crate::runner::Kind::Panic => {
                let _ = std::fs::remove_file(&vcd_base);
                let _ = std::fs::remove_file(&vcd_mut);
                return mk_v(
                    Category::Panic,
                    Oracle::O1NoCrash,
                    &format!("mwave panic: {}", o.stderr),
                );
            }
            crate::runner::Kind::Abort => {
                let _ = std::fs::remove_file(&vcd_base);
                let _ = std::fs::remove_file(&vcd_mut);
                return mk_v(
                    Category::Abort,
                    Oracle::O1NoCrash,
                    &format!("mwave abort: {}", o.stderr),
                );
            }
            crate::runner::Kind::Crash(code) => {
                let _ = std::fs::remove_file(&vcd_base);
                let _ = std::fs::remove_file(&vcd_mut);
                return mk_v(
                    Category::Panic,
                    Oracle::O1NoCrash,
                    &format!("mwave crash code {code}: {}", o.stderr),
                );
            }
            crate::runner::Kind::Hang => {
                let _ = std::fs::remove_file(&vcd_base);
                let _ = std::fs::remove_file(&vcd_mut);
                return mk_v(
                    Category::Hang,
                    Oracle::O1NoCrash,
                    &format!("mwave hang > {} ms", timeout_ms),
                );
            }
        }
    }

    // ── 4. O4 stats double-run (output identik antar run) ──
    let stats_args = vec!["mwave".into(), "stats".into(), vcd_mut_s.clone()];
    let o1 = mcmd(stats_args.clone());
    let o2 = mcmd(stats_args);
    if o1.kind == crate::runner::Kind::Ok
        && o2.kind == crate::runner::Kind::Ok
        && o1.stdout != o2.stdout
    {
        let _ = std::fs::remove_file(&vcd_base);
        let _ = std::fs::remove_file(&vcd_mut);
        return mk_v(
            Category::NonDeterministic,
            Oracle::O4Determinism,
            "mwave stats non-deterministik: dua run identik hasil beda",
        );
    }

    let _ = std::fs::remove_file(&vcd_base);
    let _ = std::fs::remove_file(&vcd_mut);
    let _ = std::fs::remove_file(&vcd_out);
    mk_v(
        Category::Ok,
        Oracle::O1NoCrash,
        "vcd pipeline ok: mwave robust + deterministic",
    )
}

/// SDF base valid minimal — seed transisi utk mutasi teks SDF. SDF parser
/// (`crates/mivon-simulator/src/simulator/sdf.rs`) TIDAK pernah di-fuzz.
const SDF_BASE: &str = "(DELAYFILE
  (SDFVERSION \"3.0\")
  (DESIGN \"top\")
  (VENDOR \"mivon\")
  (PROGRAM \"mivon-fuzz\")
  (VERSION \"1.0\")
  (DIVIDER .)
  (TIMESCALE 1ns)
  (CELL (CELLTYPE \"top\") (INSTANCE u1)
    (DELAY (ABSOLUTE (IOPATH clk q (0.1:0.2:0.3) (0.4:0.5:0.6))))
  )
)
";

/// Fuzzing SDF timing pipeline (SIM-09) — area BELUM tersentuh:
/// 1. SDF teks di-mutasi 0-2x dari SDF_BASE (parser SDF stress).
/// 2. SV source TIDAK di-mutasi di sini — run_single sudah mutasi SV 0-4x
///    sebelum evaluate; mutasi ganda = amplifier blowup (terukur 267MB).
/// 3. Subprocess `mivon <sv> --sdf <sdf> -T 200` — parse_file + annotate_sdf
///    + sim dengan timing delay.
/// 4. O1 no-crash: panic/abort/hang di jalur SDF = bug (parser SDF, annotator,
///    atau engine timing). CleanError = SDF/SV invalid wajar.
fn evaluate_sdf(source: &str, timeout_ms: u64) -> CaseResult {
    use std::path::PathBuf;
    let mk_s = |c: Category, o: Oracle, d: &str| mk(Target::Sdf, o, c, d, source);

    // RNG deterministik per-case (hash source) → mutasi reproducible.
    let mut h: u64 = 0x9E3779B97F4A7C15;
    for b in source.bytes() {
        h = h.rotate_left(5) ^ u64::from(b).wrapping_mul(0x100000001B3);
        h = h.wrapping_mul(0x9E3779B97F4A7C15);
    }
    let mut rng = crate::Rng::new(h);
    let empty_corpus = crate::corpus::Corpus::load(Some(&PathBuf::from("/nonexistent-fz-sdf")));

    // 1. SDF text mutasi 0-2x.
    let mut sdf_text = SDF_BASE.to_string();
    let n_sdf = rng.below(3);
    if n_sdf > 0 {
        let mut mutator = crate::mutator::Mutator::new(&mut rng);
        for _ in 0..n_sdf {
            sdf_text = mutator.mutate(&sdf_text, &empty_corpus);
        }
    }

    // SV: seed fuzz as-is (sudah di-mutasi oleh run_single).
    let sv_text = source;

    // Env hook: tulis SDF pre-eval untuk repro hang/panic (pasangan dari
    // MIVON_FUZZ_TRACE — SV saja tidak cukup: kasus hang butuh SDF mutasi
    // yang juga per-case).
    if let Ok(trace_path) = std::env::var("MIVON_FUZZ_SDF_TRACE") {
        let _ = std::fs::write(&trace_path, &sdf_text);
    }

    // Capture per-case (sv+sdf persisten) — untuk lokalasi kasus hang/panic
    // yang butuh PASANGAN persis. Dipakai debug; normalnya tidak di-set.
    if let Ok(cap_dir) = std::env::var("MIVON_FUZZ_CAPTURE_DIR") {
        let _ = std::fs::create_dir_all(&cap_dir);
        let seq = crate::next_crash_seq();
        let _ = std::fs::write(
            std::path::Path::new(&cap_dir).join(format!("{seq}.sv")),
            sv_text,
        );
        let _ = std::fs::write(
            std::path::Path::new(&cap_dir).join(format!("{seq}.sdf")),
            &sdf_text,
        );
    }

    // 3. Tulis temp + jalankan.
    let dir = std::env::temp_dir();
    let stem = format!(
        "mivonfzs_{}_{}",
        std::process::id(),
        crate::next_crash_seq()
    );
    let sv = dir.join(format!("{stem}.sv"));
    let sdf = dir.join(format!("{stem}.sdf"));
    if std::fs::write(&sv, sv_text).is_err() || std::fs::write(&sdf, sdf_text).is_err() {
        return mk_s(
            Category::CleanError,
            Oracle::O1NoCrash,
            "gagal tulis temp sv/sdf",
        );
    }
    let args = vec![
        sv.to_string_lossy().to_string(),
        "--sdf".to_string(),
        sdf.to_string_lossy().to_string(),
        "-T".to_string(),
        "200".to_string(),
    ];
    let outcome = with_micd_isolated(|| crate::runner::run_args(&args, timeout_ms));
    let _ = std::fs::remove_file(&sv);
    let _ = std::fs::remove_file(&sdf);

    match outcome.kind {
        crate::runner::Kind::Ok => {
            // O6: bug menyamar sbg warning/error (lihat scan_hidden_diags).
            let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
            match scan_hidden_diags(&combined) {
                Some((cat, detail)) => mk_s(cat, Oracle::O1NoCrash, &detail),
                None => mk_s(
                    Category::Ok,
                    Oracle::O1NoCrash,
                    &format!("sdf pipeline ok: parse+annotate+sim ({:?})", stem),
                ),
            }
        }
        crate::runner::Kind::CleanError => mk_s(
            Category::CleanError,
            Oracle::O1NoCrash,
            &format!(
                "sdf clean error: {}",
                outcome.stderr.lines().next().unwrap_or("")
            ),
        ),
        crate::runner::Kind::Panic => mk_s(
            Category::Panic,
            Oracle::O1NoCrash,
            &format!("sdf panic: {}", outcome.stderr),
        ),
        crate::runner::Kind::Abort => mk_s(
            Category::Abort,
            Oracle::O1NoCrash,
            &format!("sdf abort: {}", outcome.stderr),
        ),
        crate::runner::Kind::Crash(code) => mk_s(
            Category::Panic,
            Oracle::O1NoCrash,
            &format!("sdf crash code {code}: {}", outcome.stderr),
        ),
        crate::runner::Kind::Hang => mk_s(
            Category::Hang,
            Oracle::O1NoCrash,
            &format!("sdf hang > {} ms", timeout_ms),
        ),
    }
}

/// Fuzzing MICD incremental database (`--fast` = run_fast + CompileSession +
/// MICD cache) — area BELUM tersentuh:
/// 1. Base: `mivon --fast <sv> -T 200` (cold cache, seed).
/// 2. Incremental: run ulang (cache hit) → hasil harus == fresh.
/// 3. Fresh: `--recompile` (lewati MICD) → base of truth.
/// 4. O4: incremental vs recompile output identik (setelah strip timing)?
///    Jika beda → cache drift / silent miscompilation = bug CRITICAL.
/// 5. O1: panic/hang di jalur --fast = bug.
///
/// MIVON_MICD_DIR diarahkan ke temp per-case agar tidak mencemari
/// `.mivon/database` project.
fn evaluate_micd(source: &str, timeout_ms: u64) -> CaseResult {
    let mk_m = |c: Category, o: Oracle, d: &str| mk(Target::Micd, o, c, d, source);

    let dir = std::env::temp_dir();
    let stem = format!(
        "mivonfzm_{}_{}",
        std::process::id(),
        crate::next_crash_seq()
    );
    let sv = dir.join(format!("{stem}.sv"));
    if std::fs::write(&sv, source).is_err() {
        return mk_m(
            Category::CleanError,
            Oracle::O1NoCrash,
            "gagal tulis temp sv",
        );
    }

    let sv_s = sv.to_string_lossy().to_string();
    let base: Vec<String> = vec![
        "--fast".to_string(),
        sv_s.clone(),
        "-T".to_string(),
        "200".to_string(),
    ];
    let recompile: Vec<String> = vec![
        "--fast".to_string(),
        sv_s.clone(),
        "-T".to_string(),
        "200".to_string(),
        "--recompile".to_string(),
    ];

    // Isolasi MICD per-case via helper (temp unik + cleanup otomatis).
    let (r1, r2, r3) = with_micd_isolated(|| {
        (
            crate::runner::run_args(&base, timeout_ms), // cold cache — seed
            crate::runner::run_args(&base, timeout_ms), // incremental
            crate::runner::run_args(&recompile, timeout_ms), // fresh
        )
    });
    let _ = std::fs::remove_file(&sv);

    if r1.kind != crate::runner::Kind::Ok {
        // Compile/sim gagal di run pertama (SV invalid) — bukan area MICD.
        return mk_m(
            Category::CleanError,
            Oracle::O1NoCrash,
            &format!(
                "run pertama gagal: {}",
                r1.stderr.lines().next().unwrap_or("")
            ),
        );
    }

    // O1: crash/hang pada jalur incremental/fresh.
    for (label, r) in [("incremental", &r2), ("recompile", &r3)] {
        match r.kind {
            crate::runner::Kind::Ok => {
                // O6: bug menyamar sbg warning/error (lihat scan_hidden_diags).
                let combined = format!("{}\n{}", r.stdout, r.stderr);
                if let Some((cat, detail)) = scan_hidden_diags(&combined) {
                    return mk_m(cat, Oracle::O1NoCrash, &format!("{label}: {detail}"));
                }
            }
            crate::runner::Kind::CleanError => {
                return mk_m(
                    Category::Differential,
                    Oracle::O5Differential,
                    &format!(
                        "{label} clean-error padahal run pertama ok: {}",
                        r.stderr.lines().next().unwrap_or("")
                    ),
                );
            }
            crate::runner::Kind::Panic => {
                return mk_m(
                    Category::Panic,
                    Oracle::O1NoCrash,
                    &format!("{label} panic: {}", r.stderr),
                );
            }
            crate::runner::Kind::Abort => {
                return mk_m(
                    Category::Abort,
                    Oracle::O1NoCrash,
                    &format!("{label} abort: {}", r.stderr),
                );
            }
            crate::runner::Kind::Crash(code) => {
                return mk_m(
                    Category::Panic,
                    Oracle::O1NoCrash,
                    &format!("{label} crash code {code}: {}", r.stderr),
                );
            }
            crate::runner::Kind::Hang => {
                return mk_m(
                    Category::Hang,
                    Oracle::O1NoCrash,
                    &format!("{label} hang > {} ms", timeout_ms),
                );
            }
        }
    }

    // O4: MICD incremental == fresh (strip baris timing/`[TIMING]` stderr tak
    // dipakai — stdout saja; baris metrik waktu di-strip).
    let strip = |s: &str| -> Vec<String> {
        s.lines()
            .filter(|l| !l.contains("time") && !l.contains("µs") && !l.contains("ms)"))
            .map(|l| l.trim().to_string())
            .collect()
    };
    let out_incr = strip(&r2.stdout);
    let out_fresh = strip(&r3.stdout);
    if out_incr != out_fresh {
        return mk_m(
            Category::NonDeterministic,
            Oracle::O4Determinism,
            "MICD drift: incremental != --recompile (cache mengubah hasil sim)",
        );
    }

    // Bandingkan juga dengan run pertama (cold) — konsistensi tiga arah.
    let out_cold = strip(&r1.stdout);
    if out_cold != out_incr {
        return mk_m(
            Category::NonDeterministic,
            Oracle::O4Determinism,
            "MICD drift: cold run != incremental run",
        );
    }

    let _ = r1;
    mk_m(
        Category::Ok,
        Oracle::O1NoCrash,
        "micd ok: incremental == recompile == cold",
    )
}

/// Fuzzing SYNTHESIS pipeline (`mivon synth --check-only`) — area BELUM
/// tersentuh: RTL→SIR lowering (`mivon-sir`) + SYN-1..9 sintesizability check.
/// Mutasi SV seed → subprocess `mivon synth <sv> --check-only`.
/// O1 no-crash (panic/hang SIR parser = bug) + O4 double-run determinisme
/// (stdout logis identik antar run, baris timing di-strip).
fn evaluate_synth(source: &str, timeout_ms: u64) -> CaseResult {
    let mk_y = |c: Category, o: Oracle, d: &str| mk(Target::Synth, o, c, d, source);

    let dir = std::env::temp_dir();
    let stem = format!(
        "mivonfzy_{}_{}",
        std::process::id(),
        crate::next_crash_seq()
    );
    let sv = dir.join(format!("{stem}.sv"));
    if std::fs::write(&sv, source).is_err() {
        return mk_y(
            Category::CleanError,
            Oracle::O1NoCrash,
            "gagal tulis temp sv",
        );
    }
    let args = vec![
        "synth".to_string(),
        sv.to_string_lossy().to_string(),
        "--check-only".to_string(),
    ];
    let outcome = with_micd_isolated(|| crate::runner::run_args(&args, timeout_ms));

    // O4: double-run — output logis identik (strip timing). Hanya berlaku
    // bila run pertama Ok; jangan klasifikasi bug ganda pada run pertama
    // error (klasifikasi run pertama di bawah).
    // NOTE: `sv` TIDAK dihapus di sini — run kedua butuh file yang sama.
    let second = with_micd_isolated(|| crate::runner::run_args(&args, timeout_ms));
    let _ = std::fs::remove_file(&sv);
    if outcome.kind == crate::runner::Kind::Ok {
        if second.kind == crate::runner::Kind::Ok {
            let strip = |s: &str| -> Vec<String> {
                s.lines()
                    .filter(|l| !l.contains("time") && !l.contains("µs") && !l.contains("ms)"))
                    .map(|l| l.trim().to_string())
                    .collect()
            };
            if strip(&outcome.stdout) != strip(&second.stdout) {
                return mk_y(
                    Category::NonDeterministic,
                    Oracle::O4Determinism,
                    "synth --check-only output beda antar 2 run identik",
                );
            }
        } else {
            return mk_y(
                Category::Differential,
                Oracle::O5Differential,
                &format!(
                    "synth run kedua gagal padahal run pertama ok: {}",
                    second.stderr.lines().next().unwrap_or("")
                ),
            );
        }
    }

    let args_desc = args.join(" ");
    match outcome.kind {
        crate::runner::Kind::Ok => {
            // O6: bug menyamar sbg warning/error (lihat scan_hidden_diags).
            let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
            match scan_hidden_diags(&combined) {
                Some((cat, detail)) => mk_y(cat, Oracle::O1NoCrash, &detail),
                None => mk_y(
                    Category::Ok,
                    Oracle::O1NoCrash,
                    &format!("synth check-only ok: {args_desc}"),
                ),
            }
        }
        crate::runner::Kind::CleanError => mk_y(
            Category::CleanError,
            Oracle::O1NoCrash,
            &format!(
                "synth clean error: {}",
                outcome.stderr.lines().next().unwrap_or("")
            ),
        ),
        crate::runner::Kind::Panic => mk_y(
            Category::Panic,
            Oracle::O1NoCrash,
            &format!("synth panic: {}", outcome.stderr),
        ),
        crate::runner::Kind::Abort => mk_y(
            Category::Abort,
            Oracle::O1NoCrash,
            &format!("synth abort: {}", outcome.stderr),
        ),
        crate::runner::Kind::Crash(code) => mk_y(
            Category::Panic,
            Oracle::O1NoCrash,
            &format!("synth crash code {code}: {}", outcome.stderr),
        ),
        crate::runner::Kind::Hang => mk_y(
            Category::Hang,
            Oracle::O1NoCrash,
            &format!("synth hang > {} ms", timeout_ms),
        ),
    }
}

/// Fuzzing AST-diff pipeline (`mcheck a.sv --ast-diff b.sv`) — area BARU:
/// 1. a.sv = source seed (sudah di-mutasi run_single); b.sv = mutasi KEDUA
///    dari source yang sama (0-2x) agar diff struktural benar-benar muncul
///    (bukan trivially-identical). Corpus kosong → splice 0.
/// 2. Dua arah: `mcheck a.sv --ast-diff b.sv` DAN `mcheck b.sv --ast-diff a.sv`.
/// 3. Simetri: himpunan diff ternormalisasi A→B == B→A. Swap membalik arah
///    nilai ("8 vs 16" → "16 vs 8") — himpunan (kind,node,loc) + nilai
///    TERURUT harus identik. Asimetri = bug render/recovery.
/// 4. O4: A→B double-run → stdout logis identik (strip timing).
/// 5. O1: panic/hang/abort/crash = bug.
///
/// CATATAN klasifikasi exit code: `mcheck --ast-diff` exit 0 = AST identik;
/// exit 1 = diff DITEMUKAN (jalur normal, bukan error!) ATAU "gagal compile"
/// (mutasi merusak sintaks). Keduanya bukan bug — tapi diff yang ditemukan
/// WAJIB diperiksa simetri + determinism-nya (di situ letak nilai fuzz).
fn evaluate_astdiff(source: &str, timeout_ms: u64) -> CaseResult {
    use std::path::PathBuf;
    let mk_a = |c: Category, o: Oracle, d: &str| mk(Target::Astdiff, o, c, d, source);

    // RNG deterministik per-case (hash source) → mutasi b reproducible.
    let mut h: u64 = 0x9E3779B97F4A7C15;
    for b in source.bytes() {
        h = h.rotate_left(5) ^ u64::from(b).wrapping_mul(0x100000001B3);
        h = h.wrapping_mul(0x9E3779B97F4A7C15);
    }
    let mut rng = crate::Rng::new(h);
    let empty_corpus = crate::corpus::Corpus::load(Some(&PathBuf::from("/nonexistent-fz-astdiff")));

    // b.sv = mutasi kedua (0-2x) — a.sv = source asli.
    let mut b_src = source.to_string();
    let n_mut = rng.below(3);
    if n_mut > 0 {
        let mut mutator = crate::mutator::Mutator::new(&mut rng);
        for _ in 0..n_mut {
            b_src = mutator.mutate(&b_src, &empty_corpus);
        }
    }

    let dir = std::env::temp_dir();
    let stem = format!(
        "mivonfza_{}_{}",
        std::process::id(),
        crate::next_crash_seq()
    );
    let a = dir.join(format!("{stem}_a.sv"));
    let b = dir.join(format!("{stem}_b.sv"));
    if std::fs::write(&a, source).is_err() || std::fs::write(&b, &b_src).is_err() {
        return mk_a(
            Category::CleanError,
            Oracle::O1NoCrash,
            "gagal tulis temp sv a/b",
        );
    }

    // Capture per-case (a+b persisten) — untuk lokalasi kasus asimetri/
    // nondeterminisme yang butuh PASANGAN persis (kontras: run_single hanya
    // simpan source kasus, bukan pasangan diff-nya). Dipakai debug; normal
    // tidak di-set (mirip MIVON_FUZZ_CAPTURE_DIR pada target SDF).
    if let Ok(cap_dir) = std::env::var("MIVON_FUZZ_CAPTURE_DIR") {
        let _ = std::fs::create_dir_all(&cap_dir);
        let seq = crate::next_crash_seq();
        let _ = std::fs::write(
            std::path::Path::new(&cap_dir).join(format!("{seq}_a.sv")),
            source,
        );
        let _ = std::fs::write(
            std::path::Path::new(&cap_dir).join(format!("{seq}_b.sv")),
            &b_src,
        );
    }
    let a_s = a.to_string_lossy().to_string();
    let b_s = b.to_string_lossy().to_string();
    let fwd_args = vec![
        "mcheck".to_string(),
        a_s.clone(),
        "--ast-diff".to_string(),
        b_s.clone(),
    ];
    let rev_args = vec![
        "mcheck".to_string(),
        b_s.clone(),
        "--ast-diff".to_string(),
        a_s.clone(),
    ];
    let fwd = with_micd_isolated(|| crate::runner::run_args(&fwd_args, timeout_ms));
    let rev = with_micd_isolated(|| crate::runner::run_args(&rev_args, timeout_ms));
    let fwd2 = if fwd.kind == crate::runner::Kind::Ok {
        with_micd_isolated(|| crate::runner::run_args(&fwd_args, timeout_ms))
    } else {
        crate::runner::Outcome {
            kind: crate::runner::Kind::CleanError,
            code: None,
            stdout: String::new(),
            stderr: String::new(),
            ms: 0,
        }
    };
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);

    // O1: crash/hang/abort pada kedua arah = bug.
    for (label, o) in [("fwd", &fwd), ("rev", &rev)] {
        match o.kind {
            crate::runner::Kind::Ok | crate::runner::Kind::CleanError => {}
            crate::runner::Kind::Panic => {
                return mk_a(
                    Category::Panic,
                    Oracle::O1NoCrash,
                    &format!("ast-diff {label} panic: {}", o.stderr),
                );
            }
            crate::runner::Kind::Abort => {
                return mk_a(
                    Category::Abort,
                    Oracle::O1NoCrash,
                    &format!("ast-diff {label} abort: {}", o.stderr),
                );
            }
            crate::runner::Kind::Crash(code) => {
                return mk_a(
                    Category::Panic,
                    Oracle::O1NoCrash,
                    &format!("ast-diff {label} crash code {code}: {}", o.stderr),
                );
            }
            crate::runner::Kind::Hang => {
                return mk_a(
                    Category::Hang,
                    Oracle::O1NoCrash,
                    &format!("ast-diff {label} hang > {} ms", timeout_ms),
                );
            }
        }
    }

    // Gagal compile di salah satu arah → diff tak bisa dihitung — bukan
    // area ast-diff (mutasi merusak sintaks/elab). CleanError wajar.
    for (label, o) in [("fwd", &fwd), ("rev", &rev)] {
        if o.kind == crate::runner::Kind::CleanError && !looks_like_diff_found(o) {
            return mk_a(
                Category::CleanError,
                Oracle::O1NoCrash,
                &format!(
                    "ast-diff {label}: {}",
                    o.stderr.lines().next().unwrap_or("")
                ),
            );
        }
    }

    // Kedua arah jalan (identik ATAU diff ditemukan) — O4 double-run fwd.
    if fwd.kind == crate::runner::Kind::Ok
        && fwd2.kind == crate::runner::Kind::Ok
        && strip_logical(&fwd.stdout) != strip_logical(&fwd2.stdout)
    {
        return mk_a(
            Category::NonDeterministic,
            Oracle::O4Determinism,
            "ast-diff fwd output beda antar 2 run identik",
        );
    }

    // Ekstrak diff lines `  - ...` dan cek SIMETRI himpunan A→B == B→A.
    let fwd_diffs = extract_diff_lines(&fwd.stdout);
    let rev_diffs = extract_diff_lines(&rev.stdout);
    let fwd_set: std::collections::BTreeSet<String> =
        fwd_diffs.iter().map(|d| normalize_diff_line(d)).collect();
    let rev_set: std::collections::BTreeSet<String> =
        rev_diffs.iter().map(|d| normalize_diff_line(d)).collect();
    if fwd_set != rev_set {
        // Asimetri render diff — himpunan beda walau isi AST sama.
        let only_fwd: Vec<&String> = fwd_set.difference(&rev_set).collect();
        let only_rev: Vec<&String> = rev_set.difference(&fwd_set).collect();
        return mk_a(
            Category::Differential,
            Oracle::O5Differential,
            &format!(
                "ast-diff ASIMETRIS A→B != B→A: only-fwd={} only-rev={}",
                only_fwd.len(),
                only_rev.len()
            ),
        );
    }

    // O6: bug menyamar sbg warning/error (lihat scan_hidden_diags) —
    // degradasi internal pada output kedua arah tak boleh tergolong Ok.
    {
        let combined = format!(
            "{}\n{}\n{}\n{}",
            fwd.stdout, fwd.stderr, rev.stdout, rev.stderr
        );
        if let Some((cat, detail)) = scan_hidden_diags(&combined) {
            return mk_a(cat, Oracle::O1NoCrash, &detail);
        }
    }

    if fwd_diffs.is_empty() {
        mk_a(
            Category::Ok,
            Oracle::O1NoCrash,
            "ast-diff ok: AST identik, simetri terjaga",
        )
    } else {
        mk_a(
            Category::Ok,
            Oracle::O5Differential,
            &format!(
                "ast-diff ok: {} perbedaan struktural, himpunan simetris + deterministik",
                fwd_diffs.len()
            ),
        )
    }
}

/// Apakah output mcheck menunjukkan diff DITEMUKAN (bukan gagal compile)?
/// Exit 1 dipakai dua hal: diff ditemukan (jalur NORMAL) vs gagal compile.
fn looks_like_diff_found(o: &crate::runner::Outcome) -> bool {
    let s = format!("{}\n{}", o.stdout, o.stderr);
    s.contains("perbedaan struktural") || s.contains("AST berbeda") || s.contains("AST identik")
}

/// Strip baris timing/metrik (konsisten dgn target lain).
fn strip_logical(s: &str) -> Vec<String> {
    s.lines()
        .filter(|l| !l.contains("time") && !l.contains("µs") && !l.contains("ms)"))
        .map(|l| l.trim().to_string())
        .collect()
}

/// Ekstrak baris diff `  - ...` dari stdout mcheck.
fn extract_diff_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| {
            let t = l.trim_start();
            t.strip_prefix("- ").map(|rest| rest.to_string())
        })
        .collect()
}

/// Normalisasi satu diff line utk perbandingan simetri lintas arah:
/// 0. Label "only in A"/"only in B" → "only in ONE" (swap membalik ARAH
///    label — identitas diff = ada di satu design saja).
/// 0b. Pasangan boolean "true/false vs ..." → diurutkan.
/// 1. Nilai pasangan "A vs B" di-urutkan -> "min vs max", jadi
///    "width: 8 vs 16" (A→B) == "width: 16 vs 8" (B→A).
/// 2. Indeks posisi `signal[N]` di-buang -> `signal[*]`: `compare_ir_designs`
///    membandingkan signal PAIRWISE BY INDEX (Vec order). Dua design dengan
///    URUTAN signal berbeda (b.sv = hasil mutasi, urutan bisa beda dari
///    a.sv) membuat pasangan index bergeser per arah — nama di index yang
///    sama bisa beda → asimetri SEMU, bukan bug render. Identitas diff yang
///    benar = (kind, nama signal, nilai) — posisi tidak relevan.
fn normalize_diff_line(line: &str) -> String {
    let mut s = line.to_string();
    // (0) Label only-in kanonik: swap membalik ARAH label ("only in A" ↔
    // "only in B" tukar posisi file). Identitas diff = signal ada di SATU
    // design (arah tidak relevan untuk himpunan simetri).
    s = s
        .replace(" only in A", " only in ONE")
        .replace(" only in B", " only in ONE");
    // (0b) Pasangan boolean: "true vs false" / "false vs true" → urutkan.
    if s.contains("true vs false") || s.contains("false vs true") {
        s = s
            .replace("true vs false", "\u{1}T")
            .replace("false vs true", "\u{1}T")
            .replace('\u{1}', "false vs true");
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let num = chars[i..j].iter().collect::<String>();
            // (2) `signal[N]` → `signal[*]` — position-insensitive.
            let idx_is_signal_position = {
                let before = &chars[..i];
                let mut k = before.len();
                let word_end = k;
                while k > 0 && (chars[k - 1].is_ascii_alphanumeric() || chars[k - 1] == '_') {
                    k -= 1;
                }
                let word: String = before[k..].iter().collect();
                word == "signal" && chars.get(word_end).map(|c| *c == '[').unwrap_or(false)
            };
            if idx_is_signal_position {
                // cari `]` penutup
                let mut m = j;
                while m < chars.len() && chars[m] != ']' {
                    m += 1;
                }
                if m < chars.len() {
                    out.push_str("signal[*]");
                    i = m + 1;
                    continue;
                }
            }
            // (1) pola "<num> vs <num>": urutkan nilainya.
            let is_vs = j + 4 <= chars.len()
                && chars[j] == ' '
                && chars[j + 1] == 'v'
                && chars[j + 2] == 's'
                && chars[j + 3] == ' ';
            if is_vs {
                // cari angka kedua
                let mut k = j + 4;
                while k < chars.len() && chars[k] == ' ' {
                    k += 1;
                }
                let mut m = k;
                while m < chars.len() && chars[m].is_ascii_digit() {
                    m += 1;
                }
                if m > k {
                    let num2 = chars[k..m].iter().collect::<String>();
                    let (a, b) = (
                        num.parse::<u64>().unwrap_or(0),
                        num2.parse::<u64>().unwrap_or(0),
                    );
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    out.push_str(&lo.to_string());
                    out.push_str(" vs ");
                    out.push_str(&hi.to_string());
                    i = m;
                    continue;
                }
            }
            out.push_str(&num);
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Argumen CLI untuk satu kasus fuzz: tool subcommand ATAU pipeline flags,
/// atas satu file temp. RNG per-case (bukan global) → deterministik.
/// Nama module/interface pertama di source — untuk arg `-m` tool yang tak
/// menerima file positional (`mivon tbgen [OPTIONS]` membaca dari workspace,
/// bukan FILES).
fn first_module_name(source: &str) -> Option<String> {
    for line in source.lines() {
        let t = line.trim_start();
        for kw in ["module", "interface"] {
            let Some(rest) = t.strip_prefix(kw) else {
                continue;
            };
            let rest = rest.trim_start();
            let mut chars = rest.chars();
            match chars.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
                _ => continue,
            }
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn gen_cli_args(rng: &mut crate::Rng, path: &std::path::Path, source: &str) -> Vec<String> {
    let file = path.to_string_lossy().to_string();
    let mut args = Vec::new();

    // 50%: tool subcommand `mivon <tool> <file> [flags]` — area mivon-tools
    // (mcheck/melab/msim/mfmt/mlint/minspect/mprof/synth) di-fuzz atas
    // source mutasi. mbench/mcov-closure sengaja dilewatkan (mbench: run
    // benchmark penuh berkali-kali = timeout; cov-closure: butuh 2 tahap
    // file coverage.json terpisah). synth pakai --check-only (cepat, 0.03s).
    //
    // Perluasan 2026-09-28 (tak pernah terjamah sebelumnya):
    // - `cov` (mcov): pipeline coverage → coverage.json + coverage.html —
    //   sim + CoverageDatabase + report writer.
    // - `tbgen` (mtbgen): generator testbench dari port module — area
    //   codegen TB yang belum pernah di-fuzz.
    // - `waiver` (mwaiver): manajemen waiver lint/formal — file waiver.
    // - `bench` (mbench, `-n 1`): jalur benchmark compile 1 run — O1 utk
    //   pengukuran (dulu dilewatkan krn default 3 run).
    // - `emu` (R0 MHIR extraction + memory map): emulator extraction path.
    if rng.chance(50) {
        let tools = [
            "mcheck", "melab", "msim", "mfmt", "mlint", "minspect", "mprof", "synth", "cov",
            "tbgen", "waiver", "bench", "emu",
        ];
        let t = tools[rng.below(tools.len())];
        args.push(t.to_string());
        // Tool dengan bentuk argumen BUKAN `<tool> <file>` (clap strict →
        // arg aneh = clap error, noise clean_error bukan fuzz):
        // - `waiver <COMMAND>`: subcommand saja. `list` = read-only.
        // - `tbgen [OPTIONS]`: TANPA positional file — beri `-m <module>`
        //   hasil ekstraksi dari source (default stdout, deterministik).
        match t {
            "waiver" => {
                args.push("list".to_string());
                return args;
            }
            "tbgen" => {
                if let Some(m) = first_module_name(source) {
                    args.push("-m".to_string());
                    args.push(m);
                }
                return args;
            }
            _ => {}
        }
        args.push(file.clone());
        match t {
            "msim" => {
                args.push("-T".to_string());
                args.push(["50", "100", "200", "1000"][rng.below(4)].to_string());
                // FST waveform writer (`{top}.fst` di cwd) — wilayah belum
                // di-jangkau: FstWaveWriter (crates/mivon-simulator/waveform/
                // fst.rs) tidak pernah di-fuzz (VCD fuzzer via mwave hanya
                // sentuh teks VCD, bukan writer FST binary). `--fst` memicu
                // jalur write_header/hierarchy — any panic/abort/hang = bug.
                if rng.chance(50) {
                    args.push("--fst".to_string());
                }
                // Assertion/coverage ringkasan pasca-sim — jalur report yang
                // belum pernah di-fuzz (assert/cover statement handling).
                if rng.chance(30) {
                    args.push("--assertions".to_string());
                }
                if rng.chance(30) {
                    args.push("--coverage".to_string());
                }
            }
            "mfmt" => {
                if rng.chance(25) {
                    args.push("--check".to_string());
                }
            }
            "synth" => {
                args.push("--check-only".to_string());
            }
            "cov" => {
                // mcov: sim + CoverageDatabase → coverage.json/html. -T
                // pendek (kasus fuzz berbobot kecil); json/html masing-masing
                // chance terpisah agar jalur writer per-format terjamah.
                args.push("-T".to_string());
                args.push("100".to_string());
                if rng.chance(60) {
                    args.push("--json".to_string());
                }
                if rng.chance(60) {
                    args.push("--html".to_string());
                }
            }
            "bench" => {
                // Default 3 run = kelipatan waktu; 1 run cukup utuk O1
                // (bench TIDAK masuk static-tool double-run: output berisi
                // angka timing yang bisa beda antar run).
                args.push("-n".to_string());
                args.push("1".to_string());
            }
            "emu" => {
                // R0 MHIR extraction — jalur emulator tak pernah di-fuzz.
                if rng.chance(50) {
                    args.push("--dump-mhir".to_string());
                }
                if rng.chance(40) {
                    args.push("--dump-memory-map".to_string());
                }
            }
            _ => {}
        }
        return args;
    }

    // Pipeline: `mivon <file> [-T N] <flags>` — flag nondestruktif.
    // --debug/--step dulu dilewatkan (interaktif) — diuji manual: semua flag
    // debug keluar EXIT 0 dgn stdin-null, tak hang. Tambah sekarang:
    // --deep-debug/--break-cycle/--timeline/--print-signal/--snap-interval/
    // --watch/--debug = area debugger yang belum pernah di-fuzz.
    args.push(file.clone());
    if rng.chance(70) {
        args.push("-T".to_string());
        args.push(["50", "100", "200", "1000"][rng.below(4)].to_string());
    }
    // mode run_fast: 20% jalur MICD penuh (`--fast` memicu run_fast — NOTA:
    // dulu sisipkan "run_fast" sbg argv[0] yang dianggap FILE oleh mivon →
    // clean error noise; sekarang flag `--fast` yang benar).
    if rng.chance(20) {
        args.push("--fast".to_string());
    }

    // Formal BMC (Z3) — `--formal` mengaktifkan mivon-formal (bounded model
    // checking) yang BELUM PERNAH di-fuzz sama sekali. Chance kecil (8%) —
    // BMC berat; grace 3× runner menampung kasus lambat tanpa jadi hang
    // palsu.
    if rng.chance(8) {
        args.push("--formal".to_string());
    }

    let flags = [
        "--ast",
        "--tokens",
        "--tree",
        "--print-state",
        "--coverage",
        "--fast",
        "--recompile",
        "--deep-debug",
        "--debug",
    ];
    let n = 1 + rng.below(3);
    for _ in 0..n {
        if rng.chance(55) {
            args.push(flags[rng.below(flags.len())].to_string());
        }
    }
    // Flag debug bernilai: break-cycle / snap-interval / timeline / print-signal
    if rng.chance(45) {
        let cv = rng.below(3);
        match cv {
            0 => {
                args.push("--break-cycle".to_string());
                args.push(format!("{}", 1 + rng.below(50)));
            }
            1 => {
                args.push("--snap-interval".to_string());
                args.push(format!("{}", 10 + rng.below(200)));
            }
            _ => {
                // --timeline/--print-signal/--watch butuh nama signal — pilih
                // ident pendek dari source (nama signal nyata bila ada).
                let mut name = "q".to_string();
                'outer: for w in source.split([' ', '\n', '\t', '(', ')', ',', ';', '[', ']', '.'])
                {
                    let w = w.trim();
                    if w.len() >= 2
                        && w.len() <= 16
                        && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && !w.is_empty()
                    {
                        name = w.to_string();
                        break 'outer;
                    }
                }
                let kind = rng.below(3);
                match kind {
                    0 => {
                        args.push("--timeline".to_string());
                        args.push(name.clone());
                    }
                    1 => {
                        args.push("--print-signal".to_string());
                        args.push(name.clone());
                    }
                    _ => {
                        args.push("--watch".to_string());
                        args.push(name.clone());
                    }
                }
            }
        }
    }
    args
}

/// Fuzzing PREPROCESSOR (area belum tersentuh):
/// - output preprocess BERUBAH antar dua run identik = non-deterministik (bug)
/// - panic saat directive imbalance (`ifdef` ganda dsb) = bug
/// - error tanpa lokasi file:line:col = diag_missing
fn evaluate_preproc(source: &str, timeout_ms: u64) -> CaseResult {
    // Watchdog: `ifdef` imbalance/balik bisa membuat preprocess loop tak
    // berujung — tanpa batas kampanye macet.
    run_with_watchdog(
        Target::Preproc,
        source,
        timeout_ms,
        "mivon-fuzz-preproc",
        preproc_eval,
    )
}

fn preproc_eval(source: &str) -> CaseResult {
    // Jalankan preprocess 2x — determinisme.
    let r1 = std::panic::catch_unwind(|| mivon_preproc(source));
    let r2 = std::panic::catch_unwind(|| mivon_preproc(source));
    match (r1, r2) {
        (Ok(Ok(o1)), Ok(Ok(o2))) => {
            if o1 != o2 {
                return mk(
                    Target::Preproc,
                    Oracle::O4Determinism,
                    Category::NonDeterministic,
                    "preprocess non-deterministik: dua run identik hasil beda",
                    source,
                );
            }
            mk(
                Target::Preproc,
                Oracle::O1NoCrash,
                Category::Ok,
                "preproc deterministik",
                source,
            )
        }
        (Ok(Err(e1)), Ok(Err(e2))) => {
            if e1 == e2 {
                let has_loc = extract_loc(&e1);
                if has_loc {
                    mk(
                        Target::Preproc,
                        Oracle::O1NoCrash,
                        Category::CleanError,
                        &e1,
                        source,
                    )
                } else {
                    mk(
                        Target::Preproc,
                        Oracle::O2DiagLocation,
                        Category::DiagMissing,
                        &format!("preproc tanpa lokasi: {e1}"),
                        source,
                    )
                }
            } else {
                mk(
                    Target::Preproc,
                    Oracle::O4Determinism,
                    Category::NonDeterministic,
                    &format!("error preproc beda: {e1} vs {e2}"),
                    source,
                )
            }
        }
        (Ok(Err(e)), _) | (_, Ok(Err(e))) => mk(
            Target::Preproc,
            Oracle::O4Determinism,
            Category::NonDeterministic,
            &format!("satu run error satu ok: {e}"),
            source,
        ),
        _ => mk(
            Target::Preproc,
            Oracle::O1NoCrash,
            Category::Panic,
            "panic saat preprocess",
            source,
        ),
    }
}

/// Jalankan preprocessor mivon (public API) — kembalikan output atau error.
fn mivon_preproc(source: &str) -> Result<String, String> {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    match pp.preprocess(source, None) {
        Ok(out) => {
            // Tambah folder include default agar dirwasa; ok.
            Ok(out)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Fuzzing TRANSPILER MV → SV (area belum tersentuh):
/// - transpile 2x identik = determinisme (macro/state leak)
/// - output SV harus parseable (transpile merusak source = bug)
/// - panic saat MV aneh = bug
fn evaluate_mv(source: &str, timeout_ms: u64) -> CaseResult {
    // Watchdog: transpile + compile check in-process (tanpa batas, loop
    // transpiler menghentikan kampanye).
    run_with_watchdog(Target::Mv, source, timeout_ms, "mivon-fuzz-mv", mv_eval)
}

fn mv_eval(source: &str) -> CaseResult {
    // Source kosong/whitespace-only hasil mutasi (file habis terhapus) —
    // transpile input kosong → output tak parseable wajar, BUKAN bug
    // transpiler (temuan kampanye: klass roundtrip_mismatch palsu).
    if source.trim().is_empty() {
        return mk(
            Target::Mv,
            Oracle::O1NoCrash,
            Category::CleanError,
            "source kosong setelah mutasi — bukan bug transpiler",
            source,
        );
    }
    let r1 = std::panic::catch_unwind(|| mivon_api::mv::transpile(source, "fz"));
    let r2 = std::panic::catch_unwind(|| mivon_api::mv::transpile(source, "fz"));
    match (r1, r2) {
        (Ok(Ok(t1)), Ok(Ok(t2))) => {
            if t1.sv != t2.sv || t1.svh != t2.svh {
                return mk(
                    Target::Mv,
                    Oracle::O4Determinism,
                    Category::NonDeterministic,
                    "transpile non-deterministik",
                    source,
                );
            }
            // Output SV harus parseable — sertakan pesan error di detail
            // (tanpa ini "tidak parseable" tak bisa dibedakan: EL3001 no-top
            // = noise vs kerusakan codegen = bug nyata).
            let combined = format!("{}\n{}", t1.svh, t1.sv);
            match std::panic::catch_unwind(|| mivon_api::compile_str_quiet(&combined)) {
                Ok(Ok(_)) => mk(
                    Target::Mv,
                    Oracle::O1NoCrash,
                    Category::Ok,
                    "mv transpile deterministik + output parseable",
                    source,
                ),
                Ok(Err(e)) => {
                    let msg = e.to_string();
                    if is_global_error(&msg) {
                        // Output VALID tapi tanpa top module (design
                        // class/package/interface saja) — E3005 by design,
                        // sama dgn guard evaluate_compile. BUKAN kerusakan
                        // codegen (false positive: class_model.mv utuh
                        // dilaporkan roundtrip_mismatch).
                        mk(
                            Target::Mv,
                            Oracle::O1NoCrash,
                            Category::Ok,
                            &format!("mv transpile ok — tanpa top module: {msg}"),
                            source,
                        )
                    } else {
                        mk(
                            Target::Mv,
                            Oracle::O3Roundtrip,
                            Category::RoundtripMismatch,
                            &format!(
                                "transpile output tidak parseable (transpiler merusak source): {msg}"
                            ),
                            source,
                        )
                    }
                }
                Err(_) => mk(
                    Target::Mv,
                    Oracle::O3Roundtrip,
                    Category::RoundtripMismatch,
                    "panic saat compile output transpile",
                    source,
                ),
            }
        }
        (Ok(Err(_e1)), Ok(Err(_e2))) => {
            // Error deterministik — MV tak valid, bukan bug.
            mk(
                Target::Mv,
                Oracle::O1NoCrash,
                Category::CleanError,
                "mv transpile error deterministik",
                source,
            )
        }
        _ => mk(
            Target::Mv,
            Oracle::O4Determinism,
            Category::NonDeterministic,
            "transpile panic/error non-deterministik",
            source,
        ),
    }
}

/// Fuzzing HAKIM LRM (O6): `judge_single` atas source mutasi — registry
/// aturan penuh. `Violation` layak-simpan → `LrmViolation` (bug kepatuhan);
/// sumber tak-ter-compile (`InvalidTest`) → `CleanError` (bukan bug mivon);
/// sisanya → `Ok`. Berat per-case (subprocess + compile + judge), jadi
/// target ini untuk kampanye terarah, bukan default cepat.
fn evaluate_judge(source: &str, timeout_ms: u64) -> CaseResult {
    // Watchdog + stack besar + budget 6× seperti evaluate_sim: judge_single
    // = rantai (subprocess + compile + registry penuh), dan input mutasi bisa
    // overflow stack 8MB (stack overflow = abort proses, tak tertangkap
    // catch_unwind → kampanye mati total tanpa watchdog).
    let budget_ms = timeout_ms.saturating_mul(6);
    run_with_watchdog(
        Target::Judge,
        source,
        budget_ms,
        "mivon-fuzz-judge",
        move |src| judge_in_thread(src, timeout_ms),
    )
}

/// Isi evaluate_judge di thread watchdog (lihat atas).
fn judge_in_thread(source: &str, timeout_ms: u64) -> CaseResult {
    let mk_j = |c: Category, o: Oracle, d: &str| mk(Target::Judge, o, c, d, source);
    let caught = std::panic::catch_unwind(|| crate::judge::judge_single(source, timeout_ms));
    let report = match caught {
        Ok(r) => r,
        Err(_) => {
            return mk_j(
                Category::Panic,
                Oracle::O6LrmJudge,
                "panic di judge_single (hakim crash pada input ini)",
            )
        }
    };
    if report.should_save() {
        // Detail diskriminatif: aturan pertama yang violated + alasannya
        // (signature dedup pakai 2 baris detail → grup per aturan).
        // Fallback sertakan verdict lengkap (kasus MivonInternalFailure:
        // kind+detail ikut, tak runtuh jadi satu label).
        let first = report
            .rule_results
            .iter()
            .find(|r| {
                matches!(
                    r.verdict,
                    crate::verdict::RuleVerdict::Violated { .. }
                )
            })
            .map(|r| format!("{}: {}", (r.rule.0), r.explanation.lines().next().unwrap_or("")))
            .unwrap_or_else(|| format!("{:?}", report.verdict));
        return mk_j(Category::LrmViolation, Oracle::O6LrmJudge, &first);
    }
    match &report.verdict {
        crate::verdict::Verdict::InvalidTest { reason } => {
            mk_j(Category::CleanError, Oracle::O1NoCrash, reason)
        }
        crate::verdict::Verdict::MivonInternalFailure { kind, detail } => {
            // Seharusnya should_save() true — pertahanan bila klasifikasi
            // berubah: tetap bug, jangan senyap. Oracle O6 (ditemukan hakim).
            mk_j(
                Category::Panic,
                Oracle::O6LrmJudge,
                &format!("internal failure lolos should_save: {kind:?}: {detail}"),
            )
        }
        _ => mk_j(
            Category::Ok,
            Oracle::O6LrmJudge,
            &format!("lrm judge ok: {}", report.verdict.label()),
        ),
    }
}

/// Ekstrak lokasi `file:line:col` dari pesan error.
fn extract_loc(msg: &str) -> bool {
    // Pola umum mivon: "file.sv:12:7: error: ..." atau "path:12:7:  ..."
    has_num_colon(msg)
}

fn has_num_colon(msg: &str) -> bool {
    let bytes = msg.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i + 2 < n {
        if bytes[i].is_ascii_digit() && bytes[i + 1] == b':' && bytes[i + 2].is_ascii_digit() {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    /// O6 HiddenBug: pola mustahil by-design harus terdeteksi sbg bug.
    #[test]
    fn scan_detects_hidden_bug_strong_patterns() {
        for out in [
            "internal error: registry corrupt",
            "state corrupt after commit",
            "assert: must not happen here",
            "error[WR0102]: width mismatch", // severity terbalik
        ] {
            let r = scan_hidden_diags(out).expect(out);
            assert_eq!(r.0, Category::HiddenBug, "pola: {out}");
        }
    }

    /// O6 Degraded: mivon menyerah diam → dihitung (bukan Ok senyap),
    /// tapi BUKAN bug hard.
    #[test]
    fn scan_detects_degraded_fallback() {
        let r = scan_hidden_diags(
            "warning[E9001]: width of port 'x' cannot be resolved — fallback lebar 1",
        )
        .expect("harus terdeteksi");
        assert_eq!(r.0, Category::Degraded);
        assert!(!r.0.is_bug(), "Degraded bukan bug hard");
        let r2 = scan_hidden_diags(
            "warning[RT8001]: DPI function 'f' not found in imports, returning 0",
        )
        .expect("harus terdeteksi");
        assert_eq!(r2.0, Category::Degraded);
    }

    /// Output bersih → None (tidak menandai Ok sbg Degraded).
    #[test]
    fn scan_clean_output_returns_none() {
        assert!(scan_hidden_diags("Simulation completed at time 16\nok").is_none());
        assert!(scan_hidden_diags("").is_none());
    }

    /// HiddenBug menang atas Degraded pada output campuran.
    #[test]
    fn scan_strong_wins_over_weak() {
        let r = scan_hidden_diags("fallback line\ninternal error: boom").expect("ada");
        assert_eq!(r.0, Category::HiddenBug);
    }
}
