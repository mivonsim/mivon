//! Icarus reference oracle — differential EKSTERNAL vs Icarus Verilog.
//!
//! Prinsip: source self-contained (module + tb) dijalan di mivon AND
//! iverilog+vvp. Marker stream (`ASRT_...=<val>`, `_BAD=<val>`, `ERROR:..`)
//! diextract dari stdout keduaya. Mismatch marker = semantic divergence nyata
//! (hasil mivon beda dari reference independen) → Category::Differential.
//!
//! Semua tool jalan + marker sama = Category::Ok (hasil mivon DITERAPKABEL
//! vs reference eksternal — jawab q: "hasil sim CORRECT?" bukan jasta
//! no-crash/konsisten internal O4/O5).
//!
//! Iverilog cannot compile source (feature SV-only, mis. streaming `{<<{a}}`)
//! → Verdict::RefUnavailable — reference N/A, bukan bug mivon.

use crate::runner::{Kind, Outcome};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// Verdict differential reference vs iverilog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Marker stream mivon == marker stream iverilog (hasil reproduksi ref).
    Match,
    /// Marker stream beda → semantic divergence (hasil mivon != iverilog).
    Mismatch,
    /// Reference N/A: iverilog cannot compile, or vvp gagal/hang.
    RefUnavailable,
    /// mivon gagal run (panic/abort/crash/hang) — bug via O1.
    MivonBug,
}

#[derive(Debug, Clone)]
pub struct IcarusResult {
    pub verdict: Verdict,
    pub category: crate::Category,
    pub oracle: &'static str,
    pub detail: String,
}

/// Extract semua marker token cross-sim: `ASRT_...=<val>` atau `_BAD=<val>`.
///
/// Murat: scan batay `ASRT_` lalu `=<` ... `>`. Deterministic, tool-agnostic —
/// kedua iverilog AND mivon emisi $display payload identik, sehingga marker
/// stream adalah kontrak cross-sim yang wajib identik antar implementasi.
fn extract_markers(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        for token in find_marker_tokens(line) {
            if !token.is_empty() {
                out.push(token);
            }
        }
    }
    out
}

/// Scan satu baris untuk semua token `ASRT_...=<val>` (o `_BAD=<val>`).
fn find_marker_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        // cari `ASRT_` prefix (case-sensitive, kontrak cross-sim)
        if chars[i] == 'A'
            && i + 4 < chars.len()
            && chars[i + 1] == 'S'
            && chars[i + 2] == 'R'
            && chars[i + 3] == 'T'
            && chars[i + 4] == '_'
        {
            // scan sampai `=` (allow spasi/ident di antara)
            let mut j = i + 5;
            let mut eq = None;
            while j < chars.len()
                && chars[j] != ' '
                && chars[j] != '\t'
                && chars[j] != '='
                && chars[j] != '\r'
                && chars[j] != '\n'
            {
                j += 1;
            }
            if j < chars.len() && chars[j] == '=' {
                eq = Some(j);
            }
            if let Some(eqpos) = eq {
                // scan `>`
                let mut gt = eqpos + 1;
                while gt < chars.len() && chars[gt] != '>' && chars[gt] != '\r' && chars[gt] != '\n'
                {
                    gt += 1;
                }
                if gt < chars.len() && chars[gt] == '>' {
                    let token: String = chars[i..=gt].iter().collect();
                    if !token.is_empty() {
                        out.push(token);
                    }
                    i = gt + 1;
                    continue;
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak marker dari mivon Outcome — DUA STREAM:
/// - stdout: payload `$display`.
/// - stderr: baris SEVERITY saja (`Error:`/`Fatal:`/`Warning:`/`Info:`/
///   `error[`/`warning[`): `$error` mivon menulis ke STDERR sedangkan
///   iverilog vvp ke stdout → tanpa ini marker `_BAD` mivon tak terbaca →
///   mismatch ARTEFAK (fuzzer verify_bad_0039: simulasi sudah identik
///   `OUT_RST_BAD=<x>` dua-duanya, hanya stream beda). Snippet diagnostic
///   (`   34 │`, `┌─`) DIKECUALIKAN — kontennya memuat teks literal
///   `$display("ASRT_...=<...")` → false-positive (alasan stdout-only lama).
fn markers_of(out: &Outcome) -> Vec<String> {
    let mut ms = extract_markers(&out.stdout);
    for line in out.stderr.lines() {
        let t = line.trim_start();
        if t.starts_with("Error:")
            || t.starts_with("Fatal:")
            || t.starts_with("Warning:")
            || t.starts_with("Info:")
            || t.starts_with("error[")
            || t.starts_with("warning[")
        {
            ms.extend(extract_markers(t));
        }
    }
    ms
}

fn read_pipe<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> String {
    let mut buf = String::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_string(&mut buf);
    }
    buf
}

/// Hasil proses reference, dibedakan 3 hal yang lama tercampur jadi `None`.
///
/// `run_capture` lama balik `Option<i32>` dari `status.code()`. Di Unix itu
/// `None` untuk DUA sebab berbeda jauh: proses timeout (kita kill) DAN proses
/// mati karena sinyal (segfault/abort/OOM). `run_iverilog` memetakan keduanya
/// ke `RefUnavailable` "iverilog compile gagal" — artinya reference yang
/// RUSAK dilaporkan "reference tak tersedia", dan verdict bug hilang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefRun {
    /// Proses selesai dengan exit code `i32`.
    Exited(i32),
    /// Timeout — kita kill.
    TimedOut,
    /// Mati karena sinyal (bukan oleh timeout kita).
    Signaled(i32),
}

/// Jalankan command reference:timeout eksak (tanpa grace, berbeda dari
/// runner.rs yang 3×) + return klasifikasi 3-jenis di atas.
fn run_capture_classified(args: &[String], timeout_ms: u64) -> (RefRun, String, String) {
    let mut cmd = Command::new(args[0].clone());
    for a in &args[1..] {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let start = Instant::now();
    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                RefRun::Exited(-1),
                String::new(),
                format!("spawn gagal: {e}"),
            );
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_handle = std::thread::spawn(move || read_pipe(stdout));
    let err_handle = std::thread::spawn(move || read_pipe(stderr));

    let timeout = Duration::from_millis(timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let out = out_handle.join().unwrap_or_default();
    let err = err_handle.join().unwrap_or_default();

    let run = match status {
        Some(st) => match exit_status_run(&st) {
            Some(r) => r,
            // `Some(status)` tapi tanpa code = mati sinyal.
            None => RefRun::Signaled(status_signal(&st).unwrap_or(0)),
        },
        // `None` = kita kill saat timeout (atau try_wait error) → timeout.
        None => RefRun::TimedOut,
    };
    (run, out, err)
}

/// Exit code dari `ExitStatus`, atau `None` bila mati sinyal.
#[cfg(unix)]
fn exit_status_run(status: &std::process::ExitStatus) -> Option<RefRun> {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(c), _) => Some(RefRun::Exited(c)),
        (None, Some(s)) => Some(RefRun::Signaled(s)),
        (None, None) => None,
    }
}

#[cfg(not(unix))]
fn exit_status_run(status: &std::process::ExitStatus) -> Option<RefRun> {
    status.code().map(RefRun::Exited)
}

/// Nomor sinyal (unix) atau `None`.
#[cfg(unix)]
fn status_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn status_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// Cek availability iverilog binary — pub utk auto-verify default kampanye (main.rs).
pub fn iverilog_available() -> bool {
    let args = vec!["iverilog".to_string(), "-V".to_string()];
    let (_run, _o, err) = run_capture_classified(&args, 3000);
    !err.contains("not found") && !err.contains("No such file")
}

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Ok => "ok",
        Kind::CleanError => "clean_error",
        Kind::Panic => "panic",
        Kind::Abort => "abort",
        Kind::Crash(_) => "crash",
        Kind::Hang => "hang",
    }
}
/// Iverilog compile → vvp run, return (code, stdout, stderr, degraded).
/// `degraded` = compile lolos dgn "sorry:" (unsupported) → sim reference
/// mungkin menyimpang; pemanggil hanya menggugurkan MISMATCH (Match tetap sah).
fn run_iverilog(source: &str, timeout_ms: u64) -> (Option<i32>, String, String, bool) {
    let base = std::env::temp_dir().clone();
    let stem = format!(
        "mivonic_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let sv = base.join(format!("{stem}.sv"));
    let vvp = base.join(format!("{stem}.vvp"));
    let _ = std::fs::write(&sv, source);

    // 1. compile iverilog → vvp.
    let (c_run, _o, c_err) = run_capture_classified(
        &[
            "iverilog".to_string(),
            "-g2012".to_string(),
            "-gsupported-assertions".to_string(),
            "-o".to_string(),
            vvp.to_string_lossy().to_string(),
            sv.to_string_lossy().to_string(),
        ],
        timeout_ms,
    );
    let _ = std::fs::remove_file(&sv);
    // `degraded`: iverilog "sorry:" (unsupported) tapi compile diteruskan —
    // sim reference mungkin menyimpang; hanya gugurkan vonis MISMATCH
    // (kesepakatan tetap sah), bukan Match. Deferred init: semua arm match
    // selain Exited(0) return lebih dulu.
    let degraded;
    match c_run {
        // Compile GAGAL (exit != 0) → reference memang tak bisa pakai.
        RefRun::Exited(0) => {
            degraded = is_degraded_reference(&c_err);
        }
        RefRun::Exited(_) => return (None, String::new(), c_err, false),
        RefRun::TimedOut => {
            return (
                None,
                String::new(),
                format!("iverilog compile TIMEOUT > {timeout_ms}ms (reference tak pakai)"),
                false,
            )
        }
        // Reference CRASH (segfault/abort/OOM). Versi lama melaporkan ini
        // sama dengan "compile gagal" → verdict RefUnavailable, bug iverilog
        // hilang. Sekarang_surface sebagai pesan eksplisit TETAP N/A tapi
        // jelas ENGINE reference-nya rusak, bukan SV-nya.
        RefRun::Signaled(sig) => {
            return (
                None,
                String::new(),
                format!("iverilog ENGINE CRASH (sinyal {sig}) — reference rusak, bukan SV"),
                false,
            )
        }
    }

    // 2. jalankan vvp.
    let (v_run, out, v_err) = run_capture_classified(
        &["vvp".to_string(), vvp.to_string_lossy().to_string()],
        timeout_ms,
    );
    let _ = std::fs::remove_file(&vvp);
    match v_run {
        RefRun::Exited(code2) => (Some(code2), out, v_err, degraded),
        RefRun::TimedOut => {
            // vvp hang — reference not reliable
            (None, out, v_err, degraded)
        }
        RefRun::Signaled(sig) => (
            None,
            out,
            format!("vvp ENGINE CRASH (sinyal {sig}): {}", v_err),
            degraded,
        ),
    }
}
/// iverilog "sorry:" di stderr compile = konstruksi tak didukung namun
/// compile diteruskan → sim reference degraded (x palsu). Murni.
fn is_degraded_reference(compile_stderr: &str) -> bool {
    compile_stderr.contains("sorry:")
}

/// Injeksi marker differential otomatis utk source TANPA `ASRT_`.
///
/// verify_mutated melewatkan source tanpa marker (`skipped_no_marker` —
/// compare pasti vakum). Fungsi ini membuat source tanpa marker jadi
/// TERBANDINGKAN: compile in-process → ambil ≤12 signal logika top →
/// sisipkan `initial #1 $display("ASRT_AUTO_<n>=<%h>", <n>)` ke modul top.
/// Kedua tool (mivon + iverilog) jalan di source SAMA → marker stream
/// bermakna (bukan vakum). `None` bila: sudah ada marker, compile gagal,
/// tak ada signal cocok, atau blok modul tak ditemukan — pemanggil jatuh
/// ke perilaku lama. Injeksi selalu di-REVALIDASI via compile (surgery
/// teks aman by-construction).
#[allow(clippy::result_large_err)]
pub fn inject_auto_markers(source: &str) -> Option<String> {
    if source.contains("ASRT_") {
        return None;
    }
    let ir = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source))
        .ok()?
        .ok()?;
    // Kandidat: signal logika top (bukan string/real/synth-wire, ≤128b).
    let mut names: Vec<String> = Vec::new();
    for s in &ir.top.signals {
        if s.is_string || s.is_real || s.width == 0 || s.width > 128 {
            continue;
        }
        let n = s.name.as_str().to_string();
        if n.starts_with("__") || n.is_empty() {
            continue;
        }
        if !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || n.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            continue;
        }
        if names.contains(&n) {
            continue;
        }
        names.push(n);
        if names.len() >= 12 {
            break;
        }
    }
    if names.is_empty() {
        return None;
    }
    // Saring ke signal DRIVEN-bersih: undriven = `z` di iverilog vs `x`
    // di mivon (beda default by-design, lazy-zero) → banjir mismatch palsu
    // (temuan verify #19: 7/7 marker beda hanya z-vs-x). Penentu: sim
    // in-process singkat — signal yang NILAI AKHIRnya masih ber-X/Z
    // dikeluarkan (X-hunting tetap ranah oracle Suspicious, bukan icarus).
    // Guard ukuran: sim in-process tanpa watchdog → hanya source kecil.
    if source.len() <= 512_000 {
        if let Ok(Ok(final_sigs)) =
            std::panic::catch_unwind(|| mivon_api::simulate_signals(source, 100))
        {
            let clean: std::collections::HashSet<String> = final_sigs
                .iter()
                .filter(|(_, lv)| {
                    lv.bits
                        .iter()
                        .all(|b| *b == mivon_ir::LogicVal::Zero || *b == mivon_ir::LogicVal::One)
                })
                .map(|(n, _)| n.clone())
                .collect();
            names.retain(|n| clean.contains(n));
        }
    }
    if names.is_empty() {
        return None;
    }
    // Cari blok `module <top> ... endmodule` di teks.
    let top_name = ir.top.name.as_str();
    let lines: Vec<&str> = source.lines().collect();
    let start = lines.iter().position(|l| {
        let t = l.trim_start();
        if t.starts_with("//") {
            return false;
        }
        let Some(rest) = t.strip_prefix("module") else {
            return false;
        };
        // `module` diikuti spasi/`(`/`#` — bukan `modulexyz`.
        if !(rest.starts_with(char::is_whitespace)
            || rest.starts_with('(')
            || rest.starts_with('#'))
        {
            return false;
        }
        // Token nama pertama setelah `module`.
        let tok: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        tok == top_name || (tok.is_empty() && rest.contains(top_name))
    })?;
    let rel_end = lines[start..].iter().position(|l| {
        let t = l.trim_start();
        !t.starts_with("//") && t.starts_with("endmodule")
    })?;
    let end = start + rel_end;
    let mut block = String::from("  initial begin\n    #1;\n");
    for n in &names {
        block.push_str(&format!("    $display(\"ASRT_AUTO_{n}=<%h>\", {n});\n"));
    }
    block.push_str("  end\n");
    let mut injected = String::with_capacity(source.len() + block.len() + 16);
    for l in &lines[..end] {
        injected.push_str(l);
        injected.push('\n');
    }
    injected.push_str(&block);
    for l in &lines[end..] {
        injected.push_str(l);
        injected.push('\n');
    }
    // Revalidasi: hasil suntik harus tetap ter-compile.
    let ok = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(&injected))
        .ok()
        .and_then(|r| r.ok())
        .is_some();
    if ok {
        Some(injected)
    } else {
        None
    }
}

/// Differential reference vs Icarus: jawab "hasil sim mivon CORRECT (== iverilog)?".
/// Normalisasi default timescale: bila belum ada directive `timescale
/// SEBELUM deklarasi module/interface/package pertama, prepend
/// `` `timescale 1ns/1ps ``. Modul tanpa directive memakai default tool —
/// iverilog default (1s/1s) vs mivon default berbeda → mismatch PALSU
/// (verify_bad_0011: `#5` DUT efektif tak pernah berjalan di iverilog).
fn normalize_timescale(source: &str) -> std::borrow::Cow<'_, str> {
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with("`timescale") {
            return std::borrow::Cow::Borrowed(source);
        }
        if t.starts_with("module") || t.starts_with("interface") || t.starts_with("package") {
            break;
        }
    }
    let mut out = String::with_capacity(source.len() + 24);
    out.push_str("`timescale 1ns/1ps\n");
    out.push_str(source);
    std::borrow::Cow::Owned(out)
}

pub fn evaluate_icarus(source: &str, timeout_ms: u64) -> IcarusResult {
    if !iverilog_available() {
        return IcarusResult {
            verdict: Verdict::RefUnavailable,
            category: crate::Category::CleanError,
            oracle: "N/A",
            detail: "iverilog tidak terinstali — reference icarus unavailable".to_string(),
        };
    }

    // Normalisasi default timescale (lihat normalize_timescale) — hilangkan
    // mismatch palsu kelas verify_bad_0011 (DUT tanpa directive: default
    // 1s/1s iverilog vs default mivon).
    let normalized = normalize_timescale(source);
    let source = normalized.as_ref();

    // Injeksi marker otomatis di SATU titik: semua pemanggil (verify seed-pair,
    // verify-mutated, oracle sim) dapat differential bermakna atas source tanpa
    // marker. No-op murah bila `ASRT_` sudah ada (cek string dulu di dalam).
    let injected = inject_auto_markers(source);
    let source = injected.as_deref().unwrap_or(source);

    // 1. mivon run (black-box via runner)
    let mivon = crate::runner::run_file(source, timeout_ms);
    if mivon.kind != Kind::Ok {
        return IcarusResult {
            verdict: Verdict::MivonBug,
            category: match mivon.kind {
                Kind::Panic => crate::Category::Panic,
                Kind::Abort => crate::Category::Abort,
                Kind::Crash(_) => crate::Category::Panic,
                Kind::Hang => crate::Category::Hang,
                _ => crate::Category::CleanError,
            },
            oracle: "O1-no-crash",
            detail: format!(
                "mivon gagal run: {} — {}",
                kind_label(mivon.kind),
                mivon.stderr
            ),
        };
    }

    // 2. reference marker
    let (ref_code, ref_stdout, ref_stderr, degraded) = run_iverilog(source, timeout_ms);
    if ref_code.is_none() {
        let reason = if ref_stderr.contains("Streaming") || ref_stderr.is_empty() {
            "iverilog compile gagal (SV-only feature, mis. streaming)".to_string()
        } else {
            ref_stderr
                .lines()
                .next()
                .unwrap_or("compile gagal")
                .to_string()
        };
        return IcarusResult {
            verdict: Verdict::RefUnavailable,
            category: crate::Category::CleanError,
            oracle: "N/A",
            detail: format!("ref icarus N/A: {}", reason),
        };
    }

    let ref_markers = extract_markers(&ref_stdout);
    let mine = markers_of(&mivon);

    // MULTISET (sort): marker `$error` (stderr) & `$display` (stdout)
    // berasal dari dua stream terpisah — urutan lintas-stream tak teramati;
    // tanpa sort, kasus campuran selalu mismatch ARTEFAK (urutan beda
    // walau isi identik). Multiset tetap menangkap selisih NILAI
    // (kasus berulang `P=<65504>,<65506>,<0>,<0>` tetap dibandingkan utuh).
    let mut a = ref_markers.clone();
    a.sort();
    let mut b = mine.clone();
    b.sort();
    match classify_markers(&a, &b) {
        Verdict::Match => {
            return IcarusResult {
                verdict: Verdict::Match,
                category: crate::Category::Ok,
                oracle: "O5-differential",
                detail: format!(
                    "hasil sim mivon == iverilog ({} marker identik)",
                    ref_markers.len()
                ),
            };
        }
        Verdict::RefUnavailable => {
            return IcarusResult {
                verdict: Verdict::RefUnavailable,
                category: crate::Category::CleanError,
                oracle: "N/A",
                detail: "tidak ada marker ASRT_=<val> di kedua sisi — compare vakum, bukan Match"
                    .to_string(),
            };
        }
        Verdict::Mismatch => {}
        _ => {}
    }

    IcarusResult {
        verdict: if degraded {
            Verdict::RefUnavailable
        } else {
            Verdict::Mismatch
        },
        category: if degraded {
            crate::Category::CleanError
        } else {
            crate::Category::Differential
        },
        oracle: if degraded { "N/A" } else { "O5-differential" },
        detail: if degraded {
            format!(
                "ref icarus DEGRADED (sorry, sim tak tepercaya) — mismatch diabaikan:\n  ref : {}\n  mivon: {}",
                ref_markers.join(" "),
                mine.join(" ")
            )
        } else {
            format!(
                "SEMANTIC MISMATCH vs iverilog:\n  ref : {}\n  mivon: {}",
                ref_markers.join(" "),
                mine.join(" ")
            )
        },
    }
}

/// Klasifikasi dua marker stream (sudah multiset-sort) — LOGIKA MURNI.
///
/// Vakum (`a` dan `b` sama-sama kosong) = `RefUnavailable`, BUKAN `Match`:
/// marker `ASRT_X=ok` tanpa `<>` tak ter-extract, dan TB yang tak jalan
/// (top resolution salah) menghasilkan nol marker dua-duanya — dulu dilaporkan
/// `Match "0 marker identik"` = false negative (temuan triage verify_bad:
/// kasus `=ok`-only lolos sebagai match bermakna).
fn classify_markers(a: &[String], b: &[String]) -> Verdict {
    if a == b {
        if a.is_empty() {
            Verdict::RefUnavailable
        } else {
            Verdict::Match
        }
    } else {
        Verdict::Mismatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Normalisasi timescale: modul tanpa directive sebelum module pertama
    /// mendapat prepend `timescale 1ns/1ps`; yang sudah punya tidak diubah.
    #[test]
    fn timescale_normalized_when_missing_before_first_module() {
        let with = "`timescale 1ps/1ps\nmodule m; endmodule\n";
        assert!(
            matches!(normalize_timescale(with), std::borrow::Cow::Borrowed(_)),
            "sudah ada directive = tidak diubah"
        );
        let without = "// c\nmodule m;\n  initial #5;\nendmodule\n";
        let n = normalize_timescale(without);
        assert!(
            n.starts_with("`timescale 1ns/1ps\n// c\nmodule m;"),
            "prepend default: {n}"
        );
        // Directive SETELAH module pertama → tetap dinormalisasi (modul
        // sebelum directive memakai default yang sama utk kedua tool).
        let after = "module d; endmodule\n`timescale 1ns/1ps\nmodule tb; endmodule\n";
        assert!(matches!(
            normalize_timescale(after),
            std::borrow::Cow::Owned(_)
        ));
    }

    /// Injeksi marker otomatis: design tanpa ASRT_ jadi terbandingkan —
    /// suntik `initial #1 $display ASRT_AUTO_*` + tetap ter-compile.
    /// Sudah-bermarker dan garbage → None (perilaku lama). Undriven semua
    /// (filter driven) → None juga (z-vs-x by-design, bukan bug).
    #[test]
    fn inject_auto_markers_for_markerless_design() {
        let src = "module tiny(input logic clk, output logic [7:0] cnt);\n  logic [7:0] c = 0;\n  always @(posedge clk) c <= c + 1;\n  assign cnt = c;\nendmodule\nmodule tb;\n  logic clk = 0;\n  logic [7:0] cnt;\n  tiny dut(.clk(clk), .cnt(cnt));\n  always #5 clk = ~clk;\n  initial begin #50; $finish; end\nendmodule\n";
        let inj = inject_auto_markers(src).expect("design driven harus terinjeksi");
        assert!(inj.contains("ASRT_AUTO_"), "marker otomatis ada");
        assert!(inj.contains("$display"), "via $display");
        assert!(inj.contains("endmodule"), "blok modul utuh");
        // Hasil suntik valid: marker terekstrak oleh kontrak cross-sim.
        assert!(extract_markers("ASRT_AUTO_b=<1>")
            .iter()
            .any(|t| t == "ASRT_AUTO_b=<1>"));
    }

    #[test]
    fn inject_auto_markers_skips_undriven() {
        // Input undriven → semua X → tak ada yang driven → None.
        let src = "module tiny(input logic a, output logic b);\n  assign b = ~a;\nendmodule\n";
        assert!(inject_auto_markers(src).is_none());
    }

    #[test]
    fn inject_auto_markers_skips_marked_and_garbage() {
        assert!(
            inject_auto_markers("module m; initial $display(\"ASRT_X=<1>\"); endmodule").is_none()
        );
        assert!(inject_auto_markers("@@@ {{{ ;;;").is_none());
        assert!(inject_auto_markers("").is_none());
    }

    /// Reference degraded (iverilog "sorry:") bukan Mismatch (bug_0018/0028:
    /// constant selects → x palsu padahal mivon benar 10/10 pass).
    #[test]
    fn degraded_reference_detected() {
        assert!(is_degraded_reference(
            "file.sv:12: sorry: constant selects in always_* processes are not currently supported"
        ));
        assert!(!is_degraded_reference(""));
        assert!(!is_degraded_reference("error: syntax error"));
    }

    /// Marker extraction: cross-sim token stream. Marker MUTLAK punya `=<val>`
    /// (`ASRT_...=<val>`) — token tanpa `=` ("ASRT_START tb") bukan bagian
    /// kontrak cross-sim dan tidak di-extract.
    #[test]
    fn extract_asrt_markers() {
        let s = "ASRT_START tb\ncount=0\nASRT_SUM=<7>\nASRT_BAD=<9>\nplain\n";
        let m = extract_markers(s);
        assert!(!m.iter().any(|t| t == "ASRT_START tb"));
        assert!(m.iter().any(|t| t == "ASRT_SUM=<7>"));
        assert!(m.iter().any(|t| t == "ASRT_BAD=<9>"));
        assert!(!m.iter().any(|t| t == "count=0"));
    }

    /// REGRESI fuzzer: `run_capture` lama balik `Option<i32>` dan memetakan
    /// timeout DAN mati-sinyal ke `None` yang sama → `run_iverilog`
    /// melaporkan keduanya "compile gagal → RefUnavailable". Sekarang 3
    /// sebab dipisah, dan yang penting: reference yang CRASH harus punya
    /// pesan berbeda dari SV yang memang tak didukung.
    #[test]
    fn ref_run_distinguishes_exit_timeout_and_signal() {
        // Exit 0 → normal, reference bisa dipakai.
        assert_eq!(RefRun::Exited(0), RefRun::Exited(0));
        // Timeout dan crash adalah nilai BERBEDA (bukan `None` yang sama).
        assert_ne!(RefRun::TimedOut, RefRun::Signaled(11));
        assert_ne!(RefRun::Exited(1), RefRun::Signaled(11));
        // Tidak ada `None` di enum: tak ada lagi jalan yang bisa
        // administering "reference rusak" sebagai "reference tak tersedia".
        assert_ne!(RefRun::Exited(1), RefRun::TimedOut);
    }

    /// REGRESI fuzzer: proses reference yang crash harus `Signaled`, bukan
    /// `None`/`TimedOut`. Versi lama (`and_then(|s| s.code())`) couldn't
    /// membedakannya dari timeout → reference rusak dilaporkan "tak tersedia".
    #[cfg(unix)]
    #[test]
    fn ref_process_crash_is_signaled_not_timeout() {
        let args = vec![
            "sh".to_string(),
            "-c".to_string(),
            "kill -SEGV $$".to_string(),
        ];
        let (run, _out, _err) = run_capture_classified(&args, 10_000);
        assert!(
            matches!(run, RefRun::Signaled(_)),
            "crash harus Signaled, dapat {run:?}"
        );
    }

    /// REGRESI fuzzer: proses yang tak berujung = `TimedOut` (bukan crash).
    #[cfg(unix)]
    #[test]
    fn ref_process_infinite_loop_is_timed_out() {
        let args = vec![
            "sh".to_string(),
            "-c".to_string(),
            "while :; do :; done".to_string(),
        ];
        let (run, _out, _err) = run_capture_classified(&args, 300);
        assert_eq!(run, RefRun::TimedOut, "loop tak-berujung = timeout");
    }

    /// Different marker streams → detect via equality (unit).
    #[test]
    fn detect_mismatch_streams() {
        let expected = vec!["ASRT_SUM=<7>".to_string(), "ASRT_END tb".to_string()];
        let mine = vec!["ASRT_SUM=<7>".to_string(), "ASRT_END tb2".to_string()];
        assert!(expected != mine);
    }

    /// Marker `=ok` tanpa `<>` tak ter-extract (kontrak `=<val>`): TB yang
    /// hanya mencetak `ASRT_X=ok` menghasilkan NOL marker — pemicu vakum.
    /// Temuan triage verify_bad_0052 (mivon benar semua `=ok`, iverilog x).
    #[test]
    fn extract_ignores_ok_without_brackets() {
        let s = "ASRT_START tb\nASRT_UNSIGNED=ok\nASRT_MATRIX=ok\nASRT_END tb\n";
        assert!(extract_markers(s).is_empty(), "=ok tanpa <> bukan marker");
    }

    /// REGRESI fuzz-verify: compare vakum (nol marker dua sisi) = N/A,
    /// bukan Match. Dulu `a == b` kosong → Match "0 marker identik".
    #[test]
    fn classify_vacuous_is_not_a_match() {
        let empty: Vec<String> = vec![];
        assert_eq!(
            classify_markers(&empty, &empty),
            Verdict::RefUnavailable,
            "vakum bukan Match"
        );
        let a = vec!["ASRT_Q=<15>".to_string()];
        let b = vec!["ASRT_Q=<15>".to_string()];
        assert_eq!(classify_markers(&a, &b), Verdict::Match);
        let c = vec!["ASRT_Q=<16>".to_string()];
        assert_eq!(classify_markers(&a, &c), Verdict::Mismatch);
        // Satu sisi kosong, satu berisi = mismatch (TB tak jalan satu sisi).
        assert_eq!(classify_markers(&empty, &b), Verdict::Mismatch);
    }
}
