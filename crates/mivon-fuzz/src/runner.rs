//! Runner subprocess: jalankan binary `mivon` sebagai black-box.
//!
//! Miller 1990-style: timeout via kill, pipe drain via 2 reader threads
//! (cegah false-hang pipe-full 64KB).

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Ok,
    CleanError,
    Panic,
    Abort,
    Crash(i32),
    Hang,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub kind: Kind,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub ms: u128,
}

// Isolasi MICD per-case TANPA menyentuh env process-global.
//
// Dulu `with_micd_isolated` memakai `std::env::set_var` — RACE: worker
// watchdog yang dibiarkan hidup setelah timeout masih membaca
// `MIVON_MICD_DIR` saat case berikut mengganti env (dan `setenv`
// concurrent = hazard data race). Thread-local hanya terbaca thread case
// yang sedang berjalan; worker bocor punya thread-local sendiri (None)
// sehingga tak terpengaruh. Dipasang ke subprocess via `Command::env`.
thread_local! {
    static MICD_OVERRIDE: std::cell::RefCell<Option<std::ffi::OsString>> =
        const { std::cell::RefCell::new(None) };
}

/// Set/isolasi MICD dir untuk case berikut di thread INI. Return fungsi
/// restore (panggil setelah selesai).
pub fn set_micd_override(dir: Option<&std::path::Path>) {
    MICD_OVERRIDE.with(|c| {
        *c.borrow_mut() = dir.map(|d| d.as_os_str().to_os_string());
    });
}

fn apply_micd_env(cmd: &mut Command) {
    if let Some(v) = MICD_OVERRIDE.with(|c| c.borrow().clone()) {
        cmd.env("MIVON_MICD_DIR", v);
    }
}

/// Jalankan `mivon <file>` dengan timeout.
pub fn run_file(source: &str, timeout_ms: u64) -> Outcome {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "mivonfz_{}_{}.sv",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::write(&path, source).is_err() {
        return Outcome {
            kind: Kind::CleanError,
            code: None,
            stdout: String::new(),
            stderr: "gagal tulis file temp".to_string(),
            ms: 0,
        };
    }

    let bin = find_mivon();
    let mut cmd = Command::new(&bin);
    cmd.arg(&path)
        .arg("-T")
        .arg("1000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    apply_micd_env(&mut cmd);

    let outcome = spawn(&mut cmd, timeout_ms);
    let _ = std::fs::remove_file(&path);
    outcome
}

/// Jalankan `mivon <args...>` di cwd temp (target Cli).
pub fn run_args(args: &[String], timeout_ms: u64) -> Outcome {
    let bin = find_mivon();
    let mut cmd = Command::new(&bin);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    apply_micd_env(&mut cmd);

    spawn(&mut cmd, timeout_ms)
}

/// Cari binary mivon: env MIVON_BIN, release (LTO, cepat), debug, atau workspace.
pub fn find_mivon() -> String {
    if let Ok(bin) = std::env::var("MIVON_BIN") {
        return bin;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // current_exe = target/{debug,release}/mivon-fuzz → dir = target/{debug,release}.
            // RELEASE dulu (LTO cepat), lalu debug, lalu sibling same-dir.
            if let Some(target) = dir.parent() {
                // target/release dan target/debug adalah PASANGAN dari dir ini
                let rel = target.join("release/mivon");
                let dbg = target.join("debug/mivon");
                if rel.exists() {
                    return rel.to_string_lossy().to_string();
                }
                if dbg.exists() {
                    return dbg.to_string_lossy().to_string();
                }
            }
            // Fallback same-dir (binary diinstall berdampingan)
            let cand = dir.join("mivon");
            if cand.exists() {
                return cand.to_string_lossy().to_string();
            }
        }
    }
    // Workspace root walk (cari Cargo.toml dengan [workspace]),
    // RELEASE dulu (LTO — startup/eksekusi jauh lebih cepat).
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let release = dir.join("target/release/mivon");
        let debug = dir.join("target/debug/mivon");
        if release.exists() {
            return release.to_string_lossy().to_string();
        }
        if debug.exists() {
            return debug.to_string_lossy().to_string();
        }
        if !dir.pop() {
            break;
        }
    }
    "mivon".to_string()
}

fn spawn(cmd: &mut Command, timeout_ms: u64) -> Outcome {
    let start = Instant::now();
    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Outcome {
                kind: Kind::CleanError,
                code: None,
                stdout: String::new(),
                stderr: format!("spawn gagal: {e}"),
                ms: start.elapsed().as_millis(),
            };
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Drain pipe via reader thread — cegah pipe-full false-hang
    let out_handle = std::thread::spawn(move || read_pipe(stdout));
    let err_handle = std::thread::spawn(move || read_pipe(stderr));

    // Grace 3×: kasus SLOW (source besar 1.5MB/220 module = 13s terukur,
    // load mesin tinggi) dulu di-kill tepat di timeout → dilaporkan `Hang`
    // → dihitung `is_bug()` → noise kampanye (cli hang=49/1500, sim 159/1500
    // semuanya replay-ok saat mesin sepi). Kill hanya melewati 3× timeout =
    // loop tak-berujung ASLI; yang selesai di antara → Kind::Ok + `ms` besar
    // (pemanggil bisa menandai Slow lewat `ms > timeout`).
    let kill_limit = Duration::from_millis(timeout_ms.saturating_mul(3));
    // `killed_by_us` WAJIB: `status == None` punya DUA sebab — (a) kita
    // sendiri yang kill saat timeout, atau (b) `try_wait` error. Tanpa
    // penanda, (a) dan (b) tak bisa dibedakan. Lihat klasifikasi di bawah.
    let mut killed_by_us = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if start.elapsed() > kill_limit {
            killed_by_us = true;
            let _ = child.kill();
            let _ = child.wait();
            break None; // hang asli (melebihi grace 3×)
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    let ms = start.elapsed().as_millis();

    let (kind, code) = match status {
        Some(status) => classify_status(&status, &stderr),
        // Tidak ada status + kita yang kill = hang (loop tak-berujung).
        None if killed_by_us => (Kind::Hang, None),
        // Tidak ada status + `try_wait` gagal = tak bisacertainty → error
        // bersih (bukan hang: tidak ada bukti proses tak selesai).
        None => (Kind::CleanError, None),
    };

    Outcome {
        kind,
        code,
        stdout,
        stderr,
        ms,
    }
}

fn read_pipe<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> String {
    let mut buf = String::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_string(&mut buf);
    }
    buf
}

/// Klasifikasi dari `ExitStatus` LENGKAP (bukan cuma exit code).
///
/// Dua kelas bug yang dulu TERHILANG dan sekarang jadi bug nyata:
///
/// 1. **Mati karena sinyal (SIGSEGV/SIGABRT/SIGILL/SIGFPE/SIGKILL)**.
///    Di Unix `ExitStatus::code()` bernilai `None` **tepat** ketika proses
///    dibunuh sinyal — BUKAN saat exit normal. `classify(None, ..)` lama
///    mengembalikan `CleanError`, jadi segfault/abort/OOM tak pernah
///    terklasifikasi: lengan `Crash(139)`/`Abort(132,134)` di `classify`
///    praktis mati kode (shell memang reports 128+signal, tapi proses
///    yang di-`wait` langsung dari Rust tidak punya shell di depannya).
///    Inilah kelas bug yang justru paling penting bagi fuzzer — crash
///    memory-safety — dan sebelumnya 100% tak terlihat pada 7 target
///    subprocess (sim/cli/vcd/sdf/micd/synth/astdiff).
/// 2. **Exit 1 + teks panic.** Lengan lama `1 if !contains("panic")`
///    berarti: kalau stderr memuat "panic", TIDAK ada lengan yang cocok →
///    jatuh ke `_ => CleanError`. Guard-nya no-op dan justru salah: panic
///    dengan exit 1 terklasifikasi sebagai error bersih.
///
/// Konvensi exit code shell dipakai agar `detail` tetap terbaca manusia:
/// `128 + sinyal`.
#[cfg(unix)]
fn classify_status(status: &std::process::ExitStatus, stderr: &str) -> (Kind, Option<i32>) {
    use std::os::unix::process::ExitStatusExt;
    if let Some(sig) = status.signal() {
        let code = 128 + sig;
        return match sig {
            // SIGABRT (6): abort(), double-panic, `panic = "abort"`.
            SIGABRT => (Kind::Abort, Some(code)),
            // SIGKILL (9): biasanya OOM-killer saat cases besar + thread
            // watchdog bocor menumpuk → memory blowup = bug juga.
            SIGKILL => (Kind::Crash(code), Some(code)),
            // SIGSEGV(11)/SIGBUS(7)/SIGILL(4)/SIGFPE(8): crash memory-safety
            // atau stack overflow (parser/elaborator rekursif).
            _ => (Kind::Crash(code), Some(code)),
        };
    }
    classify(status.code(), stderr)
}

/// Non-unix: tak ada `signal()`, cukup exit code biasa.
#[cfg(not(unix))]
fn classify_status(status: &std::process::ExitStatus, stderr: &str) -> (Kind, Option<i32>) {
    classify(status.code(), stderr)
}

/// Nomor sinyal POSIX sebagai konstanta (tanpa dependensi `libc` crate —
/// crate fuzz hanya punya 4 optional dep dan harus tetap ringan).
///
/// `const` (bukan `fn`) supaya bisa dipakai sebagai POLA `match`.
#[cfg(unix)]
const SIGABRT: i32 = 6;
#[cfg(unix)]
const SIGKILL: i32 = 9;

fn classify(code: Option<i32>, stderr: &str) -> (Kind, Option<i32>) {
    let Some(code) = code else {
        return (Kind::CleanError, None);
    };
    match code {
        0 => (Kind::Ok, Some(0)),
        // mivon CLI pakai exit code 1 untuk error bersih. Tapi kalau stderr
        // memuat teks panic yang sah, itu BUG — lengan eksplisit setelah
        // guard supaya `1` + panic tidak jatuh ke `_ => CleanError`.
        1 if is_panic_text(stderr) => (Kind::Panic, Some(1)),
        1 => (Kind::CleanError, Some(1)),
        101 => (Kind::Panic, Some(101)),
        132 | 134 => (Kind::Abort, Some(code)),
        139 => (Kind::Crash(139), Some(139)),
        c if c >= 128 => (Kind::Crash(c), Some(c)),
        _ => (Kind::CleanError, Some(code)),
    }
}

/// Deteksi teks panic Rust di stderr.
///
/// PENTING: pesan panic Rust = `panicked at src/...` — SELALU berisi
/// `panicked at` (atau `RUST_BACKTRACE`). Kata `panic` polos bisa muncul
/// di output normal ("no panic in this run") sehingga memakainya sebagai
/// guard = false positive jadi bug palsu.
fn is_panic_text(stderr: &str) -> bool {
    stderr.contains("panicked at") || stderr.contains("RUST_BACKTRACE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_exit_codes() {
        assert_eq!(classify(Some(0), "").0, Kind::Ok);
        assert_eq!(classify(Some(1), "error: syntax").0, Kind::CleanError);
        assert_eq!(classify(Some(101), "panicked at").0, Kind::Panic);
        assert_eq!(classify(Some(139), "").0, Kind::Crash(139));
        assert_eq!(classify(None, "").0, Kind::CleanError);
    }

    #[test]
    fn find_mivon_returns_something() {
        assert!(!find_mivon().is_empty());
    }

    /// REGRESI fuzzer: `1 if !stderr.contains("panic")` dulu membuat panic
    /// ber-exit-1 jatuh ke `_ => CleanError` (guard no-op + salah substring:
    /// pesan panic Rust = `panicked at`, bukan `panic`).
    #[test]
    fn classify_exit_1_with_panic_text_is_panic_not_clean_error() {
        assert_eq!(
            classify(Some(1), "panicked at src/x.rs:5:9").0,
            Kind::Panic
        );
        assert_eq!(
            classify(Some(1), "thread 'main' panicked at 1:1\nRUST_BACKTRACE").0,
            Kind::Panic
        );
        // Tanpa teks panic yang sah → error bersih (perilaku dipertahankan).
        assert_eq!(classify(Some(1), "error: syntax di 3:2").0, Kind::CleanError);
        // Kata `panic` polos BUKAN bukti panic (false positive jadi bug palsu).
        assert_eq!(classify(Some(1), "no panic detected").0, Kind::CleanError);
    }

    /// REGRESI fuzzer: proses mati karena sinyal punya `code() == None`.
    /// Versi lama memetakan itu ke `CleanError` → segfault/abort/OOM tak
    /// pernah jadi bug. Sekarang harus `Crash`/`Abort`.
    #[cfg(unix)]
    #[test]
    fn classify_signal_death_is_crash_not_clean_error() {
        use std::os::unix::process::ExitStatusExt;
        // SIGSEGV (11) → shell exit 139.
        let segv = std::process::ExitStatus::from_raw(11);
        assert_eq!(classify_status(&segv, "").0, Kind::Crash(139));
        // SIGABRT (6) → shell exit 134.
        let abrt = std::process::ExitStatus::from_raw(6);
        assert_eq!(classify_status(&abrt, "").0, Kind::Abort);
        // SIGKILL (9) → OOM-killer → shell exit 137.
        let kill = std::process::ExitStatus::from_raw(9);
        assert_eq!(classify_status(&kill, "").0, Kind::Crash(137));
        // Exit normal tetap lewat jalur `classify` (tanpa sinyal).
        let ok = std::process::ExitStatus::from_raw(0);
        assert_eq!(classify_status(&ok, "").0, Kind::Ok);
    }

    /// REGRESI fuzzer: `ExitStatus::from_raw(0)` di unix = exit 0 (bukan
    /// sinyal). Jaga test `from_raw_zero_is_not_a_signal` tetap valid dan
    /// test sinyal tidak salah arti karena encoding `from_raw`.
    #[cfg(unix)]
    #[test]
    fn from_raw_zero_is_not_a_signal() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            std::process::ExitStatus::from_raw(0).signal(),
            None,
            "exit 0 bukan sinyal"
        );
    }

    /// REGRESI fuzzer: subprocess yang TIMEOUT (kita kill) = Hang, BUKAN
    /// crash. Penanda `killed_by_us` harus memisahkan dua sebab `None`.
    ///
    /// Pakai `spawn` langsung (bukan `run_args` yang argv[0]-nya selalu
    /// `find_mivon()`, bukan argumen yang diberikan pemanggil).
    #[test]
    fn spawn_timeout_is_hang_not_crash() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 5")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // timeout 300ms → kill_limit 900ms < 5s → pasti di-kill oleh kita.
        let out = spawn(&mut cmd, 300);
        assert_eq!(out.kind, Kind::Hang, "timeout harus Hang, bukan crash");
        assert_eq!(out.code, None);
    }

    /// REGRESI fuzzer: proses yang MATI KARENA SINYAL (bukan oleh timeout
    /// kita) harus `Crash`, bukan `CleanError`. Sebelumnya `code()==None`
    /// → CleanError sehingga segfault/abort/OOM 100% tak terlihat.
    #[test]
    fn spawn_signal_death_is_crash() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            // SIGSEGV: dd ke alamat tak-terpetakan, atau `kill -SEGV $$`.
            .arg("kill -SEGV $$")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // timeout panjang supaya proses SAMPAI mati sinyal, bukan di-kill timeout.
        let out = spawn(&mut cmd, 10_000);
        assert!(
            matches!(out.kind, Kind::Crash(_)),
            "SIGSEGV harus Crash, dapat {:?}",
            out.kind
        );
        assert_eq!(out.code, Some(139), "konvensi shell 128+11");
    }
}
