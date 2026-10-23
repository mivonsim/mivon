//! ──────────────────────────────────────────────────────────────────────────────
//! Global Diagnostic Engine (GDE) — agregator global, BUKAN kolektor kedua.
//!
//! Aturan "satu pusat koleksi": sink per-komponen (`DiagSink`) adalah
//! SATU-SATUNYA penyimpan diagnostic penuh. Setiap `DiagSink::push()`
//! mendaftarkan hitungan ke engine ini, dan engine hanya menyimpan:
//!   - counter per kode (`by_code`)  — untuk coverage top-code registry
//!   - counter per level (`by_level`)
//!   - total/dropped
//!   - sampel bounded (SAMPLE_CAP) untuk diagnostic TANPA posisi dan
//!     diagnostic dengan kode di luar registry `all_codes()` — dua-duanya
//!     subset kecil yang dipakai CLI `--gdiag`.
//!
//! Dulu engine menyimpan `Vec<Diagnostic>` penuh (cap 1 juta) sebagai salinan
//! kedua dari setiap diagnostic → memori tumbuh tanpa henti pada proses
//! panjang (GUI/LSP/CI worker) dan data dobel dengan sink. Sekarang salinan
//! penuh hanya di sink; agregat di sini bounded dan murah.
//!
//! API (semua agregat/kounter, tanpa koleksi penuh):
//!   - `diag_global()`                 → akses singleton global
//!   - `report()`                      → single entry point (dipanggil DiagSink::push)
//!   - `report_error/warning/fatal/note(...)` → helper
//!   - `level_count()/code_count()`   → counter per level/kode
//!   - `code_usage()/uncovered_codes()` → analisis coverage kode registry
//!   - `unpositioned()/uncoded()`     → sampel bounded (subset kecil)
//!   - `summary()/coverage_report()`  → laporan ringkas (CLI `--gdiag`)
//!
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::codes::all_codes;
use super::diagnostic::{DiagCode, DiagLevel, Diagnostic};

/// Batas sampel (bounded) untuk daftar unpositioned / unregistered.
/// Keduanya subset kecil pada run sehat; cap mencegah loop spam (mis.
/// `forever` yang memuntahkan warning tanpa posisi) menumpuk memori.
const SAMPLE_CAP: usize = 200;

/// Registry semua kode resmi — di-cache sekali (70 varian) agar tiap `report()`
/// tidak membangun ulang seluruh tabel `all_codes()`.
fn registry_codes() -> &'static [DiagCode] {
    static REG: OnceLock<Vec<DiagCode>> = OnceLock::new();
    REG.get_or_init(|| all_codes().iter().map(|(c, _)| *c).collect())
}

struct GlobalState {
    by_code: HashMap<DiagCode, usize>,
    by_level: HashMap<DiagLevel, usize>,
    /// Sampel diagnostic TANPA posisi (untuk CLI coverage report).
    unpositioned_sample: Vec<Diagnostic>,
    /// Sampel diagnostic dengan kode di luar registry `all_codes()`.
    unregistered_sample: Vec<Diagnostic>,
}

/// Global Diagnostic Engine — agregator diagnostic lintas seluruh komponen.
pub struct GlobalDiagnosticEngine {
    state: Mutex<GlobalState>,
    /// Total diagnostic yang dilaporkan.
    total: AtomicUsize,
    /// Diagnostic yang dibuang karena kapasitas tercapai.
    dropped: AtomicUsize,
    started: Instant,
}

impl Default for GlobalDiagnosticEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalDiagnosticEngine {
    pub fn new() -> Self {
        GlobalDiagnosticEngine {
            state: Mutex::new(GlobalState {
                by_code: HashMap::new(),
                by_level: HashMap::new(),
                unpositioned_sample: Vec::new(),
                unregistered_sample: Vec::new(),
            }),
            total: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            started: Instant::now(),
        }
    }

    // ── Pelaporan (single entry point) ──

    /// Daftarkan diagnostic ke agregat global. Semua komponen masuk lewat sini
    /// (melalui `DiagSink::push`). TIDAK menyimpan salinan penuh — hanya
    /// counter + sampel bounded. Thread-safe.
    pub fn report(&self, diag: Diagnostic) {
        self.total.fetch_add(1, Ordering::Relaxed);
        let mut st = self.state.lock().unwrap();
        *st.by_code.entry(diag.code).or_insert(0) += 1;
        *st.by_level.entry(diag.level).or_insert(0) += 1;

        // Sampel unpositioned: tanpa source_snippet DAN tanpa spans.
        if diag.source_snippet.is_none() && diag.spans.is_empty() {
            if st.unpositioned_sample.len() < SAMPLE_CAP {
                st.unpositioned_sample.push(diag.clone());
            } else {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        // Sampel kode tak terdaftar (registry coverage). Kode di luar registry
        // jarang — juga bounded.
        if !registry_codes().contains(&diag.code)
            && st.unregistered_sample.len() < SAMPLE_CAP
        {
            st.unregistered_sample.push(diag);
        }
    }

    /// Helper: laporkan error dengan kode.
    pub fn report_error(&self, code: DiagCode, message: impl Into<String>) {
        self.report(Diagnostic::new(DiagLevel::Error, code, message.into()));
    }

    /// Helper: laporkan warning dengan kode.
    pub fn report_warning(&self, code: DiagCode, message: impl Into<String>) {
        self.report(Diagnostic::new(DiagLevel::Warning, code, message.into()));
    }

    /// Helper: laporkan fatal dengan kode.
    pub fn report_fatal(&self, code: DiagCode, message: impl Into<String>) {
        self.report(Diagnostic::new(DiagLevel::Fatal, code, message.into()));
    }

    /// Helper: laporkan note (tanpa kode error).
    pub fn report_note(&self, message: impl Into<String>) {
        self.report(Diagnostic::new(
            DiagLevel::Note,
            DiagCode::SimulationError,
            message.into(),
        ));
    }

    // ── Counter agregat ──

    /// Jumlah diagnostic per level yang tercatat.
    pub fn level_count(&self, level: DiagLevel) -> usize {
        self.state
            .lock()
            .unwrap()
            .by_level
            .get(&level)
            .copied()
            .unwrap_or(0)
    }

    /// Jumlah diagnostic per kode yang tercatat.
    pub fn code_count(&self, code: DiagCode) -> usize {
        self.state
            .lock()
            .unwrap()
            .by_code
            .get(&code)
            .copied()
            .unwrap_or(0)
    }

    /// Sampel diagnostic tanpa posisi (bounded SAMPLE_CAP, klik dokumentasi modul).
    pub fn unpositioned(&self) -> Vec<Diagnostic> {
        self.state.lock().unwrap().unpositioned_sample.clone()
    }

    /// Sampel diagnostic dengan kode di luar registry (bounded SAMPLE_CAP).
    pub fn uncoded(&self) -> Vec<Diagnostic> {
        self.state.lock().unwrap().unregistered_sample.clone()
    }

    /// Statistik pemakaian kode: (kode-string, jumlah), terurut menurun.
    pub fn code_usage(&self) -> Vec<(String, usize)> {
        let st = self.state.lock().unwrap();
        let mut v: Vec<(String, usize)> = st
            .by_code
            .iter()
            .map(|(c, n)| (c.as_str().to_string(), *n))
            .collect();
        drop(st);
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// Kode di registry yang TIDAK pernah dipakai komponen mana pun
    /// ("uncovered" oleh pemakaian nyata).
    pub fn uncovered_codes(&self) -> Vec<String> {
        let st = self.state.lock().unwrap();
        all_codes()
            .into_iter()
            .filter(|(c, _)| !st.by_code.contains_key(c))
            .map(|(_, s)| s.to_string())
            .collect()
    }

    // ── Statistik & laporan ──

    /// Total diagnostic yang dilaporkan (agregat — bukan koleksi penuh).
    pub fn len(&self) -> usize {
        self.total()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total diagnostic yang dilaporkan.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Sampel yang dibuang karena SAMPLE_CAP tercapai.
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Kosongkan agregat global (untuk test / sesi baru).
    pub fn reset(&self) {
        let mut st = self.state.lock().unwrap();
        st.by_code.clear();
        st.by_level.clear();
        st.unpositioned_sample.clear();
        st.unregistered_sample.clear();
    }

    /// Ringkasan singkat: total per level + posisi/registry coverage.
    pub fn summary(&self) -> String {
        let e = self.level_count(DiagLevel::Error);
        let w = self.level_count(DiagLevel::Warning);
        let f = self.level_count(DiagLevel::Fatal);
        let n = self.level_count(DiagLevel::Note);
        let up = self.unpositioned().len();
        let registry = all_codes().len();
        format!(
            "Global diagnostics: total={} (pushed={} dropped={}) error={} warning={} fatal={} note={} | unpositioned={} | codes_emitted={}/{}",
            self.len(),
            self.total(),
            self.dropped(),
            e,
            w,
            f,
            n,
            up,
            self.code_usage().len(),
            registry,
        )
    }

    /// Laporan coverage lengkap: per-level, top kode, kode yang belum terpakai.
    pub fn coverage_report(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.summary());
        out.push('\n');
        let usage = self.code_usage();
        if !usage.is_empty() {
            out.push_str("  Top codes:\n");
            for (code, n) in usage.iter().take(20) {
                out.push_str(&format!("    {} : {}\n", code, n));
            }
        }
        let uncovered = self.uncovered_codes();
        if !uncovered.is_empty() {
            out.push_str(&format!(
                "  Codes never emitted ({}): {}\n",
                uncovered.len(),
                uncovered.join(", ")
            ));
        }
        let up = self.unpositioned();
        if !up.is_empty() {
            out.push_str(&format!("  Unpositioned diagnostics ({}):\n", up.len()));
            for d in up.iter().take(20) {
                out.push_str(&format!("    [{}] {}\n", d.code.as_str(), d.message));
            }
        }
        out
    }
}

/// Singleton global engine (agregator — lihat dokumentasi modul).
pub fn diag_global() -> &'static GlobalDiagnosticEngine {
    static GLOBAL: OnceLock<GlobalDiagnosticEngine> = OnceLock::new();
    GLOBAL.get_or_init(GlobalDiagnosticEngine::new)
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::DiagSink;

    #[test]
    fn test_report_aggregates() {
        let engine = GlobalDiagnosticEngine::new();
        assert!(engine.is_empty());
        engine.report_error(DiagCode::UndefinedSignal, "a not found");
        engine.report_warning(DiagCode::WidthMismatchWarning, "width mismatch");
        assert_eq!(engine.total(), 2);
        assert_eq!(engine.code_count(DiagCode::UndefinedSignal), 1);
        assert_eq!(engine.level_count(DiagLevel::Error), 1);
        assert_eq!(engine.level_count(DiagLevel::Warning), 1);
        // Kedua diagnostic di atas tanpa snippet/span → masuk sampel unpositioned.
        assert_eq!(engine.unpositioned().len(), 2);
    }

    /// Fatal tercatat di level-nya sendiri; summary menampilkan fatal=X.
    #[test]
    fn fatal_recorded_in_summary() {
        let engine = GlobalDiagnosticEngine::new();
        engine.report_fatal(DiagCode::InfiniteDelta, "fatal stop");
        assert_eq!(engine.level_count(DiagLevel::Fatal), 1);
        let s = engine.summary();
        assert!(s.contains("fatal=1"), "summary harus mencatat fatal: {s}");
    }

    #[test]
    fn test_unpositioned_sample() {
        let engine = GlobalDiagnosticEngine::new();
        engine.report_error(DiagCode::UndefinedSignal, "no position");
        let mut d = Diagnostic::new(DiagLevel::Error, DiagCode::TypeMismatch, "with position");
        d = d.with_source_snippet(crate::diagnostics::SourceSnippet::new(
            "a.sv", 10, 5, "x = 1;",
        ));
        engine.report(d);
        let up = engine.unpositioned();
        assert_eq!(up.len(), 1, "hanya diagnostic tanpa posisi yang di-sample");
        assert_eq!(up[0].message, "no position");
    }

    /// Sampel unpositioned bounded — loop spam tidak boleh menumpuk memori.
    #[test]
    fn test_unpositioned_sample_capped() {
        let engine = GlobalDiagnosticEngine::new();
        for i in 0..(SAMPLE_CAP + 50) {
            engine.report_error(DiagCode::UndefinedSignal, format!("spam {i}"));
        }
        assert_eq!(engine.unpositioned().len(), SAMPLE_CAP);
        assert_eq!(engine.dropped(), 50);
        // Counter tetap akurat meski sampel penuh.
        assert_eq!(engine.total(), SAMPLE_CAP + 50);
        assert_eq!(engine.code_count(DiagCode::UndefinedSignal), SAMPLE_CAP + 50);
    }

    #[test]
    fn test_coverage_counts() {
        let engine = GlobalDiagnosticEngine::new();
        engine.report_error(DiagCode::UndefinedSignal, "x");
        engine.report_error(DiagCode::UndefinedSignal, "y");
        engine.report_warning(DiagCode::WidthMismatchWarning, "z");
        let usage = engine.code_usage();
        assert_eq!(usage[0].0, DiagCode::UndefinedSignal.as_str());
        assert_eq!(usage[0].1, 2);
        // Kode yang pernah dipakai tidak masuk uncovered.
        let uncovered = engine.uncovered_codes();
        assert!(!uncovered.contains(&DiagCode::UndefinedSignal.as_str().to_string()));
    }

    #[test]
    fn test_summary_has_counts() {
        let engine = GlobalDiagnosticEngine::new();
        engine.report_error(DiagCode::UndefinedSignal, "x");
        let s = engine.summary();
        assert!(s.contains("error=1"));
        assert!(s.contains("pushed="));
    }

    #[test]
    fn test_diag_sink_forwards_to_global() {
        // DiagSink adalah jalur utama komponen; push harus tercatat di agregat
        // singleton (counter per kode/level — bukan koleksi penuh).
        diag_global().reset();
        let sink = DiagSink::new();
        sink.push(Diagnostic::new(
            DiagLevel::Error,
            DiagCode::UndefinedSignal,
            "forwarded",
        ));
        sink.push(Diagnostic::new(
            DiagLevel::Warning,
            DiagCode::WidthMismatchWarning,
            "forwarded2",
        ));
        // Agregat hanya bertambah — test paralel bisa menambah counter lain,
        // jadi assert ≥ (bukan ==).
        assert!(
            diag_global().code_count(DiagCode::UndefinedSignal) >= 1,
            "error harus tercatat di agregat global"
        );
        assert!(
            diag_global().level_count(DiagLevel::Warning) >= 1,
            "warning harus tercatat di agregat global"
        );
        diag_global().reset();
    }
}
