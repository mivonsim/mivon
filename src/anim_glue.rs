//! Glue animasi pipeline untuk `main.rs`.
//!
//! Semua helper di sini hanya meneruskan panggilan ke `PipelineAnimator`
//! bila animasi aktif — tidak ada logika pipeline di file ini.

use crate::cli::Cli;
use crate::emit_diags;
use mivon_core::animasi::{Phase, PipelineAnimator};

/// Apakah animasi pipeline aktif (menggambar di terminal).
pub(crate) fn anim_active(anim: &Option<PipelineAnimator>) -> bool {
    anim.as_ref().map(|a| a.is_active()).unwrap_or(false)
}

/// Kapan animasi pipeline diizinkan: bukan quiet, bukan mode debug/step,
/// bukan compile-only/lazy (output verbose akan merusak area animasi).
/// `MIVON_NO_ANIM` menonaktifkan animasi paksa (dipakai saat profiling/debug
/// dengan gdb/scripting — area animasi bisa mengganggu output dan thread render).
pub(crate) fn anim_enabled(cli: &Cli) -> bool {
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
pub(crate) fn anim_phase_running(anim: &Option<PipelineAnimator>, phase: Phase) {
    if let Some(a) = anim.as_ref() {
        a.phase_running(phase);
    }
}

pub(crate) fn anim_phase_done(anim: &Option<PipelineAnimator>, phase: Phase) {
    if let Some(a) = anim.as_ref() {
        a.phase_done(phase);
    }
}

pub(crate) fn anim_set_files(anim: &Option<PipelineAnimator>, total: u64, done: u64) {
    if let Some(a) = anim.as_ref() {
        a.set_files(total, done);
    }
}

pub(crate) fn anim_set_modules(anim: &Option<PipelineAnimator>, n: u64) {
    if let Some(a) = anim.as_ref() {
        a.set_modules(n);
    }
}

pub(crate) fn anim_finish(anim: &mut Option<PipelineAnimator>, ok: bool, errors: usize, warnings: usize) {
    if let Some(a) = anim.as_mut() {
        a.finish(ok, errors, warnings);
    }
}

pub(crate) fn anim_abort(anim: &mut Option<PipelineAnimator>, errors: usize, warnings: usize) {
    if let Some(a) = anim.as_mut() {
        a.abort(errors, warnings);
    }
}

/// Log fase bergaya Cargo: baris status dibersihkan dulu (bila aktif), pesan
/// dicetak sebagai baris tersendiri DI ATAS status, lalu status digambar ulang
/// di baris berikutnya. Non-aktif → eprintln biasa.
pub(crate) fn anim_log(anim: &Option<PipelineAnimator>, msg: &str) {
    match anim.as_ref() {
        Some(a) if a.is_active() => a.suspend_print(|| eprintln!("{}", msg)),
        _ => eprintln!("{}", msg),
    }
}

/// Wrapper `emit_diags` yang aman terhadap animasi aktif — diagnostik multi-baris
/// dicetak saat render dijeda, lalu baris status digambar ulang.
pub(crate) fn emit_diags_anim(
    anim: &Option<PipelineAnimator>,
    diags: &[mivon_core::diagnostics::diagnostic::Diagnostic],
) {
    match anim.as_ref() {
        Some(a) if a.is_active() => a.suspend_print(|| emit_diags(diags)),
        _ => emit_diags(diags),
    }
}

/// Hitung jumlah file per ekstensi `[sv, svh, v, vh]` lalu pasang ke animasi
/// (tampil di baris idle: `Compiling 128 file · sv 112 · svh 14`).
pub(crate) fn anim_set_exts<P: AsRef<std::path::Path>>(anim: &Option<PipelineAnimator>, files: &[P]) {
    if let Some(a) = anim.as_ref() {
        let mut exts = [0u64; 4];
        for f in files {
            match f
                .as_ref()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
            {
                Some(ref e) if e == "sv" => exts[0] += 1,
                Some(ref e) if e == "svh" => exts[1] += 1,
                Some(ref e) if e == "v" => exts[2] += 1,
                Some(ref e) if e == "vh" => exts[3] += 1,
                _ => {}
            }
        }
        a.set_ext_counts(exts);
    }
}

/// Pasang top module ke baris status (subjek fase ELA).
pub(crate) fn anim_set_top(anim: &Option<PipelineAnimator>, top: &str) {
    if let Some(a) = anim.as_ref() {
        a.set_top(top);
    }
}

/// Pasang jumlah file cache-hit (MICD) — tampil sebagai `N cached`.
pub(crate) fn anim_set_cached(anim: &Option<PipelineAnimator>, n: u64) {
    if let Some(a) = anim.as_ref() {
        a.set_cached(n);
    }
}
