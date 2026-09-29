//! StatusLine — progres compile Mivon bergaya Cargo.
//!
//! Menampilkan SATU baris status yang digambar ulang di tempat (`\r` +
//! clear-line), bukan area 12 baris dengan waveform. Isinya: strip 5 fase
//! pipeline (LEX/PAR/ELA/OPT/VER), subjek fase (file `.sv/.svh/.v/.vh`
//! terakhir, top module, dsb.), counter, dan elapsed.
//!
//! Karakteristik gaya Cargo:
//! - Satu baris, tanpa menyembunyikan kursor, tanpa scroll management.
//! - Baris log fase dicetak thread utama di ATAS baris status — baris status
//!   selalu menempel di posisi kursor terakhir, jadi output baru menggesernya
//!   ke bawah. Panggil `clear_line()` dulu bila output menyambung baris status.
//! - Redraw hanya bila konten berubah (anti-flicker), minimum `REDRAW_MS`.
//! - Non-TTY / `--quiet` / `MIVON_NO_ANIM` → tidak ada animasi sama sekali.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// ── ANSI ──
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BRIGHT_GREEN: &str = "\x1b[92m";
const YELLOW: &str = "\x1b[33m";
const GRAY: &str = "\x1b[90m";
const CYAN: &str = "\x1b[36m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const CLEAR_LINE: &str = "\x1b[2K";

/// Interval redraw minimum (ms) — mencegah flicker dan I/O berlebih.
const REDRAW_MS: u64 = 80;
/// Maksimum lebar subjek fase (nama file/top) sebelum dipotong.
const MAX_SUBJECT: usize = 24;

/// Fase pipeline yang ditampilkan (tetap 5, sesuai pipeline legacy).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Lex,
    Par,
    Ela,
    Opt,
    Ver,
}

const PHASE_NAMES: [&str; 5] = ["LEX", "PAR", "ELA", "OPT", "VER"];

impl Phase {
    fn idx(self) -> usize {
        match self {
            Phase::Lex => 0,
            Phase::Par => 1,
            Phase::Ela => 2,
            Phase::Opt => 3,
            Phase::Ver => 4,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PhaseStatus {
    Idle,
    Running,
    Done,
    Warn,
    Error,
}

struct PhaseInfo {
    status: PhaseStatus,
    warn_count: usize,
}

impl Default for PhaseInfo {
    fn default() -> Self {
        PhaseInfo {
            status: PhaseStatus::Idle,
            warn_count: 0,
        }
    }
}

struct AnimState {
    phases: [PhaseInfo; 5],
    files_total: u64,
    files_done: u64,
    files_cached: u64,
    /// File terakhir yang selesai diproses (basename).
    current_file: String,
    /// Jumlah file per ekstensi: sv, svh, v, vh (dihitung caller).
    exts: [u64; 4],
    modules: u64,
    tokens: u64,
    top: String,
    start: Instant,
    finished: bool,
    ok: bool,
    errors: usize,
    warnings: usize,
    /// Saat true thread render TIDAK menulis — dipakai `suspend_print`
    /// agar output multi-baris (diagnostik/timing) tidak balapan dengannya.
    paused: bool,
}

impl Default for AnimState {
    fn default() -> Self {
        AnimState {
            phases: std::array::from_fn(|_| PhaseInfo::default()),
            files_total: 0,
            files_done: 0,
            files_cached: 0,
            current_file: String::new(),
            exts: [0; 4],
            modules: 0,
            tokens: 0,
            top: String::new(),
            start: Instant::now(),
            finished: false,
            ok: true,
            errors: 0,
            warnings: 0,
            paused: false,
        }
    }
}

/// Handle ringan (clone-able, thread-safe) ke state animasi — dipakai dari
/// rayon par_iter / callback CompileSession untuk melaporkan progres per-file
/// tanpa membawa `PipelineAnimator` utuh.
#[derive(Clone)]
pub struct AnimHandle {
    state: Arc<Mutex<AnimState>>,
}

impl AnimHandle {
    /// Laporkan satu file selesai disiapkan (lex/parse per-file).
    /// Alloc basename di LUAR lock — panggilan dari beberapa worker rayon
    /// serentak, hold time lock harus sesingkat mungkin.
    pub fn file_done(&self, path: &Path, cached: bool) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let mut st = self.state.lock().unwrap();
        st.files_done += 1;
        if cached {
            st.files_cached += 1;
        }
        // File auto-include bisa membuat done > total yang di-set awal.
        if st.files_done > st.files_total {
            st.files_total = st.files_done;
        }
        st.current_file = name;
    }
}

/// Status line animator pipeline compile. `start()` mengembalikan `None` bila
/// terminal tidak mendukung (bukan TTY) atau animasi dinonaktifkan — caller
/// tidak perlu perubahan apa pun untuk jalur non-interaktif.
pub struct PipelineAnimator {
    state: Arc<Mutex<AnimState>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    active: bool,
}

/// Deteksi apakah stdout adalah TTY.
fn stdout_is_tty() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = io::stdout().as_raw_fd();
        unsafe extern "C" {
            fn isatty(fd: i32) -> i32;
        }
        unsafe { isatty(fd) != 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

impl PipelineAnimator {
    /// Mulai animasi. `enabled` mengontrol apakah animasi diizinkan (mis.
    /// false saat `--quiet` / mode debug). Mengembalikan `None` bila tidak
    /// ada animasi (bukan TTY atau disabled).
    ///
    /// PANGGIL SAAT KERJA NYATA MULAI (tepat sebelum compile/preprocess),
    /// bukan saat setup — sebelum itu terminal harus bersih.
    pub fn start(enabled: bool) -> Option<PipelineAnimator> {
        if std::env::var("MIVON_NO_ANIM").is_ok() {
            return None;
        }
        if !enabled || !stdout_is_tty() {
            return None;
        }
        let state = Arc::new(Mutex::new(AnimState::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let render_state = Arc::clone(&state);
        let render_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut last_line = String::new();
            let mut last_draw = Instant::now();
            loop {
                if render_stop.load(Ordering::Relaxed) {
                    break;
                }
                let line = {
                    let st = render_state.lock().unwrap();
                    // Render dijeda selama output lain dicetak (suspend_print).
                    if st.paused {
                        String::new()
                    } else {
                        build_line(&st, false)
                    }
                };
                if line.is_empty() {
                    // Tidak digambar; reset cache agar baris langsung muncul
                    // kembali setelah resume.
                    last_line.clear();
                } else if line != last_line
                    && last_draw.elapsed() >= Duration::from_millis(REDRAW_MS)
                {
                    // Satu write tunggal per frame — atomic di stdout LineWriter.
                    let mut out = io::stdout();
                    let _ = write!(out, "\r{}{}", CLEAR_LINE, line);
                    let _ = out.flush();
                    last_line = line;
                    last_draw = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(REDRAW_MS));
            }
        });

        // Gambar baris pertama segera (idle: "Compiling N file ...").
        {
            let line = {
                let st = state.lock().unwrap();
                build_line(&st, false)
            };
            let mut out = io::stdout();
            let _ = write!(out, "\r{}{}", CLEAR_LINE, line);
            let _ = out.flush();
        }

        Some(PipelineAnimator {
            state,
            stop,
            handle: Some(handle),
            active: true,
        })
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Handle clone-able untuk callback per-file (rayon / CompileSession).
    pub fn handle(&self) -> AnimHandle {
        AnimHandle {
            state: Arc::clone(&self.state),
        }
    }

    /// Bersihkan baris status sebelum output lain dicetak (log fase, diagnostik)
    /// agar teks baru tidak menyambung dengan baris status.
    pub fn clear_line(&self) {
        let mut out = io::stdout();
        let _ = write!(out, "\r{}", CLEAR_LINE);
        let _ = out.flush();
    }

    /// Cetak output (biasanya multi-baris: diagnostik, timing) dengan aman:
    /// render dijeda → baris status dibersihkan → `f()` berjalan → baris status
    /// digambar ulang. Cegah balapan tulis antara thread render dan thread
    /// utama — satu-satunya kunci sinkronisasi output.
    pub fn suspend_print<F: FnOnce()>(&self, f: F) {
        {
            let mut st = self.state.lock().unwrap();
            st.paused = true;
        }
        self.clear_line();
        f();
        {
            let mut st = self.state.lock().unwrap();
            st.paused = false;
            // Gambar ulang segera (tanpa menunggu tick berikutnya).
            if !st.finished {
                let line = build_line(&st, false);
                let mut out = io::stdout();
                let _ = write!(out, "\r{}{}", CLEAR_LINE, line);
                let _ = out.flush();
            }
        }
    }

    pub fn phase_running(&self, phase: Phase) {
        let mut st = self.state.lock().unwrap();
        let i = phase.idx();
        st.phases[i].status = PhaseStatus::Running;
    }

    pub fn phase_done(&self, phase: Phase) {
        let mut st = self.state.lock().unwrap();
        let i = phase.idx();
        st.phases[i].status = PhaseStatus::Done;
    }

    pub fn phase_warn(&self, phase: Phase, count: usize) {
        let mut st = self.state.lock().unwrap();
        let i = phase.idx();
        st.phases[i].status = PhaseStatus::Warn;
        st.phases[i].warn_count = count;
    }

    pub fn phase_error(&self, phase: Phase) {
        let mut st = self.state.lock().unwrap();
        let i = phase.idx();
        st.phases[i].status = PhaseStatus::Error;
    }

    pub fn set_files(&self, total: u64, done: u64) {
        let mut st = self.state.lock().unwrap();
        st.files_total = total;
        st.files_done = done;
    }

    pub fn set_cached(&self, n: u64) {
        let mut st = self.state.lock().unwrap();
        st.files_cached = n;
    }

    /// Jumlah file per ekstensi `[sv, svh, v, vh]` — tampil di baris idle.
    pub fn set_ext_counts(&self, exts: [u64; 4]) {
        let mut st = self.state.lock().unwrap();
        st.exts = exts;
    }

    /// Top module aktif (tampil saat fase ELA).
    pub fn set_top(&self, top: &str) {
        let mut st = self.state.lock().unwrap();
        st.top = top.to_string();
    }

    pub fn set_modules(&self, n: u64) {
        let mut st = self.state.lock().unwrap();
        st.modules = n;
    }

    pub fn set_tokens(&self, n: u64) {
        let mut st = self.state.lock().unwrap();
        st.tokens = n;
    }

    /// Hentikan thread render dan gambar baris ringkasan final (diakhiri
    /// newline; output berikutnya dicetak di bawahnya).
    pub fn finish(&mut self, ok: bool, errors: usize, warnings: usize) {
        if !self.active {
            return;
        }
        {
            let mut st = self.state.lock().unwrap();
            for p in st.phases.iter_mut() {
                if p.status == PhaseStatus::Running {
                    p.status = PhaseStatus::Done;
                }
            }
            st.finished = true;
            st.ok = ok;
            st.errors = errors;
            st.warnings = warnings;
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let line = {
            let st = self.state.lock().unwrap();
            build_line(&st, true)
        };
        let mut out = io::stdout();
        let _ = write!(out, "\r{}{}\n", CLEAR_LINE, line);
        let _ = out.flush();
        self.active = false;
    }

    /// Abort karena error — baris menunjukkan kegagalan.
    pub fn abort(&mut self, errors: usize, warnings: usize) {
        self.finish(false, errors, warnings);
    }
}

impl Drop for PipelineAnimator {
    fn drop(&mut self) {
        if self.active {
            // Jalur keluar tanpa finish() (error dini): hentikan render dan
            // bersihkan baris status agar tidak ada sisa teks di terminal.
            self.stop.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            let mut out = io::stdout();
            let _ = write!(out, "\r{}", CLEAR_LINE);
            let _ = out.flush();
        }
    }
}

// ── Rendering ──

/// Bentuk tanda per fase.
fn phase_marker(status: PhaseStatus) -> &'static str {
    match status {
        PhaseStatus::Idle => "·",
        PhaseStatus::Running => "▸",
        PhaseStatus::Done => "✓",
        PhaseStatus::Warn => "▲",
        PhaseStatus::Error => "✖",
    }
}

fn phase_color(status: PhaseStatus) -> &'static str {
    match status {
        PhaseStatus::Idle => GRAY,
        PhaseStatus::Running => BRIGHT_GREEN,
        PhaseStatus::Done => GREEN,
        PhaseStatus::Warn => YELLOW,
        PhaseStatus::Error => RED,
    }
}

/// Strip 5 fase, mis. `LEX✓ PAR▸ ELA· OPT· VER·` (berwarna per status).
fn phase_strip(st: &AnimState) -> String {
    let mut s = String::with_capacity(32);
    for (i, info) in st.phases.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let color = phase_color(info.status);
        let bold = if info.status == PhaseStatus::Running {
            BOLD
        } else {
            ""
        };
        s.push_str(color);
        s.push_str(bold);
        s.push_str(PHASE_NAMES[i]);
        s.push_str(phase_marker(info.status));
        s.push_str(RESET);
    }
    s
}

/// Potong subjek dari kiri bila terlalu panjang: `…core/alu.sv`.
fn clamp_subject(s: &str) -> String {
    if s.chars().count() <= MAX_SUBJECT {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let tail: String = chars[chars.len() - MAX_SUBJECT + 1..].iter().collect();
    format!("…{}", tail)
}

/// Subjek fase: LEX/PAR → file terakhir; ELA → top; OPT → token; VER → diag.
fn subject_line(st: &AnimState, idx: usize) -> String {
    let raw = match idx {
        0 | 1 => st.current_file.clone(),
        2 => {
            if !st.top.is_empty() {
                format!("top: {}", st.top)
            } else if st.modules > 0 {
                format!("{} module", st.modules)
            } else {
                String::new()
            }
        }
        3 => {
            if st.tokens > 0 {
                format!("{} token", fmt_count(st.tokens))
            } else {
                String::new()
            }
        }
        _ => {
            if st.errors > 0 || st.warnings > 0 {
                format!("{} err · {} warn", st.errors, st.warnings)
            } else {
                String::new()
            }
        }
    };
    clamp_subject(&raw)
}

/// Counter kanan: `47/128 · 12 cached · 0.6s`.
fn stats_line(st: &AnimState) -> String {
    let mut parts: Vec<String> = Vec::new();
    if st.files_total > 0 {
        parts.push(format!(
            "{}{}/{}{}",
            BOLD, st.files_done, st.files_total, RESET
        ));
    }
    if st.files_cached > 0 {
        parts.push(format!("{}{} cached{}", GRAY, st.files_cached, RESET));
    }
    let elapsed = st.start.elapsed().as_secs_f64();
    parts.push(format!("{}{:.1}s{}", CYAN, elapsed, RESET));
    parts.join(&format!("{} · {}", DIM, RESET))
}

/// Baris idle sebelum ada fase berjalan:
/// `  Compiling 128 file · sv 112 · svh 14 · v 2`.
fn idle_line(st: &AnimState) -> String {
    let mut s = format!("  {}Compiling{} ", DIM, RESET);
    s.push_str(&format!("{}{} file{}", BOLD, st.files_total, RESET));
    let labels = ["sv", "svh", "v", "vh"];
    for (i, n) in st.exts.iter().enumerate() {
        if *n > 0 {
            s.push_str(&format!("{} · {} {}{}", DIM, labels[i], n, RESET));
        }
    }
    let elapsed = st.start.elapsed().as_secs_f64();
    s.push_str(&format!("{} · {}{:.1}s{}", DIM, CYAN, elapsed, RESET));
    s
}

/// Baris ringkasan final. Urutan: ANGKA dulu baru label
/// (`128 file`, bukan `file128`).
fn summary_line(st: &AnimState) -> String {
    let elapsed = st.start.elapsed().as_secs_f64();
    if st.ok {
        let mut s = format!("  {}{}✓ Compile Completed{}", GREEN, BOLD, RESET);
        s.push_str(&format!(
            "{} · {}{} file{}",
            DIM, BOLD, st.files_total, RESET
        ));
        if st.modules > 0 {
            s.push_str(&format!("{} · {}{} module{}", DIM, BOLD, st.modules, RESET));
        }
        if st.warnings > 0 {
            s.push_str(&format!(
                "{} · {}{} warning{}{}",
                DIM,
                YELLOW,
                st.warnings,
                if st.warnings > 1 { "s" } else { "" },
                RESET
            ));
        }
        s.push_str(&format!("{} · {:.2}s{}", DIM, elapsed, RESET));
        s
    } else {
        let mut s = format!("  {}{}✖ Compile Failed{}", RED, BOLD, RESET);
        let err_color = if st.errors > 0 { RED } else { GRAY };
        let warn_color = if st.warnings > 0 { YELLOW } else { GRAY };
        s.push_str(&format!(
            "{} · {}{} error{}{}",
            DIM,
            err_color,
            st.errors,
            if st.errors == 1 { "" } else { "s" },
            RESET
        ));
        s.push_str(&format!(
            "{} · {}{} warning{}{}",
            DIM,
            warn_color,
            st.warnings,
            if st.warnings == 1 { "" } else { "s" },
            RESET
        ));
        s.push_str(&format!("{} · {:.2}s{}", DIM, elapsed, RESET));
        s
    }
}

fn build_line(st: &AnimState, final_frame: bool) -> String {
    if final_frame {
        return summary_line(st);
    }
    // Fase aktif = pertama yang Running; kalau tidak ada, fase terakhir
    // yang tidak Idle (semua selesai tapi finish belum dipanggil).
    let active = st
        .phases
        .iter()
        .position(|p| p.status == PhaseStatus::Running)
        .or_else(|| {
            st.phases
                .iter()
                .rposition(|p| p.status != PhaseStatus::Idle)
        });
    let Some(idx) = active else {
        return idle_line(st);
    };

    let subject = subject_line(st, idx);
    let stats = stats_line(st);
    if subject.is_empty() {
        format!("  {}  {}", phase_strip(st), stats)
    } else {
        format!(
            "  {}  {}{}{}  {}",
            phase_strip(st),
            CYAN,
            subject,
            RESET,
            stats
        )
    }
}

/// Format angka besar: `1.5K` / `6.7M`.
fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with() -> AnimState {
        AnimState::default()
    }

    #[test]
    fn test_phase_idx() {
        assert_eq!(Phase::Lex.idx(), 0);
        assert_eq!(Phase::Par.idx(), 1);
        assert_eq!(Phase::Ela.idx(), 2);
        assert_eq!(Phase::Opt.idx(), 3);
        assert_eq!(Phase::Ver.idx(), 4);
    }

    #[test]
    fn test_idle_line_exts() {
        let mut st = state_with();
        st.files_total = 128;
        st.exts = [112, 14, 2, 0];
        let line = idle_line(&st);
        assert!(line.contains("128 file"), "line: {}", line);
        assert!(line.contains("sv 112"), "line: {}", line);
        assert!(line.contains("svh 14"), "line: {}", line);
        assert!(line.contains("v 2"), "line: {}", line);
        assert!(!line.contains("vh 0"), "vh seharusnya hilang: {}", line);
    }

    #[test]
    fn test_status_line_running() {
        let mut st = state_with();
        st.phases[0].status = PhaseStatus::Running;
        st.current_file = "alu_core.sv".into();
        st.files_total = 10;
        st.files_done = 4;
        let line = build_line(&st, false);
        assert!(line.contains("LEX▸"), "line: {}", line);
        assert!(line.contains("PAR·"), "line: {}", line);
        assert!(line.contains("alu_core.sv"), "line: {}", line);
        assert!(line.contains("4/10"), "line: {}", line);
    }

    #[test]
    fn test_status_line_done_phase() {
        let mut st = state_with();
        st.phases[0].status = PhaseStatus::Done;
        st.phases[1].status = PhaseStatus::Done;
        st.phases[2].status = PhaseStatus::Running;
        st.top = "tb_counter".into();
        let line = build_line(&st, false);
        assert!(line.contains("LEX✓"), "line: {}", line);
        assert!(line.contains("PAR✓"), "line: {}", line);
        assert!(line.contains("ELA▸"), "line: {}", line);
        assert!(line.contains("top: tb_counter"), "line: {}", line);
        assert!(line.contains("VER·"), "line: {}", line);
    }

    #[test]
    fn test_summary_ok_and_failed() {
        let mut st = state_with();
        st.files_total = 128;
        st.modules = 14;
        st.finished = true;
        st.ok = true;
        st.warnings = 1;
        let ok = summary_line(&st);
        assert!(ok.contains("Compile Completed"), "line: {}", ok);
        assert!(ok.contains("128"), "line: {}", ok);
        assert!(ok.contains("14 module"), "line: {}", ok);
        assert!(ok.contains("1 warning"), "line: {}", ok);

        st.ok = false;
        st.errors = 2;
        let bad = summary_line(&st);
        assert!(bad.contains("Compile Failed"), "line: {}", bad);
        assert!(bad.contains("2 errors"), "line: {}", bad);
    }

    #[test]
    fn test_summary_failed_reports_actual_counts() {
        // Regresi: jalur "elaborasi di-skip karena parse error" pernah memanggil
        // finish(false, 0, 0) → header "Compile Failed · 0 errors" padahal blok
        // "Kesiapan Simulasi" menampilkan N error. Ringkasan harus menampilkan
        // angka yang diberikan pemanggil apa adanya.
        let mut st = state_with();
        st.ok = false;
        st.errors = 37;
        st.warnings = 0;
        let bad = summary_line(&st);
        assert!(bad.contains("Compile Failed"), "line: {}", bad);
        assert!(bad.contains("37 errors"), "line: {}", bad);
        assert!(bad.contains("0 warnings"), "line: {}", bad);

        // Bentuk tunggal — jangan sampai menulis "1 errors" / "1 warnings".
        st.errors = 1;
        st.warnings = 1;
        let one = summary_line(&st);
        assert!(one.contains("1 error"), "line: {}", one);
        assert!(!one.contains("1 errors"), "line: {}", one);
        assert!(one.contains("1 warning"), "line: {}", one);
        assert!(!one.contains("1 warnings"), "line: {}", one);
    }

    #[test]
    fn test_clamp_subject() {
        let long = "some/very/long/path/to/rtl/core/alu_core.sv";
        let clamped = clamp_subject(long);
        assert!(
            clamped.chars().count() <= MAX_SUBJECT,
            "clamped: {}",
            clamped
        );
        assert!(clamped.ends_with("alu_core.sv"), "clamped: {}", clamped);
        assert_eq!(clamp_subject("alu.sv"), "alu.sv");
    }

    #[test]
    fn test_handle_file_done_counters() {
        let state = Arc::new(Mutex::new(AnimState::default()));
        let h = AnimHandle {
            state: Arc::clone(&state),
        };
        h.file_done(Path::new("rtl/alu.sv"), false);
        h.file_done(Path::new("rtl/alu.svh"), true);
        h.file_done(Path::new("rtl/top.v"), true);
        let st = state.lock().unwrap();
        assert_eq!(st.files_done, 3);
        assert_eq!(st.files_cached, 2);
        // total awal 0 → ikut naik ke done (auto-include safety).
        assert_eq!(st.files_total, 3);
        assert_eq!(st.current_file, "top.v");
    }

    #[test]
    fn test_fmt_count() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1_500), "1.5K");
        assert_eq!(fmt_count(6_700_000), "6.7M");
    }

    #[test]
    fn test_animator_not_tty_or_disabled() {
        // Dalam test stdout bukan TTY → start harus None, termasuk enabled=true.
        assert!(PipelineAnimator::start(false).is_none());
        assert!(PipelineAnimator::start(true).is_none());
    }
}
