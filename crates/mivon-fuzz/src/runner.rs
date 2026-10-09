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
    /// Selesai SETELAH ≥1 perpanjangan deadline (lambat-berprogres).
    /// Pemanggil memetakan Ok+slow → `Slow` (bukan `Ok` senyap) agar
    /// kampanye melihat biaya waktu; Hang murni (tanpa progres) tetap Hang.
    pub slow: bool,
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
            slow: false,
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
                slow: false,
            };
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Drain pipe via reader thread — cegah pipe-full false-hang. Reader
    // mencatat PROGRES (byte masuk): bedakan proses LAMBAT-berprogres dari
    // SPIN senyap. Wall-time murni tak bisa bedakan keduanya (terbukti:
    // kampanye asing 10k-case di mesin sama membuat kasus 1.2MB yang sehat
    // terlihat "hang" di timeout tetap).
    let progress = std::sync::Arc::new(PipeProgress::default());
    let out_handle = {
        let progress = std::sync::Arc::clone(&progress);
        std::thread::spawn(move || drain_pipe(stdout, &progress, true))
    };
    let err_handle = {
        let progress = std::sync::Arc::clone(&progress);
        std::thread::spawn(move || drain_pipe(stderr, &progress, false))
    };

    // Deadline ADAPTIF berbasis progres (bukan wall-time tetap):
    // - Basis 3× timeout (grace lama: kasus SLOW 13s terukur dulu di-kill
    //   tepat di timeout → `Hang` palsu → noise kampanye).
    // - Lewat basis TANPA progres byte baru sejak deadline sebelumnya →
    //   kill → `Hang` (spin senyap / loop tak-berujung).
    // - ADA progres → perpanjang +1× timeout, maks MAX_EXTENSIONS kali
    //   (total ≤6×, sejajar budget in-process sim). Selesai setelah
    //   perpanjangan → `slow: true` (lambat, bukan hang).
    // Batas jujur: proses senyap TAPI bekerja (elab besar tanpa output)
    // tak terbedakan dari spin oleh pengamat luar (halting problem) —
    // untuk itu jaring kedua tetap ada (replay-tenang auto-dismiss Hang
    // di run_single + kategori Slow bukan bug).
    let mut deadline = start + Duration::from_millis(timeout_ms.saturating_mul(3));
    let mut extensions = 0u32;
    let mut bytes_at_deadline = 0u64;
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
        if Instant::now() > deadline {
            let now_bytes = progress.bytes.load(std::sync::atomic::Ordering::Relaxed);
            if should_extend_deadline(extensions, now_bytes, bytes_at_deadline) {
                bytes_at_deadline = now_bytes;
                extensions += 1;
                deadline += Duration::from_millis(timeout_ms);
            } else {
                let _ = child.kill();
                // RAS: proses bisa exit NORMAL di antara `try_wait()` dan
                // `kill()` (kill lalu ESRCH). Membuang status hasil `wait()` =
                // laporkan `Hang` padahal proses selesai tepat di batas → noise
                // palsu kelas yang grace 3× ini ada untuk dihilangkan.
                // Ambil statusnya: kalau mati karena SIGKILL (kill kita) →
                // hang; kalau exit normal → proses memang selesai, pakai status.
                let status = match child.wait() {
                    Ok(st) if is_killed_by_us(&st) => None,
                    Ok(st) => Some(st),
                    Err(_) => None,
                };
                killed_by_us = status.is_none();
                break status;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let (stdout, stderr) = join_pipes(out_handle, err_handle, &progress);
    let ms = start.elapsed().as_millis();

    let (kind, code) = match status {
        Some(status) => classify_status(&status, &stderr),
        // Tidak ada status + kita yang kill = hang (loop tak-berujung).
        None if killed_by_us => (Kind::Hang, None),
        // Tidak ada status + `try_wait` gagal = tak bisacertainty → error
        // bersih (bukan hang: tidak ada bukti proses tak selesai).
        None => (Kind::CleanError, None),
    };

    // Gate Ok: proses yang di-kill (Hang/Crash) tak boleh tandai slow
    // walau sempat berprogres (burst-lalu-spin) — kalau tidak Hang
    // bocor jadi Slow non-bug di pemanggil.
    let slow = extensions > 0 && matches!(kind, Kind::Ok);
    Outcome {
        kind,
        code,
        stdout,
        stderr,
        ms,
        slow,
    }
}

/// Batas perpanjangan deadline progresif: basis 3× + maks 3× tambahan =
/// total ≤6× timeout (sejajar budget in-process sim).
const MAX_PROGRESS_EXTENSIONS: u32 = 3;

/// Keputusan murni deadline adaptif: perpanjang hanya bila ada byte progres
/// baru sejak deadline sebelumnya DAN di bawah cap. Murni (tanpa subprocess
/// / timing) agar logika inti teruji deterministik.
fn should_extend_deadline(extensions: u32, bytes_now: u64, bytes_at_deadline: u64) -> bool {
    extensions < MAX_PROGRESS_EXTENSIONS && bytes_now > bytes_at_deadline
}

/// Progres pipe lintas thread: buffer + hitung byte (tanda hidup proses).
#[derive(Default)]
struct PipeProgress {
    out: std::sync::Mutex<String>,
    err: std::sync::Mutex<String>,
    bytes: std::sync::atomic::AtomicU64,
}

impl PipeProgress {
    fn push(&self, is_stdout: bool, text: &str) {
        let mut buf = if is_stdout {
            self.out.lock().unwrap_or_else(|e| e.into_inner())
        } else {
            self.err.lock().unwrap_or_else(|e| e.into_inner())
        };
        buf.push_str(text);
        self.bytes.fetch_add(
            text.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    fn take(&self) -> (String, String) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let mut err = self.err.lock().unwrap_or_else(|e| e.into_inner());
        (
            std::mem::take(&mut *out),
            std::mem::take(&mut *err),
        )
    }
}

/// Drain satu pipe per chunk; tiap byte = bukti progres (bukan hang).
/// Decoder inkremental: sisa sekuens UTF-8 multi-byte yang terbelah batas
/// `read()` dibawa ke chunk berikut (bukan FFFD ganda) — tanpa ini string
/// yang sama bisa beda antar run tergantung timing pipe → false
/// `NonDeterministic` di double-run O4.
fn drain_pipe<R: std::io::Read + Send + 'static>(
    pipe: Option<R>,
    progress: &PipeProgress,
    is_stdout: bool,
) {
    let Some(mut p) = pipe else {
        return;
    };
    let mut chunk = [0u8; 8192];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        match p.read(&mut chunk) {
            Ok(0) => {
                if !carry.is_empty() {
                    progress.push(is_stdout, &String::from_utf8_lossy(&carry));
                }
                break;
            }
            Ok(n) => {
                let mut data = std::mem::take(&mut carry);
                data.extend_from_slice(&chunk[..n]);
                let split = utf8_split_point(&data);
                carry = data.split_off(split);
                progress.push(is_stdout, &String::from_utf8_lossy(&data));
            }
            Err(_) => {
                // Flush sisa carry juga di sini (maks 3 byte; pola klasifikasi
                // ASCII tak terpengaruh karena ASCII tak pernah masuk carry).
                if !carry.is_empty() {
                    progress.push(is_stdout, &String::from_utf8_lossy(&carry));
                }
                break;
            }
        }
    }
}

/// Titik belah aman: awal sekuens UTF-8 trailing yang belum lengkap
/// (atau len bila buffer valid penuh). Byte lead invalid → anggap lengkap
/// (lossy tetap ganti FFFD seperti dulu).
fn utf8_split_point(buf: &[u8]) -> usize {
    let mut i = buf.len();
    let mut cont = 0usize;
    while i > 0 && cont < 4 {
        let b = buf[i - 1];
        if b & 0x80 == 0 {
            return buf.len();
        }
        if b & 0xC0 == 0x80 {
            cont += 1;
            i -= 1;
            continue;
        }
        let expected = if b & 0xE0 == 0xC0 {
            1
        } else if b & 0xF0 == 0xE0 {
            2
        } else if b & 0xF8 == 0xF0 {
            3
        } else {
            return buf.len();
        };
        if cont < expected {
            return i - 1;
        }
        return buf.len();
    }
    if cont > 0 {
        return 0;
    }
    buf.len()
}

/// Ambil buffer kedua pipe setelah proses selesai.
fn join_pipes(
    out_handle: std::thread::JoinHandle<()>,
    err_handle: std::thread::JoinHandle<()>,
    progress: &PipeProgress,
) -> (String, String) {
    let _ = out_handle.join();
    let _ = err_handle.join();
    progress.take()
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
            // Sinyal dari luar: proses dibunuh orang lain (supervisor,
            // `pkill`, cgroup teardown, Ctrl-C) — BUKAN bug mivon. Tanpa
            // lengan ini, setiap interruptkampanye jadi `Crash(143)` yang
            // disimpan sebagai temuan.
            SIGTERM | SIGINT | SIGHUP | SIGQUIT => (Kind::CleanError, Some(code)),
            // Sisanya (SIGKILL=9 OOM-killer, SIGSEGV=11, SIGBUS=7, SIGILL=4,
            // SIGFPE=8): crash memory-safety, stack overflow, atau memory
            // blowup dari case besar + thread watchdog bocor menumpuk.
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
/// Sinyal yang BUKAN bug mivon: dikirim dari luar (supervisor, `pkill`,
/// job-object/cgroup teardown, Ctrl-C). Proses bunuh-dirinya sendiri akibat
/// perintah eksternal = bukan temuan fuzzer.
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGINT: i32 = 2;
#[cfg(unix)]
const SIGHUP: i32 = 1;
#[cfg(unix)]
const SIGQUIT: i32 = 3;

/// Apakah status ini hasil KILL kita sendiri (SIGKILL dari `Child::kill`)?
///
/// Dipakai saat grace habis: kalau proses mati SIGKILL = memang tak selesai
/// (hang). Kalau exit normal, berarti ia selesai di detik terakhir dan
/// `kill()`-nya hanya ESRCH — itu BUKAN hang.
#[cfg(unix)]
fn is_killed_by_us(status: &std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(SIGKILL)
}

#[cfg(not(unix))]
fn is_killed_by_us(_status: &std::process::ExitStatus) -> bool {
    true
}

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

    /// REGRESI fuzzer: sinyal dari LUAR (supervisor/pkill/Ctrl-C) bukan bug
    /// mivon — tak boleh jadi `Crash` yang tersimpan di `.mivon-fuzz-bugs`.
    #[cfg(unix)]
    #[test]
    fn classify_external_signal_is_not_a_bug() {
        use std::os::unix::process::ExitStatusExt;
        for sig in [SIGTERM, SIGINT, SIGHUP, SIGQUIT] {
            let st = std::process::ExitStatus::from_raw(sig);
            assert_eq!(
                classify_status(&st, "").0,
                Kind::CleanError,
                "sinyal eksternal {sig} bukan crash mivon"
            );
        }
    }

    /// REGRESI fuzzer: `ExitStatus::from_raw(0)` = exit 0; di unix hanya
    /// menandai sinyal bila bit ORed rendah. Jaga test sinyal di atas tak
    /// salah arti karena encoding `from_raw`.
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
    ///
    /// Loop `while :; do :; done` — BUILTIN saja: tanpa `sleep` (bisa hilang
    /// dari PATH) dan tanpa proses anak yatim yang memegang pipe setelah
    /// shell-nya di-kill (yatim = `join()` reader thread macet).
    #[cfg(unix)]
    #[test]
    fn spawn_timeout_is_hang_not_crash() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("while :; do :; done")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // timeout 300ms → kill_limit 900ms → pasti di-kill oleh kita.
        let out = spawn(&mut cmd, 300);
        assert_eq!(out.kind, Kind::Hang, "timeout harus Hang, bukan crash");
        assert_eq!(out.code, None);
    }

    /// REGRESI fuzzer: proses yang MATI KARENA SINYAL (bukan oleh timeout
    /// kita) harus `Crash`, bukan `CleanError`. Sebelumnya `code()==None`
    /// → CleanError sehingga segfault/abort/OOM 100% tak terlihat.
    #[cfg(unix)]
    #[test]
    fn spawn_signal_death_is_crash() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            // `$$` di mode `-c` = PID shell itu sendiri (tak ada fork) →
            // tak ada proses anak yatim yang memegang pipe.
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

    /// Deadline ADAPTIF: proses lambat TAPI berprogres (output mengalir)
    /// dapat perpanjangan dan selesai → Ok + `slow`, BUKAN Hang.
    /// Tanpa ini wall-time tetap membunuh yang lambat (false hang saat load).
    #[cfg(unix)]
    #[test]
    fn spawn_slow_with_progress_is_slow_not_hang() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("i=0; while [ $i -lt 13 ]; do echo tick$i; i=$((i+1)); sleep 0.05; done")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // Nominal ~0.65s; timeout 150ms (basis 450ms, cap 3 ekstensi → ≤900ms).
        // Rasio seimbang: nominal/basis ≈ cap/nominal ≈ 1.44× — tanpa ekstensi
        // pasti Hang; selesai = bukti deadline adaptif. Jangan rapatkan: di
        // bawah basis → slow=false (bukan bug kode, salah target waktu).
        let out = spawn(&mut cmd, 150);
        assert_eq!(out.kind, Kind::Ok, "berprogres harus selesai Ok, dapat {:?}", out.kind);
        assert!(out.slow, "harus tandai slow (lewat perpanjangan)");
        assert!(out.stdout.contains("tick12"), "output progres wajib utuh");
    }

    /// Spin SENYAP (tanpa byte baru) → tetap Hang walau deadline adaptif.
    #[cfg(unix)]
    #[test]
    fn spawn_silent_spin_is_still_hang() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("while :; do :; done")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        let out = spawn(&mut cmd, 200);
        assert_eq!(out.kind, Kind::Hang, "spin senyap = hang, dapat {:?}", out.kind);
        assert!(!out.slow, "hang tak boleh tandai slow");
    }

    /// Keputusan deadline adaptif — murni, deterministik (tanpa timing):
    /// progres baru + di bawah cap → perpanjang; senyap / cap habis → kill.
    #[test]
    fn deadline_extension_decision_is_pure() {
        assert!(should_extend_deadline(0, 100, 0), "progres pertama → perpanjang");
        assert!(should_extend_deadline(2, 500, 100), "progres lanjut → perpanjang");
        assert!(!should_extend_deadline(0, 0, 0), "senyap total → kill");
        assert!(
            !should_extend_deadline(1, 100, 100),
            "byte stagnan → kill (burst-lalu-spin)"
        );
        assert!(
            !should_extend_deadline(MAX_PROGRESS_EXTENSIONS, 999, 0),
            "cap habis → kill walau berprogres"
        );
        assert!(
            !should_extend_deadline(MAX_PROGRESS_EXTENSIONS + 5, 999, 0),
            "lewat cap → kill"
        );
    }

    /// Burst-di-awal lalu spin: SATU perpanjangan terbuang, lalu Hang
    /// (cap MAX_PROGRESS_EXTENSIONS). Output awal tetap tersimpan.
    #[cfg(unix)]
    #[test]
    fn spawn_burst_then_spin_is_hang_with_capped_extensions() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("echo boot; echo ready; while :; do :; done")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // timeout 200ms: basis 600ms + maks 3×200ms ekstensi ≈ 1.2s.
        let out = spawn(&mut cmd, 200);
        assert_eq!(out.kind, Kind::Hang, "burst-lalu-spin = hang, dapat {:?}", out.kind);
        assert!(!out.slow, "kill tak boleh tandai slow walau sempat berprogres");
        assert!(out.stdout.contains("boot"), "output sebelum spin wajib ada");
    }
}
