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

/// Jalankan command (list arg) avec timeout + drain pipe (mirror runner.spawn).
fn run_capture(args: &[String], timeout_ms: u64) -> (Option<i32>, String, String) {
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
            return (None, String::new(), format!("spawn gagal: {e}"));
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
            break None; // hang
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let out = out_handle.join().unwrap_or_default();
    let err = err_handle.join().unwrap_or_default();
    let code = status.and_then(|s| s.code());
    (code, out, err)
}

fn read_pipe<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> String {
    let mut buf = String::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_string(&mut buf);
    }
    buf
}

/// Cek availability iverilog binary — pub utk auto-verify default kampanye (main.rs).
pub fn iverilog_available() -> bool {
    let (_c, _o, err) = run_capture(&["iverilog".to_string(), "-V".to_string()], 3000);
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
/// Iverilog compile → vvp run, return (code, stdout, stderr).
fn run_iverilog(source: &str, timeout_ms: u64) -> (Option<i32>, String, String) {
    let base = std::env::temp_dir().clone();
    let stem = format!(
        "mivonic_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let sv = base.join(format!("{stem}.sv"));
    let vvp = base.join(format!("{stem}.vvp"));
    let _ = std::fs::write(&sv, source);

    let (code, _o, err) = run_capture(
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
    // compile gagal → code != 0 → reference N/A
    if code.unwrap_or(1) != 0 {
        return (None, String::new(), err);
    }
    let (code2, out, err2) = run_capture(
        &["vvp".to_string(), vvp.to_string_lossy().to_string()],
        timeout_ms,
    );
    let _ = std::fs::remove_file(&vvp);
    if code2.is_none() {
        // vvp hang — reference not reliable
        return (None, out, err2);
    }
    (code2, out, err2)
}
/// Differential reference vs Icarus: jawab "hasil sim mivon CORRECT (== iverilog)?".
pub fn evaluate_icarus(source: &str, timeout_ms: u64) -> IcarusResult {
    if !iverilog_available() {
        return IcarusResult {
            verdict: Verdict::RefUnavailable,
            category: crate::Category::CleanError,
            oracle: "N/A",
            detail: "iverilog tidak terinstali — reference icarus unavailable".to_string(),
        };
    }

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
    let (ref_code, ref_stdout, ref_stderr) = run_iverilog(source, timeout_ms);
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
    if a == b {
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

    IcarusResult {
        verdict: Verdict::Mismatch,
        category: crate::Category::Differential,
        oracle: "O5-differential",
        detail: format!(
            "SEMANTIC MISMATCH vs iverilog:\n  ref : {}\n  mivon: {}",
            ref_markers.join(" "),
            mine.join(" ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Different marker streams → detect via equality (unit).
    #[test]
    fn detect_mismatch_streams() {
        let expected = vec!["ASRT_SUM=<7>".to_string(), "ASRT_END tb".to_string()];
        let mine = vec!["ASRT_SUM=<7>".to_string(), "ASRT_END tb2".to_string()];
        assert!(expected != mine);
    }
}
