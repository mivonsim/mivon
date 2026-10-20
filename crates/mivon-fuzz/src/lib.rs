//! mivon-fuzz — fuzzer internal (dev-only), bukan CLI user.
//!
//! Pendekatan: fuzz berbasis **corpus project nyata** (cva6, openc910,
//! opentitan). Semua seed dari RTL asli — TANPA template sintetis.
//! Kampanye: ambil seed nyata → mutasi 0-4x → evaluasi oracle.

// Sesuai desain Cargo.toml: seluruh isi crate HANYA aktif saat feature `dev`
// (deps mivon-api/mivon-ir/mivon-parser optional, di-gate oleh `dev`).
// Tanpa gate ini `cargo test --workspace` (tanpa --features dev) compile
// lib mivon-fuzz polos → unresolved import `mivon_api`/`mivon_ir`/`mivon_parser`.
#![cfg(feature = "dev")]

pub mod corpus;
pub mod deps;
pub mod directed;
pub mod feedback;
pub mod judge;
pub mod ledger;
pub mod minimize;
pub mod mutator;
pub mod oracle;
pub mod oracle_icarus;
pub mod runner;
pub mod triage;
pub mod validate;

// ── LRM Judge system ────────────────────────────────────────────────────────
pub mod lrm_judge;
pub mod lrm_model;
pub mod lrm_rules;
pub mod observer;
pub mod regression;
pub mod testcase;
pub mod verdict;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

pub const VERSION: &str = "0.1.0";

/// Target pipeline stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Target {
    #[default]
    All,
    Lexer,
    Parser,
    Elaborator,
    Simulator,
    Fmt,
    Cli,
    /// Preprocessor: directive `ifdef/`define/`include — determinisme +
    /// kebocoran macro + stack imbalance.
    Preproc,
    /// Transpiler MV → SV: determinisme + output parseable.
    Mv,
    /// VCD waveform pipeline: mwave (stats/tree/search/export/compare/
    /// filter/merge/get) atas VCD hasil sim yang di-mutasi — parser VCD
    /// robustness + determinisme.
    Vcd,
    /// SDF timing annotation pipeline (SIM-09): `SdfData::parse` + `--sdf`
    /// annotate + sim — area BELUM tersentuh (parser SDF tidak pernah di-fuzz).
    /// Mutasi teks SDF + SV seed → subprocess `mivon <sv> --sdf <sdf> -T 200`.
    Sdf,
    /// MICD incremental database (`--fast` run_fast): incremental vs
    /// `--recompile` (fresh) harus identik — cache drift / silent
    /// miscompilation = bug. Area BELUM tersentuh.
    Micd,
    /// Synthesis pipeline (`mivon synth --check-only`): parse → RTL→SIR
    /// lowering (`mivon-sir`) → SYN-1..9 sintesizability check. SIR lowering
    /// parser tak pernah di-fuzz. O1 no-crash + O4 double-run.
    Synth,
    /// Differential AST struktural (`mcheck a.sv --ast-diff b.sv` vs
    /// `b.sv --ast-diff a.sv`): HIMPUNAN pasangan diff harus simetris A→B ==
    /// B→A (swap membalik arah "only-in-A/B", TAPI himpunan (kind, node, loc)
    /// pasangan identik) — determinisme + recovery simetri. Area BARU —
    /// AST-diff report tak pernah di-fuzz dan `mcheck --ast-diff` memakai
    /// HashSet-iteration di render (SYN-9-class nondeterminisme kembali masuk
    /// lewat pintu lain).
    Astdiff,
    /// Hakim LRM IEEE 1800 (`judge_single`): seed mutasi → observasi black-box +
    /// state struktural → registry aturan penuh. `Violation` /
    /// `MivonInternalFailure` yang layak-simpan = bug (kepatuhan LRM, bukan
    /// sekadar no-crash/konsistensi internal O1-O5).
    Judge,
}

impl Target {
    pub const ALL: &[Target] = &[
        Target::Lexer,
        Target::Parser,
        Target::Elaborator,
        Target::Simulator,
        Target::Fmt,
        Target::Cli,
        Target::Preproc,
        Target::Mv,
        Target::Vcd,
        Target::Sdf,
        Target::Micd,
        Target::Synth,
        Target::Astdiff,
        Target::Judge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Target::All => "all",
            Target::Lexer => "lexer",
            Target::Parser => "parser",
            Target::Elaborator => "elab",
            Target::Simulator => "sim",
            Target::Fmt => "fmt",
            Target::Cli => "cli",
            Target::Preproc => "preproc",
            Target::Mv => "mv",
            Target::Vcd => "vcd",
            Target::Sdf => "sdf",
            Target::Micd => "micd",
            Target::Synth => "synth",
            Target::Astdiff => "astdiff",
            Target::Judge => "judge",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "all" => Some(Target::All),
            "lexer" | "lex" => Some(Target::Lexer),
            "parser" | "parse" => Some(Target::Parser),
            "elab" | "elaborator" => Some(Target::Elaborator),
            "sim" | "simulator" | "run" => Some(Target::Simulator),
            "fmt" => Some(Target::Fmt),
            "cli" => Some(Target::Cli),
            "preproc" | "pp" => Some(Target::Preproc),
            "mv" | "transpile" => Some(Target::Mv),
            "vcd" | "wave" => Some(Target::Vcd),
            "sdf" => Some(Target::Sdf),
            "micd" | "fast" => Some(Target::Micd),
            "synth" | "sir" => Some(Target::Synth),
            "astdiff" | "ast-diff" => Some(Target::Astdiff),
            "judge" | "lrm" => Some(Target::Judge),
            _ => None,
        }
    }
}

/// Kategori hasil kasus fuzz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Ok,
    /// Error diagnostik yang valid (bukan bug).
    CleanError,
    // === Bug categories ===
    /// Crash: panic, abort, segfault.
    Panic,
    /// Process exit non-zero tanpa panic (kecuali clean error).
    Abort,
    /// Hang: timeout tanpa selesai.
    Hang,
    /// Diagnostik hilang: error seharusnya ada tapi tidak muncul.
    DiagMissing,
    /// Lokasi diagnostik di LUAR rentang sumber: error melaporkan
    /// `file:line:col` yang line-nya > jumlah baris sumber (+1 EOF) atau
    /// col < 1 — line/col wajib 100% akurat menunjuk lokasi masalah.
    DiagLocRange,
    /// Round-trip mismatch: fmt(fmt(s)) != fmt(s).
    RoundtripMismatch,
    /// Hasil berbeda antar run (non-deterministic).
    NonDeterministic,
    /// Guard bypass: assertion/safety property dilanggar.
    GuardBypass,
    /// Differential: hasil beda antar engine path.
    Differential,
    /// Hasil sim VALID secara konsistensi (det+diff ok) TAPI perlu perhatian
    /// manusia: stimulus ada namun banyak signal tetap X/Z di akhir sim —
    /// bisa wajar (undriven) atau bukti state-propagation bug. BUKAN hard bug.
    Suspicious,
    /// Lewat timeout TAPI selesai dalam grace (3× subprocess / 2× watchdog
    /// in-process). Replay saat mesin sepi membuktikan selesai normal →
    /// SLOW, bukan hang. BUKAN bug (dulu semua terhitung Hang → noise
    /// kampanye proporsional beban CPU & ukuran seed).
    Slow,
    /// BUG MENYAMAR sbg warning/error — output mengandung pola KUAT
    /// inkonsistensi internal: level warning dgn kode error (`warning[E...]`),
    /// "internal error", "corrupt", "inconsistent", "must not happen",
    /// severity terbalik (`error[WR...`). Tak mungkin keputusan by-design →
    /// dianggap BUG (is_bug true, tersave utk review).
    HiddenBug,
    /// Mivon MENYERAH DIAM pada kasus ini (degradasi terekspresikan):
    /// "fallback", "treated as", "taking true branch", "using null default",
    /// "cannot be resolved", "returning 0", "stub", "skipped",
    /// "belum didukung". BUKAN bug hard (banyak keputusan degrade by-design)
    /// TAPI dihitung & tampil di summary kampanye supaya lonjakan degradasi
    /// cepat ketahuan (dulu semua kategori Ok → senyap).
    Degraded,
    /// Pelanggaran LRM IEEE 1800 menurut hakim (`judge_single` verdict
    /// `Violation` layak-simpan): kepatuhan, bukan sekadar no-crash.
    LrmViolation,
}

impl Category {
    pub fn is_bug(self) -> bool {
        matches!(
            self,
            Category::Panic
                | Category::Abort
                | Category::Hang
                | Category::DiagMissing
                | Category::DiagLocRange
                | Category::RoundtripMismatch
                | Category::NonDeterministic
                | Category::GuardBypass
                | Category::Differential
                | Category::HiddenBug
                | Category::LrmViolation
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Ok => "ok",
            Category::CleanError => "clean_error",
            Category::Panic => "panic",
            Category::Abort => "abort",
            Category::Hang => "hang",
            Category::DiagMissing => "diag_missing",
            Category::DiagLocRange => "diag_loc_range",
            Category::RoundtripMismatch => "roundtrip_mismatch",
            Category::NonDeterministic => "nondeterministic",
            Category::GuardBypass => "guard_bypass",
            Category::Differential => "differential",
            Category::Suspicious => "suspicious",
            Category::Slow => "slow",
            Category::HiddenBug => "hidden_bug",
            Category::Degraded => "degraded",
            Category::LrmViolation => "lrm_violation",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Category::Ok),
            "clean_error" => Some(Category::CleanError),
            "panic" => Some(Category::Panic),
            "abort" => Some(Category::Abort),
            "hang" => Some(Category::Hang),
            "diag_missing" => Some(Category::DiagMissing),
            "diag_loc_range" => Some(Category::DiagLocRange),
            "roundtrip_mismatch" => Some(Category::RoundtripMismatch),
            "nondeterministic" => Some(Category::NonDeterministic),
            "guard_bypass" => Some(Category::GuardBypass),
            "differential" => Some(Category::Differential),
            "suspicious" => Some(Category::Suspicious),
            "slow" => Some(Category::Slow),
            "hidden_bug" => Some(Category::HiddenBug),
            "degraded" => Some(Category::Degraded),
            "lrm_violation" => Some(Category::LrmViolation),
            _ => None,
        }
    }
}

/// Oracle identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Oracle {
    /// Tidak ada crash/panic/abort/hang.
    O1NoCrash,
    /// Diagnostik punya lokasi file:line:col.
    O2DiagLocation,
    /// Round-trip: fmt(fmt(s)) == fmt(s).
    O3Roundtrip,
    /// Determinism: run dua kali hasil sama.
    O4Determinism,
    /// Differential: hasil beda antar engine path.
    O5Differential,
    /// Kepatuhan LRM: hakim IEEE 1800 atas observasi + state struktural.
    O6LrmJudge,
}

impl Oracle {
    pub fn as_str(self) -> &'static str {
        match self {
            Oracle::O1NoCrash => "O1-no-crash",
            Oracle::O2DiagLocation => "O2-diag-location",
            Oracle::O3Roundtrip => "O3-roundtrip",
            Oracle::O4Determinism => "O4-determinism",
            Oracle::O5Differential => "O5-differential",
            Oracle::O6LrmJudge => "O6-lrm-judge",
        }
    }
}

/// Hasil satu kasus fuzz.
#[derive(Debug, Clone)]
pub struct CaseResult {
    pub target: Target,
    pub category: Category,
    pub oracle: &'static str,
    pub detail: String,
    pub source: String,
    /// Keys coverage hasil evaluasi (`ir|k|*`, `lex|t|*`, sim pakai
    /// engine-coverage sendiri). Kosong = target tak menyumbang.
    /// Diproses `run_single` via `feedback::record_keys` → input
    /// baru-menarik disimpan sbg seed tumbuh. Dulu HANYA jalur sim
    /// menyumbang — corpus tumbuh asimetris, target lain buta sinyal.
    pub cov_keys: Vec<String>,
}

impl CaseResult {
    /// Signature untuk dedup (target|oracle|category|2-baris-detail).
    ///
    /// Dua baris pertama (bukan satu): detail differential icarus/engine
    /// selalu berawalan baris generik yang SAMA ("SEMANTIC MISMATCH vs
    /// iverilog:" / "differential default vs ...") — baris-2 (`ref : ...`)
    /// yang membedakan divergensi. Satu baris = semua mismatch runtuh jadi
    /// satu grup, bug berbeda terkubur (temuan triage tahap 2). Baris-2
    /// dipotong 160 char agar signature tetap ringkas; detail 1-baris tetap
    /// satu grup per nilai baris-1 (pengelompokan tak berubah, walau string
    /// bertambah sufiks `|`).
    pub fn signature(&self) -> String {
        let mut lines = self.detail.lines();
        let first_line = lines.next().unwrap_or("");
        let second_line = lines.next().unwrap_or("");
        let second_trunc: String = second_line.chars().take(160).collect();
        format!(
            "{}|{}|{}|{}|{}",
            self.target.as_str(),
            self.oracle,
            self.category.label(),
            first_line,
            second_trunc
        )
    }
}

/// Konfigurasi kampanye fuzz.
#[derive(Debug, Clone)]
pub struct FuzzConfig {
    pub target: Target,
    pub cases: usize,
    pub seed: u64,
    pub timeout_ms: u64,
    pub corpus_dir: Option<PathBuf>,
    pub save_bugs: bool,
    /// Jumlah worker paralel. `0` = auto (cores/2, clamp 1-8), `1` = serial
    /// (jalur lama), `N>1` = N worker (`--jobs N`). RNG per-kasus
    /// deterministik (`case_rng(seed, slot)`) — hasil TIDAK tergantung
    /// jadwal worker; permutasi urutan eksekusi tak mengubah hasil.
    pub jobs: usize,
}

impl Default for FuzzConfig {
    fn default() -> Self {
        Self {
            target: Target::All,
            cases: 2000,
            seed: 0xC0FFEE,
            // Budget per-case sengaja LONG (5s): subprocess mivon CLI (sim/sdf/
            // cli/vcd/micd) harus full-parse + elaborasi per case — jalur elab
            // ERROR terukur 2-3.5s untuk file 4KB (pair 21/24/27 kampanye SDF).
            // Timeout pendek (1.5-2s) = false hang massal (terukur hang=99/400
            // palsu). 5s menangkap hang ASLI (infinite loop) tanpa noise speed.
            timeout_ms: 5000,
            corpus_dir: None,
            save_bugs: true,
            jobs: 0,
        }
    }
}

/// Resolusi jumlah worker: `0` = auto = cores/2 clamp 1-8 (kompromi —
/// evaluator in-process CPU-bound penuh di watchdog thread; oversubscribe
/// 1:1 membuat semua worker lambat + false-hang grace; subprocess target
/// toleran tapi tak dipilih kasus-demi-kasus), `1` = serial, `N` = N worker.
pub fn case_jobs(jobs: usize) -> usize {
    match jobs {
        0 => std::thread::available_parallelism()
            .map(|n| n.get().saturating_div(2))
            .unwrap_or(1)
            .clamp(1, 8),
        n => n.max(1),
    }
}

/// RNG per-kasus deterministik: setiap slot kasus punya stream sendiri
/// (seed XOR splitmix(slot)) — hasil kampanye identik serial vs paralel,
/// jadwal worker tak boleh mengubah mutasi/evaluasi mana pun.
pub fn case_rng(seed: u64, slot: usize) -> Rng {
    let mut m = seed ^ 0x9E3779B97F4A7C15;
    m = m.wrapping_add((slot as u64).wrapping_mul(0xD1B54A32D192ED03));
    Rng::new(m)
}

/// Ringkasan per target.
#[derive(Debug, Default, Clone)]
pub struct TargetSummary {
    pub target: Target,
    pub total: usize,
    pub bugs: usize,
    /// Kasus bersumber raksasa (> SIZE_CAP) yang TETAP dievaluasi (bukan
    /// di-skip) dengan budget waktu lebih longgar. Dihitung agar kampanye
    /// tahu persis berapa banyak seed besar yang dijamah — bukan hilang
    /// senyap. (Dulu `skipped_big` memang di-`continue` → coverage terbuang.)
    pub large_evaluated: usize,
    /// Hang yg TERNYATA selesai saat replay tenang (6× budget) → dibuang
    /// dari hitungan bug (slow ≠ hang; review kampanye 2026-09-29).
    pub dismissed_hangs: usize,
    pub categories: std::collections::BTreeMap<Category, usize>,
}

/// Cap ukuran source pasca-mutasi. Corpus seed max 4MB, tapi duplicate_chunk
/// bisa meledakkan seed jadi 267MB (terukur kampanye SDF: hang=93/300 = noise
/// blowup, bukan bug). Input > cap tidak dievaluasi (skip → skipped_big).
pub const SIZE_CAP: usize = 2_000_000;

/// Laporan kampanye fuzz.
#[derive(Debug, Default, Clone)]
pub struct FuzzReport {
    pub bugs: Vec<CaseResult>,
    pub summaries: Vec<TargetSummary>,
}

/// Deterministic PRNG (splitmix64, 16-iter warmup seperti mvm-fuzz).
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Self { state: seed };
        // 16-iter warmup
        for _ in 0..16 {
            rng.next_u64();
        }
        rng
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Return value in [0, bound).
    pub fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() as usize) % bound
    }

    /// Probability in [0, 100).
    pub fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }

    /// Pick random element.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// i64 in range.
    pub fn i64_in(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() as i64 % (hi - lo + 1).max(1))
    }
}

/// Counter global crash sequence.
static CRASH_SEQ: AtomicU32 = AtomicU32::new(0);

pub fn next_crash_seq() -> u32 {
    CRASH_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Jalur fuzz default.
pub fn bugs_dir() -> PathBuf {
    PathBuf::from(".mivon-fuzz-bugs")
}

/// Jalur corpus default.
pub fn default_corpus_dir() -> PathBuf {
    PathBuf::from("crates/mivon-fuzz/fuzz/corpus/seeds")
}

/// Jalur reports.
pub fn reports_dir() -> PathBuf {
    PathBuf::from("fuzz/reports")
}

/// Jalur bug database.
pub fn bugdb_path() -> PathBuf {
    PathBuf::from(".mivon-fuzz-bugdb.json")
}

/// Satu baris ringkasan per target. Dipakai real-time di `run()` (stderr)
/// dan ringkasan akhir di `main.rs::cmd_run` (stdout) — jangan duplikasi
/// formatnya di dua tempat.
pub fn summary_line(s: &TargetSummary) -> String {
    let mut parts: Vec<String> = s
        .categories
        .iter()
        .map(|(c, n)| format!("{}={}", c.label(), n))
        .collect();
    parts.sort();
    // dismissed_hangs: hang yg selesai saat replay tenang — tampil bila ada.
    let dismissed = if s.dismissed_hangs > 0 {
        format!(" dismissed={}", s.dismissed_hangs)
    } else {
        String::new()
    };
    format!(
        "[{}] total={} bugs={} large={}{} :: {}",
        s.target.as_str(),
        s.total,
        s.bugs,
        s.large_evaluated,
        dismissed,
        parts.join(" ")
    )
}

/// Run kampanye fuzz.
///
/// Corpus di-load SEKALI untuk seluruh kampanye (dulu per-target = 14×
/// rebuild deps-index yang membaca semua file corpus — memori/waktu
/// terbuang; terukur swap-thrash di mesin 7GB).
pub fn run(cfg: FuzzConfig) -> FuzzReport {
    let corpus = corpus::Corpus::load(cfg.corpus_dir.as_deref());
    if corpus.is_empty() {
        eprintln!(
            "WARNING: corpus kosong di {:?} — tidak ada seed nyata",
            cfg.corpus_dir.clone().unwrap_or_else(default_corpus_dir)
        );
    }
    match cfg.target {
        Target::All => {
            let mut merged = FuzzReport::default();
            for &target in Target::ALL {
                // Guard OOM thread bocor: budget thread bocor SATU proses
                // kampanye sudah habis di target sebelumnya → sisa target
                // dilewati SEBELUM evaluasi (tanpa ini, tiap target tersisa
                // masih mengevaluasi 1 kasus — bisa hang lagi = bocoran
                // tambahan). Tanpa reset per-target: reset di awal tiap target
                // akan melipatgandakan cap total = risiko OOM × jumlah target.
                if oracle::leaked_watchdogs() >= oracle::WATCHDOG_LEAK_CAP {
                    eprintln!(
                        "\n  (guard: {} hang asli >= cap {} — target {:?} dilewati; budget thread bocor kampanye habis)",
                        oracle::leaked_watchdogs(),
                        oracle::WATCHDOG_LEAK_CAP,
                        target
                    );
                    break;
                }
                let mut single_cfg = cfg.clone();
                single_cfg.target = target;
                let report = run_single(single_cfg, &corpus);
                // Ringkasan real-time per target: dulu hanya muncul di AKHIR
                // run() seluruh 14 target → triage antar-wake tidak melihat
                // target mana yang menghasilkan bug tanpa menunggu selesai.
                for s in &report.summaries {
                    eprintln!("{}", summary_line(s));
                }
                merged.bugs.extend(report.bugs);
                merged.summaries.extend(report.summaries);
            }
            merged
        }
        _ => run_single(cfg, &corpus),
    }
}

/// Concat sumber tb dengan module pasangannya (`tb_<mod>.sv` → cari
/// `<mod>.sv`/`<mod>.mv` di corpus). Bila pasangan tidak ada, gunakan tb apa
/// adanya (bisa jadi tb mandiri).
fn combine_tb_with_module(
    tb_path: &std::path::Path,
    tb_src: &str,
    corpus: &corpus::Corpus,
) -> String {
    let tb_name = tb_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let module_name = tb_name.strip_prefix("tb_").unwrap_or(&tb_name).to_string();

    // Cari pasangan module di corpus (path lain).
    for i in 0..corpus.len() {
        if let Some(p) = corpus.seed_at(i) {
            if p == tb_path {
                continue;
            }
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name == module_name {
                if let Ok(mod_src) = std::fs::read_to_string(p) {
                    let mod_src = if p
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e == "mv")
                        .unwrap_or(false)
                    {
                        // MV pasangan — transpile via jalur yang sama.
                        crate::corpus::transpile_seed(p).unwrap_or(mod_src)
                    } else {
                        mod_src
                    };
                    return format!("{}\n{}", mod_src, strip_duplicate_modules(tb_src, &mod_src));
                }
            }
        }
    }
    tb_src.to_string()
}

/// Strip deklarasi module/interface dari TB yang sudah ada di mod_src
/// (Hapus `module <name> ... endmodule` untuk semua nama yang muncul di mod_src
///  supaya concat menghasilkan 1 set module tak duplikat).
fn strip_duplicate_modules(tb_src: &str, mod_src: &str) -> String {
    // Kumpulkan semua nama module/interface di mod_src.
    let mut defined = std::collections::HashSet::new();
    for line in mod_src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("module ") {
            if let Some(name) = rest.split(|c: char| c.is_whitespace()).next() {
                if !name.is_empty() && !name.starts_with('#') {
                    defined.insert(name.to_string());
                }
            }
        }
        if let Some(rest) = t.strip_prefix("interface ") {
            if let Some(name) = rest.split(|c: char| c.is_whitespace()).next() {
                if !name.is_empty() && !name.starts_with('#') {
                    defined.insert(name.to_string());
                }
            }
        }
    }
    if defined.is_empty() {
        return tb_src.to_string();
    }
    // Bersihkan blok `module <dup_name> ... endmodule` / `interface <dup> ... endinterface`.
    let mut out = String::with_capacity(tb_src.len());
    let mut skip_until_end: Option<&str> = None; // "module"/"interface"
    for line in tb_src.lines() {
        let t = line.trim();
        if skip_until_end.is_some() {
            // Skip sampai keyword penutup modul/interface yang sesuai.
            if t.starts_with("endmodule") || t.starts_with("endinterface") {
                skip_until_end = None;
            }
            continue;
        }
        let stripped_for_check = t.trim_start();
        let mut should_skip = false;
        for (kw, _close) in [("module", "endmodule"), ("interface", "endinterface")] {
            if let Some(rest) = stripped_for_check.strip_prefix(kw) {
                if rest.starts_with(char::is_whitespace) || rest.starts_with('(') {
                    // `module X ...` or `module #(P) X ...`
                    let name_token = rest
                        .trim_start()
                        .split(|c: char| c.is_whitespace() || c == '#' || c == '(')
                        .next()
                        .unwrap_or("");
                    if defined.contains(name_token) {
                        should_skip = true;
                        skip_until_end = Some(if kw == "module" {
                            "module"
                        } else {
                            "interface"
                        });
                        break;
                    }
                }
            }
        }
        if should_skip {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Bangun rencana indeks seed untuk satu kampanye.
///
/// - `cases >= kandidat`: SEMUA kandidat dijamah minimal sekali (terurut),
///   sisa `cases - n` diambil acak (setiap seed dijamin "tanpa skip").
/// - `cases < kandidat`: budget disebar MERATA via stride ke SELURUH rentang
///   kandidat (`idx = k*n/cases`), bukan hanya `cases` pertama. Tanpa ini
///   proyek besar di ujung daftar (mis. opentitan ~3947 file) tak pernah
///   dijamah dalam kampanye default. Offset acak per kampanye merotasi titik
///   awal agar kampanye berulang mengakumulasi cakupan.
///
/// Fungsi murni (rng hanya untuk bagian acak) sehingga dapat diuji deterministik.
pub(crate) fn build_seed_plan(candidates: &[usize], cases: usize, rng: &mut Rng) -> Vec<usize> {
    if candidates.is_empty() || cases == 0 {
        return Vec::new();
    }
    let n = candidates.len();
    if n <= cases {
        // Semua seed dijamah minimal sekali, sisanya acak.
        let mut plan: Vec<usize> = candidates.to_vec();
        for _ in n..cases {
            plan.push(candidates[rng.below(n)]);
        }
        plan
    } else {
        // Lebih banyak seed dari kasus: sebar merata (stride) ke SELURUH
        // kandidat + rotasi acak titik awal.
        let start = rng.below(n);
        let mut plan = Vec::with_capacity(cases);
        for k in 0..cases {
            let idx = (start + k * n / cases) % n;
            plan.push(candidates[idx]);
        }
        plan
    }
}

/// Rencana kampanye sadar-ledger: seed BELUM pernah dievaluasi didahulukan.
///
/// - `unseen` dijamah lebih dulu (bila `cases < unseen.len()`, stride antar
///   unseen + rotasi → kampanye berulang mengakumulasi cakupan sampai habis).
/// - Sisa budget baru diisi dari `seen` (stride) — kampanye tetap penuh saat
///   seluruh seed baru sudah dijamah.
///
/// Fungsi murni (RNG hanya untuk bagian acak) — teruji deterministik.
pub(crate) fn build_prioritized_plan(
    unseen: &[usize],
    seen: &[usize],
    cases: usize,
    rng: &mut Rng,
) -> Vec<usize> {
    if unseen.is_empty() {
        // Semua seed sudah pernah dijamah → pakai rencana biasa atas seen.
        return build_seed_plan(seen, cases, rng);
    }
    if cases <= unseen.len() {
        // Budget belum cukup menjamah semua unseen → stride antar unseen
        // (rotasi titik awal; lintas kampanye mengakumulasi cakupan).
        return build_seed_plan(unseen, cases, rng);
    }
    // Seluruh unseen dijamah SEKALI (terurut), sisa budget diisi dari seed
    // yang sudah pernah — memperluas cakupan, bukan mengulang unseen.
    let rest = cases - unseen.len();
    let mut plan: Vec<usize> = unseen.to_vec();
    let mut fill = build_seed_plan(seen, rest, rng);
    if fill.len() < rest {
        // `seen` kosong/habis (mis. kampanye pertama, corpus < cases) →
        // ulang dari SEMUA kandidat supaya budget kampanye tetap penuh.
        let all: Vec<usize> = unseen.iter().chain(seen.iter()).copied().collect();
        fill.extend(build_seed_plan(&all, rest - fill.len(), rng));
    }
    plan.extend(fill);
    plan
}

/// Agregat hasil satu kasus — nilainya (diratakan lintas worker; dulu
/// mutasi langsung variabel loop).
struct CaseAgg {
    bugs: Vec<CaseResult>,
    categories: std::collections::BTreeMap<Category, usize>,
    large_evaluated: usize,
    dismissed_hangs: usize,
    ledger_marked: bool,
    /// Guard thread-bocor tercapai di kasus INI → pengulang kampanye
    /// (main loopBreak/stop workers).
    leak_cap_hit: bool,
}

impl CaseAgg {
    fn empty() -> Self {
        Self {
            bugs: Vec::new(),
            categories: std::collections::BTreeMap::new(),
            large_evaluated: 0,
            dismissed_hangs: 0,
            ledger_marked: false,
            leak_cap_hit: false,
        }
    }
}

/// State worker-pool shared (Arc) — penjemput slot atomik + stop flag guard.
struct Pool {
    next: std::sync::atomic::AtomicUsize,
    stop: std::sync::atomic::AtomicUsize,
}

/// Evaluasi SATU slot kasus — pipeline identik untuk jalur serial & worker
/// paralel (dulu satu blok loop ~200 baris; ekstraksi = prasyarat
/// paralelizasi TANPA duplikat semantik). Deterministik: RNG stream
/// `case_rng(cfg.seed, slot)` — jadwal worker tak mengubah mutasi/eval.
fn eval_case(
    cfg: &FuzzConfig,
    corpus: &corpus::Corpus,
    plan: &[usize],
    slot: usize,
    ledger: &std::sync::Mutex<crate::ledger::SeedLedger>,
    sigs: &std::sync::Mutex<std::collections::HashSet<String>>,
) -> CaseAgg {
    let mut agg = CaseAgg::empty();
    let mut rng = case_rng(cfg.seed, slot);

    // Early-guard: stop sudah dilipat (worker lain memandang cap) → kasus
    // ini TIDAK dievaluasi sama sekali — tanpa ini, jobs-1 worker menyisa
    // menambah leak ≤ (jobs-1)×256MB setelah guard trip.
    if oracle::leaked_watchdogs() >= oracle::WATCHDOG_LEAK_CAP {
        agg.leak_cap_hit = true;
        return agg;
    }

    // Ambil seed nyata dari corpus via plan (100% — tanpa template
    // sintetis). `.mv` return RAW source, transpile setelah mutasi.
    let Some(&seed_idx) = plan.get(slot) else {
        return agg;
    };
    let Some(base_seed) = corpus.seed_by_index(seed_idx) else {
        return agg;
    };
    let is_mv = base_seed.is_mv;
    // Kunci ledger seed ini (path|size|mtime) — ditandai setelah evaluasi.
    let ledger_key = crate::ledger::seed_key(&base_seed.path);

    // Siapkan base source:
    //   .sv + tb_pair → concat modul+tb
    //   lainnya → mentah (terhitung .mv juga — mutasi menyentuh .mv)
    let mut base_source = if !is_mv {
        let is_tb = base_seed
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.starts_with("tb_"))
            .unwrap_or(false);
        if is_tb {
            combine_tb_with_module(&base_seed.path, &base_seed.text, corpus)
        } else {
            base_seed.text.clone()
        }
    } else {
        // MV: simpan raw; transpile terjadi SETELAH mutasi
        base_seed.text.clone()
    };

    // Source raksasa: JANGAN di-skip. Mutasi PENUH dimatikan
    // (`duplicate_chunk`/splice bisa meledak 267MB → false hang massal),
    // TAPI 1x mutasi RINGAN tak-pengembang tetap jalan (dulu n_mut=0 =
    // dievaluasi mentah tanpa variasi = coverage gap).
    let is_large = base_source.len() > SIZE_CAP;
    if is_large {
        let mut mutator = mutator::Mutator::new(&mut rng);
        base_source = mutator.mutate_light(&base_source, corpus);
    } else {
        let n_mut = rng.below(5);
        let mut mutator = mutator::Mutator::new(&mut rng);
        for _ in 0..n_mut {
            base_source = mutator.mutate(&base_source, corpus);
        }
    }

    // Directed mutation: 30% kasus — serangan terarah SETELAH mutasi acak
    // (dilewati bila source raksasa, demi cegah blowup).
    if !is_large && rng.chance(30) {
        let mut dir = directed::DirectedMutator::new(&mut rng);
        base_source = dir.mutate(&base_source);
    }

    // CLAMP pasca-mutasi: rangkaian `duplicate_chunk`/splice pada seed
    // hampir-2MB bisa meledak puluhan MB (terukur: swap-thrash mesin
    // 7GB, kampanye merayap). Source hasil mutasi > SIZE_CAP → evaluasi
    // versi light (tak berkembang) — tetap ada variasi, tanpa blowup.
    if !is_large && base_source.len() > SIZE_CAP {
        let mut clamp = mutator::Mutator::new(&mut rng);
        base_source = clamp.mutate_light(&base_source, corpus);
    }

    // MV → transpile mutated MV ke SV sebelum evaluasi — KECUALI target
    // Mv: `evaluate_mv` mengharap source MV mentah (dia yang mentranspile
    // di dalam untuk cek determinisme + output parseable). Transpile di
    // sini membuat SV di-feed balik ke transpiler MV → 100% case jadi
    // "transpile error" (clean_error=2000/2000, kampanye MV tak berguna).
    let source = if is_mv && cfg.target != Target::Mv {
        let base_name = base_seed
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("design")
            .to_string();
        corpus::transpile_mv_string(&base_source, &base_name).unwrap_or_else(|_| {
            // Transpile gagal (mutasi merusak sintaks MV) — jadikan
            // placeholder error-clean agar tak terhitung sebagai bug.
            "// mv transpile failed\nmodule fz_mv_transpile_err; endmodule".to_string()
        })
    } else {
        base_source
    };

    // Env hook: tulis source pre-eval untuk repro
    if let Ok(trace_path) = std::env::var("MIVON_FUZZ_TRACE") {
        let _ = std::fs::write(&trace_path, &source);
    }

    // Source raksasa: TIDAK di-skip. Evaluasi dgn budget waktu lebih
    // longgar (parse/elab source besar butuh waktu lebih) agar tak
    // salah digolongkan hang.
    let eval_timeout = if source.len() > SIZE_CAP {
        agg.large_evaluated += 1;
        cfg.timeout_ms.saturating_mul(4)
    } else {
        cfg.timeout_ms
    };

    let mut result = oracle::evaluate(cfg.target, &source, eval_timeout);

    // Guard OOM thread bocor: cap tercapai → kategori tetap dihitung
    // (summary tak kehilangan kasus), tetapi seed TIDAK di-mark ledger,
    // TANPA O6-scan/feedback/save — identik semantik serial lama (HEAD~1):
    // kampanye berikutnya mengulang seed dgn replay-tenang + dismiss penuh.
    agg.leak_cap_hit = oracle::leaked_watchdogs() >= oracle::WATCHDOG_LEAK_CAP;
    *agg.categories.entry(result.category).or_insert(0) += 1;
    if agg.leak_cap_hit {
        return agg;
    }

    // Ledger: seed ini sudah dijamah kampanye (mark SETELAH evaluasi —
    // seed yang crash di tengah tetap dianggap belum selesai).
    // Poison recovery: konten set tetap valid — mark jangan hilang senyap.
    ledger
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .mark(&ledger_key);
    agg.ledger_marked = true;

    // O6 utk jalur IN-PROCESS: scan detail utk pola KUAT HiddenBug.
    if !result.category.is_bug() {
        if let Some((cat, detail)) = oracle::scan_hidden_diags(&result.detail) {
            if cat == Category::HiddenBug {
                result.category = cat;
                result.detail = detail;
            }
        }
    }

    // Feed balik coverage keys penyumbang oracle (compile/lexer in-process;
    // sim merekam sendiri via cov engine).
    if !result.cov_keys.is_empty() {
        let fresh = crate::feedback::record_keys(&result.cov_keys);
        if fresh > 0 && source.len() <= crate::feedback::INTERESTING_MAX_BYTES {
            let _ = crate::feedback::save_interesting(&source);
        }
    }

    if result.category.is_bug() {
        let sig = result.signature();
        // Poison recovery: isi set tetap valid (flag saja) → dedup tetap betul.
        let fresh = sigs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(sig);
        if fresh {
            // AUTO-DISMISS hang via replay tenang: replay dgn budget 6× —
            // tak selesai → hang asli (simpan sbg bug); selesai dgn kategori
            // non-bug → BUKAN bug (jangan simpan, jangan hitung).
            if result.category == Category::Hang {
                let calmer = oracle::evaluate(
                    result.target,
                    &result.source,
                    cfg.timeout_ms.saturating_mul(6),
                );
                if !calmer.category.is_bug() {
                    // Kategori pindah ke hasil replay (summary konsisten).
                    if let Some(c) = agg.categories.get_mut(&Category::Hang) {
                        *c = c.saturating_sub(1);
                    }
                    *agg.categories.entry(calmer.category).or_insert(0) += 1;
                    agg.dismissed_hangs += 1;
                    return agg;
                }
            }
            if cfg.save_bugs {
                save_crash(&result, slot);
            }
            agg.bugs.push(result);
        }
    }
    agg
}

/// Gabung satu CaseAgg ke agregat kampanye (main thread — dulu dilakukan
/// inline di loop). Return = leak_cap_hit.
fn merge_case(
    agg: CaseAgg,
    results: &mut Vec<CaseResult>,
    categories: &mut std::collections::BTreeMap<Category, usize>,
    large_evaluated: &mut usize,
    dismissed_hangs: &mut usize,
) -> bool {
    for (c, n) in agg.categories {
        *categories.entry(c).or_insert(0) += n;
    }
    *large_evaluated += agg.large_evaluated;
    *dismissed_hangs += agg.dismissed_hangs;
    results.extend(agg.bugs);
    agg.leak_cap_hit
}

fn run_single(cfg: FuzzConfig, corpus: &corpus::Corpus) -> FuzzReport {
    use std::collections::BTreeMap;

    // Ledger seed ter-evaluasi (persisten lintas kampanye): kampanye
    // memprioritaskan seed yang BELUM pernah dijamah — target "semua seed
    // dijalankan tanpa ada yang di-skip" tercapai secara akumulatif.
    let ledger = std::sync::Mutex::new(crate::ledger::SeedLedger::load());

    let mut rng = Rng::new(cfg.seed); // hanya utk seed-plan (serial, deterministic)

    let mut results: Vec<CaseResult> = Vec::new();
    let mut dismissed_hangs: usize = 0;
    let mut categories: BTreeMap<Category, usize> = BTreeMap::new();
    let seen_sigs = std::sync::Mutex::new(std::collections::HashSet::<String>::new());
    let mut large_evaluated: usize = 0;

    if corpus.skipped_large > 0 {
        eprintln!(
            "  (corpus: {} seed raksasa >= {}MB di-hitung, tak dibuang senyap)",
            corpus.skipped_large,
            crate::corpus::MAX_SEED_BYTES / (1024 * 1024)
        );
    }

    // Seed-coverage + LEDGER: seed yang belum pernah dievaluasi (lintas
    // kampanye) didahulukan; sisa budget diisi dari seed yang sudah pernah.
    // Bila kasus >= kandidat, tiap seed tetap dijamah minimal sekali.
    let (unseen, seen): (Vec<usize>, Vec<usize>) = {
        let candidates: Vec<usize> = if cfg.target == Target::Mv {
            (0..corpus.len())
                .filter(|&i| {
                    corpus
                        .seed_at(i)
                        .map(|p| p.extension().and_then(|e| e.to_str()) == Some("mv"))
                        .unwrap_or(false)
                })
                .collect()
        } else {
            (0..corpus.len()).collect()
        };
        let mut unseen = Vec::new();
        let mut seen = Vec::new();
        for &i in &candidates {
            let key = corpus.seed_at(i).map(crate::ledger::seed_key);
            let seen_now = key
                .as_ref()
                .is_some_and(|k| {
                    ledger
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .is_seen(k)
                });
            match key {
                Some(_) if seen_now => seen.push(i),
                _ => unseen.push(i),
            }
        }
        eprintln!(
            "  (ledger: {} unseen, {} seen dari {} kandidat seed)",
            unseen.len(),
            seen.len(),
            candidates.len()
        );
        (unseen, seen)
    };
    let seed_plan = build_prioritized_plan(&unseen, &seen, cfg.cases, &mut rng);
    // Kadens progres: kampanye kecil tetap terlihat di stderr (tiap 25
    // kasus); kampanye besar tiap 500.
    let progress_every = if cfg.cases <= 500 { 25 } else { 500 };

    let jobs = case_jobs(cfg.jobs);

    if jobs <= 1 {
        // ── Jalur serial (jalur lama) ──
        let cases = cfg.cases.min(seed_plan.len());
        for slot in 0..cases {
            let agg = eval_case(&cfg, corpus, &seed_plan, slot, &ledger, &seen_sigs);
            if merge_case(
                agg,
                &mut results,
                &mut categories,
                &mut large_evaluated,
                &mut dismissed_hangs,
            ) {
                eprintln!(
                    "\n  (guard: {} hang asli >= cap {} — thread stack bocor 256MB/kasus; kampanye dihentikan bersih)",
                    oracle::leaked_watchdogs(),
                    oracle::WATCHDOG_LEAK_CAP
                );
                break;
            }
            if (slot + 1) % progress_every == 0 {
                let _ = ledger
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .save();
                eprint!(
                    "\r  [{}/{}] bugs: {} large: {}",
                    slot + 1,
                    cases,
                    results.len(),
                    large_evaluated
                );
            }
        }
    } else {
        // ── Worker pool paralel: N worker menjemput slot dari plan ──
        // Determinisme tetap: RNG stream per-slot (`case_rng`), plan serial
        // → hasil tidak tergantung jadwal worker, hanya slot yang dijamah
        // berbeda urutan (ekuivalen — tidak ada mutasi state lintas slot).
        eprintln!(
            "  (parallel: {} workers; per-slot RNG deterministik — hasil = serial)",
            jobs
        );
        let pool = std::sync::Arc::new(Pool {
            next: std::sync::atomic::AtomicUsize::new(0),
            stop: std::sync::atomic::AtomicUsize::new(0),
        });
        let cases = cfg.cases.min(seed_plan.len());
        let (tx, rx) = std::sync::mpsc::channel::<CaseAgg>();
        // Borrow kembar (thread::scope: lifetime terikat scope — no 'static).
        let cfg_ref = &cfg;
        let corpus_ref = corpus;
        let plan_ref = &seed_plan;
        let ledger_ref = &ledger;
        let sigs_ref = &seen_sigs;
        std::thread::scope(|scope| {
            for _ in 0..jobs {
                let pool = pool.clone();
                let tx = tx.clone();
                let _worker = scope.spawn(
                    move || loop {
                        if pool.stop.load(Ordering::Relaxed) != 0 {
                            break;
                        }
                        let slot = pool.next.fetch_add(1, Ordering::AcqRel);
                        if slot >= cases {
                            break;
                        }
                        let agg =
                            eval_case(cfg_ref, corpus_ref, plan_ref, slot, ledger_ref, sigs_ref);
                        if agg.leak_cap_hit {
                            pool.stop.store(1, Ordering::Relaxed);
                        }
                        if tx.send(agg).is_err() {
                            break;
                        }
                    },
                );
            }
            drop(tx);
        });
        // Merge hasil worker (main thread).
        let mut merged = 0usize;
        for agg in rx {
            merged += 1;
            if merge_case(
                agg,
                &mut results,
                &mut categories,
                &mut large_evaluated,
                &mut dismissed_hangs,
            ) {
                eprintln!(
                    "\n  (guard: {} hang asli >= cap {} — thread stack bocor 256MB/kasus; kampanye dihentikan bersih)",
                    oracle::leaked_watchdogs(),
                    oracle::WATCHDOG_LEAK_CAP
                );
            }
            if merged % progress_every == 0 {
                let _ = ledger
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .save();
                eprint!(
                    "\r  [{}/{}] bugs: {} large: {}",
                    merged, cases, results.len(), large_evaluated
                );
            }
        }
    }

    if cfg.cases >= progress_every {
        eprintln!();
    }
    if let Err(e) = ledger
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .save()
    {
        eprintln!("  (ledger: gagal simpan: {e})");
    }
    crate::feedback::flush();
    let (n_total, n_new) = ledger
        .lock()
        .map(|l| (l.len(), l.new_marks()))
        .unwrap_or((0, 0));
    eprintln!("  (ledger: {} kunci total, +{} baru kampanye ini)", n_total, n_new);

    let summary = TargetSummary {
        target: cfg.target,
        total: cfg.cases,
        bugs: results.len(),
        large_evaluated,
        dismissed_hangs,
        categories,
    };

    FuzzReport {
        bugs: results,
        summaries: vec![summary],
    }
}

/// Simpan crash ke .mivon-fuzz-bugs/
fn save_crash(result: &CaseResult, iter: usize) {
    let dir = bugs_dir();
    let _ = std::fs::create_dir_all(&dir);

    let seq = next_crash_seq();
    let kind = result.category.label();
    let stem = artifact_stem(&result.signature(), kind);
    let path = dir.join(format!("{stem}.sv"));
    let _ = std::fs::write(&path, &result.source);

    // Metadata .txt — `target:` ditambahkan utk replay/auto-verify lanjutan
    // (dulu tak ada → reclassify hang butuh tebak target dari signature).
    let meta_path = dir.join(format!("{stem}.txt"));
    let meta = format!(
        "oracle: {}\ncategory: {}\ntarget: {}\nsignature: {}\ndetail: {}\niter: {}\nseq: {}\n",
        result.oracle,
        result.category.label(),
        result.target.as_str(),
        result.signature(),
        result.detail.lines().next().unwrap_or(""),
        iter,
        seq,
    );
    let _ = std::fs::write(&meta_path, meta);

    // Bug DB otomatis (.mivon-fuzz-bugdb.json) — dulu `bugdb_path()` tak
    // pernah ditulis siapa pun (DB fiktif). Append entry format lama
    // {"entries":[{kind,source,detail,seed,iter,t}]} tanpa dependency JSON.
    append_bugdb(
        kind,
        &result.source,
        result.detail.lines().next().unwrap_or(""),
        iter,
    );
}

/// Hash FNV-1a 64-bit (murni, tanpa dependency) — nama artefak bug.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

/// Nama dasar (stem) artefak bug: hash signature + kategori.
///
/// Stabil untuk temuan identik (re-find = file sama, tidak menumpuk) dan
/// unik antar temuan berbeda. Menggantikan `bug_{seq}_{kind}` lama yang
/// seq-nya reset tiap proses → artefak bisa tertimpa senyap.
fn artifact_stem(signature: &str, kind: &str) -> String {
    format!("bug_{:012x}_{}", fnv1a(signature) & 0xFFFF_FFFF_FFFF, kind)
}

/// Escape string → aman utk string JSON (quote/backslash/control char).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Sisip satu entry ke konten bugdb (LOGIKA MURNI — diuji unit test).
/// Format lama `{"entries":[...]}`: sisip SEBELUM `]` penutup; `rfind("}]")`
/// aman karena field terakhir entry (`t`) selalu mendahului penutup.
fn bugdb_insert_entry(content: &str, entry: &str) -> String {
    let mut content = content.to_string();
    // Penutup format lama = `]}`; index `]` = len-2 (trim trailing ws).
    // Guard AWAL: konten korup yg kebetulan diakhiri `]}` ("not json ]}")
    // tidak boleh dianggap format valid.
    let insert_at = if content.starts_with("{\"entries\":[")
        && content.trim_end().strip_suffix("]}").is_some()
    {
        content.trim_end().len() - 2 // index `]` → sisip sebelum `]`
    } else {
        // File rusak/bukan format lama → struktur valid baru (entry lama
        // di file korup dibiarkan hilang, bukan menulis JSON pecah).
        content = "{\"entries\":[]}".to_string();
        content.len() - 2 // index `]` (14-2=12)
    };
    // Entries lama = ada `{` di antara `[{` dan `]`.
    let has_entries = content[11..insert_at].contains('{');
    let glue = if has_entries { "," } else { "" };
    content.insert_str(insert_at, &format!("{glue}{entry}"));
    content
}

/// Lock global append bugdb: read-modify-write sesudah applicant worker
/// paralel — tanpa lock, rename terakhir menutin (entry bug hilang) atau
/// penyimpsangan tulis `File::create` membuat pojok shared korup.
static BUGDB_MTX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Append satu entry ke bug database (format lama dihormati; tulis atomik
/// via temp+rename ala MICD). Gagal = diam (bug utama tetap ke-save ke .sv).
fn append_bugdb(kind: &str, source: &str, detail: &str, iter: usize) {
    use std::io::Write as _;
    // Poison recovery: konten tetap valid (flag saja) → lanjutkan.
    let _guard = BUGDB_MTX.lock().unwrap_or_else(|p| p.into_inner());
    let path = crate::bugdb_path();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = format!(
        "{{\"kind\":\"{}\",\"source\":\"{}\",\"detail\":\"{}\",\"seed\":0,\"iter\":{},\"t\":{}}}",
        json_escape(kind),
        json_escape(source),
        json_escape(detail),
        iter,
        t
    );
    let content = std::fs::read_to_string(&path).unwrap_or_else(|_| "{\"entries\":[]}".to_string());
    let content = bugdb_insert_entry(&content, &entry);
    if let Some(tmp) = path.parent() {
        let _ = std::fs::create_dir_all(tmp);
    }
    let tmp_path = path.with_extension("json.tmp");
    let mut f = match std::fs::File::create(&tmp_path) {
        Ok(f) => f,
        Err(_) => return,
    };
    if f.write_all(content.as_bytes()).is_err() {
        return;
    }
    drop(f);
    let _ = std::fs::rename(&tmp_path, &path);
}

#[cfg(test)]
mod bugdb_tests {
    use super::*;

    /// json_escape wajib menutup quote/backslash/newline (source berisi
    /// kode SV dgn string literal → kalau lolos, JSON bugdb pecah).
    #[test]
    fn json_escape_escapes_dangerous_chars() {
        let s = json_escape("say \"hi\" \\ \n\ttab");
        assert_eq!(s, "say \\\"hi\\\" \\\\ \\n\\ttab");
        assert!(!s.contains('\n'), "newline harus jadi \\n literal");
    }

    /// Sisip ke konten kosong & berisi — format lama utuh.
    #[test]
    fn bugdb_insert_into_empty_and_existing() {
        let empty = "{\"entries\":[]}";
        let e1 = bugdb_insert_entry(empty, "{\"kind\":\"Hang\"}");
        assert_eq!(e1, r#"{"entries":[{"kind":"Hang"}]}"#);
        let e2 = bugdb_insert_entry(&e1, "{\"kind\":\"Panic\"}");
        assert_eq!(
            e2, r#"{"entries":[{"kind":"Hang"},{"kind":"Panic"}]}"#,
            "entry kedua menempel sebelum ] penutup"
        );
    }

    /// Konten korup (bukan format lama) → struktur valid baru, bukan pecah.
    #[test]
    fn bugdb_insert_repairs_corrupt_content() {
        let broken = "not json at all ]}";
        let out = bugdb_insert_entry(broken, "{\"kind\":\"Ok\"}");
        assert_eq!(out, r#"{"entries":[{"kind":"Ok"}]}"#);
    }

    fn mk_case(detail: &str) -> CaseResult {
        CaseResult {
            target: Target::Simulator,
            category: Category::Differential,
            oracle: "O5-differential",
            detail: detail.to_string(),
            source: "module x; endmodule".to_string(),
            cov_keys: Vec::new(),
        }
    }

    /// REGRESI fuzz-dedup (temuan triage tahap 2): dua mismatch dengan baris
    /// pertama generik SAMA tapi sinyal beda wajib beda signature — dulu
    /// runtuh jadi satu grup, bug kedua terkubur tak tersimpan.
    #[test]
    fn signature_separates_same_header_different_signals() {
        let a =
            mk_case("SEMANTIC MISMATCH vs iverilog:\n  ref : ASRT_Q=<15>\n  mivon: ASRT_Q=<31>");
        let b =
            mk_case("SEMANTIC MISMATCH vs iverilog:\n  ref : ASRT_D=<17>\n  mivon: ASRT_D=<34>");
        assert_ne!(a.signature(), b.signature(), "sinyal beda = grup beda");
        let a2 =
            mk_case("SEMANTIC MISMATCH vs iverilog:\n  ref : ASRT_Q=<15>\n  mivon: ASRT_Q=<31>");
        assert_eq!(a.signature(), a2.signature(), "detail identik = grup sama");
    }

    /// Detail satu baris (kasus lama: hang/panic) tetap stabil — baris-2
    /// kosong tak menambah noise pemisah.
    #[test]
    fn signature_single_line_stable() {
        let a = mk_case("hang > 5000 ms (grace 2x habis; worker dilanjutkan di background)");
        let b = mk_case("hang > 5000 ms (grace 2x habis; worker dilanjutkan di background)");
        assert_eq!(a.signature(), b.signature());
        assert!(a.signature().ends_with('|'), "baris-2 kosong = sufiks '|'");
    }

    /// Plumbing target/kategori judge (tahap 10): token, label, is_bug.
    #[test]
    fn judge_target_and_category_plumbing() {
        assert_eq!(Target::from_token("judge"), Some(Target::Judge));
        assert_eq!(Target::from_token("lrm"), Some(Target::Judge));
        assert_eq!(Target::Judge.as_str(), "judge");
        assert!(
            Target::ALL.contains(&Target::Judge),
            "judge ikut kampanye all"
        );
        assert_eq!(
            Category::from_label("lrm_violation"),
            Some(Category::LrmViolation)
        );
        assert_eq!(Category::LrmViolation.label(), "lrm_violation");
        assert!(Category::LrmViolation.is_bug(), "violasi LRM = bug");
        assert_eq!(Oracle::O6LrmJudge.as_str(), "O6-lrm-judge");
    }

    /// Seed-coverage guarantee (tahap 3): tiap kandidat dijamah >=1x saat
    /// cases >= kandidat; selebihnya acak. RNG deterministik.
    #[test]
    fn seed_plan_covers_every_candidate_once() {
        let cand: Vec<usize> = (0..5).collect();
        let plan = build_seed_plan(&cand, 5, &mut Rng::new(1));
        assert_eq!(plan.len(), 5);
        // 5 kasus pertama = kandidat terurut (tiap seed >=1x).
        assert_eq!(plan[..5], [0, 1, 2, 3, 4]);
    }

    #[test]
    fn seed_plan_overcovers_with_random_tail() {
        let cand: Vec<usize> = (0..3).collect();
        let plan = build_seed_plan(&cand, 10, &mut Rng::new(7));
        assert_eq!(plan.len(), 10);
        // 3 pertama = cover penuh (terurut); 7 sisanya bebas (dari kandidat).
        assert_eq!(plan[..3], [0, 1, 2]);
        assert!(plan[3..].iter().all(|&i| (0..3).contains(&i)));
    }

    #[test]
    fn seed_plan_empty_when_no_candidates() {
        let plan: Vec<usize> = build_seed_plan(&[], 50, &mut Rng::new(1));
        assert!(plan.is_empty());
    }

    /// Anti-starvasi: saat kandidat > kasus, budget disebar ke SELURUH
    /// rentang (bukan hanya `cases` pertama). Tanpa ini seed di ujung
    /// daftar (proyek besar) tak pernah dijamah.
    #[test]
    fn seed_plan_spreads_when_candidates_exceed_cases() {
        let cand: Vec<usize> = (0..100).collect();
        let plan = build_seed_plan(&cand, 10, &mut Rng::new(1));
        assert_eq!(plan.len(), 10);
        // Semua indeks unik & valid.
        let mut uniq = plan.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 10, "tak boleh duplikat: {plan:?}");
        assert!(plan.iter().all(|&i| i < 100));
        // Harus menjangkau paruh atas daftar (bukan hanya 0..10).
        assert!(
            plan.iter().any(|&i| i >= 50),
            "harus menyentuh seed di ujung daftar: {plan:?}"
        );
    }

    /// Rencana sadar-ledger: seed BELUM dievaluasi didahulukan; sisa budget
    /// diisi seed yang sudah pernah — tidak ada seed baru ter-skip saat
    /// masih ada unseen.
    #[test]
    fn prioritized_plan_puts_unseen_first() {
        let unseen: Vec<usize> = (100..110).collect();
        let seen: Vec<usize> = (0..90).collect();
        let plan = build_prioritized_plan(&unseen, &seen, 12, &mut Rng::new(3));
        assert_eq!(plan.len(), 12);
        // 10 slot pertama = seluruh unseen (terurut), 2 sisa dari seen.
        assert_eq!(plan[..10], unseen[..]);
        assert!(
            plan[10..].iter().all(|i| seen.contains(i)),
            "sisa harus dari seen"
        );
    }

    /// Tanpa ledger (semua unseen) = perilaku lama: tiap kandidat dijamah.
    #[test]
    fn prioritized_plan_all_unseen_matches_plain_plan() {
        let unseen: Vec<usize> = (0..5).collect();
        let plan = build_prioritized_plan(&unseen, &[], 5, &mut Rng::new(9));
        assert_eq!(plan, vec![0, 1, 2, 3, 4]);
    }

    /// Kampanye pertama corpus < cases (seen kosong): budget TETAP penuh —
    /// sisa slot diisi ulang dari semua kandidat, bukan mengecil.
    #[test]
    fn prioritized_plan_keeps_budget_full_on_first_campaign() {
        let unseen: Vec<usize> = (0..3).collect();
        let plan = build_prioritized_plan(&unseen, &[], 10, &mut Rng::new(5));
        assert_eq!(plan.len(), 10, "budget kampanye tak boleh mengecil");
        assert_eq!(plan[..3], [0, 1, 2], "tiga unseen pertama dijamah");
        assert!(plan[3..].iter().all(|&i| i < 3));
    }

    /// Semua sudah terlihat → budget tetap penuh dari `seen`.
    #[test]
    fn prioritized_plan_falls_back_to_seen_when_no_unseen() {
        let seen: Vec<usize> = (0..50).collect();
        let plan = build_prioritized_plan(&[], &seen, 7, &mut Rng::new(11));
        assert_eq!(plan.len(), 7);
        assert!(plan.iter().all(|i| seen.contains(i)));
    }

    /// Kesetaraan serial vs worker-pool: preproc in-process deterministik →
    /// kategori per-slot IDENTIK jobs=1 vs jobs=3 (RNG stream per-slot).
    #[test]
    fn parallel_matches_serial_preproc() {
        let seed = 0xC0FFEE_BEEF;
        let mut serial = FuzzConfig::default();
        serial.target = Target::Preproc;
        serial.cases = 10;
        serial.jobs = 1;
        serial.seed = seed;
        serial.save_bugs = false;
        let rep_serial = run(serial.clone());

        let mut par = serial.clone();
        par.jobs = 3;
        let rep_par = run(par);

        assert_eq!(
            rep_serial.summaries[0].categories, rep_par.summaries[0].categories,
            "kategori serial vs parallel harus identik (determinisme per-slot)"
        );
        assert!(
            !rep_par.summaries[0].categories.is_empty(),
            "preproc 10 kasus harus menghasilkan kategori"
        );
    }

    /// case_rng: stream per-slot berbeda & deterministik lintas pemanggilan.
    #[test]
    fn case_rng_deterministic_per_slot() {
        let mut a1 = case_rng(0xC0FFEE, 7);
        let mut a2 = case_rng(0xC0FFEE, 7);
        assert_eq!(a1.next_u64(), a2.next_u64(), "slot sama = stream sama");
        let mut b = case_rng(0xC0FFEE, 8);
        assert_ne!(a1.next_u64(), b.next_u64(), "slot beda = stream beda");
    }

    /// case_jobs: 0 = auto (clamp 1-8), 1 = serial, besar = tetap N.
    #[test]
    fn case_jobs_resolution() {
        assert_eq!(case_jobs(1), 1);
        assert_eq!(case_jobs(99), 99);
        let auto = case_jobs(0);
        assert!((1..=8).contains(&auto), "auto = cores/2 clamp 1-8, dapat {auto}");
    }

    /// Nama artefak bug: stabil untuk signature sama, unik untuk beda.
    #[test]
    fn artifact_stem_stable_and_unique() {
        let a = artifact_stem(
            "sim|O5-differential|differential|first|second",
            "differential",
        );
        let b = artifact_stem(
            "sim|O5-differential|differential|first|second",
            "differential",
        );
        assert_eq!(a, b, "temuan identik = nama sama (idempoten)");
        let c = artifact_stem(
            "sim|O5-differential|differential|first|lain",
            "differential",
        );
        assert_ne!(a, c, "temuan beda = nama beda");
        assert!(a.starts_with("bug_") && a.ends_with("_differential"));
        assert_eq!(a.len(), 4 + 12 + 1 + "differential".len());
    }
}
