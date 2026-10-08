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
pub mod minimize;
pub mod mutator;
pub mod oracle;
pub mod oracle_icarus;
pub mod runner;
pub mod triage;
pub mod validate;

// ── LRM Judge system ────────────────────────────────────────────────────────
pub mod lrm_model;
pub mod lrm_rules;
pub mod lrm_judge;
pub mod observer;
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
}

impl Category {
    pub fn is_bug(self) -> bool {
        matches!(
            self,
            Category::Panic
                | Category::Abort
                | Category::Hang
                | Category::DiagMissing
                | Category::RoundtripMismatch
                | Category::NonDeterministic
                | Category::GuardBypass
                | Category::Differential
                | Category::HiddenBug
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
            Category::RoundtripMismatch => "roundtrip_mismatch",
            Category::NonDeterministic => "nondeterministic",
            Category::GuardBypass => "guard_bypass",
            Category::Differential => "differential",
            Category::Suspicious => "suspicious",
            Category::Slow => "slow",
            Category::HiddenBug => "hidden_bug",
            Category::Degraded => "degraded",
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
            "roundtrip_mismatch" => Some(Category::RoundtripMismatch),
            "nondeterministic" => Some(Category::NonDeterministic),
            "guard_bypass" => Some(Category::GuardBypass),
            "differential" => Some(Category::Differential),
            "suspicious" => Some(Category::Suspicious),
            "slow" => Some(Category::Slow),
            "hidden_bug" => Some(Category::HiddenBug),
            "degraded" => Some(Category::Degraded),
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
}

impl Oracle {
    pub fn as_str(self) -> &'static str {
        match self {
            Oracle::O1NoCrash => "O1-no-crash",
            Oracle::O2DiagLocation => "O2-diag-location",
            Oracle::O3Roundtrip => "O3-roundtrip",
            Oracle::O4Determinism => "O4-determinism",
            Oracle::O5Differential => "O5-differential",
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
        }
    }
}

/// Ringkasan per target.
#[derive(Debug, Default, Clone)]
pub struct TargetSummary {
    pub target: Target,
    pub total: usize,
    pub bugs: usize,
    /// Kasus di-skip karena source membengkak > SIZE_CAP pasca-mutasi
    /// (input raksasa 100MB+ = lambat di tool mana pun, bukan bug mivon).
    pub skipped_big: usize,
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

/// Run kampanye fuzz.
pub fn run(cfg: FuzzConfig) -> FuzzReport {
    match cfg.target {
        Target::All => {
            let mut merged = FuzzReport::default();
            for &target in Target::ALL {
                let mut single_cfg = cfg.clone();
                single_cfg.target = target;
                let report = run_single(single_cfg);
                merged.bugs.extend(report.bugs);
                merged.summaries.extend(report.summaries);
            }
            merged
        }
        _ => run_single(cfg),
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

fn run_single(cfg: FuzzConfig) -> FuzzReport {
    use std::collections::BTreeMap;

    let corpus = corpus::Corpus::load(cfg.corpus_dir.as_deref());
    if corpus.is_empty() {
        eprintln!(
            "WARNING: corpus kosong di {:?} — tidak ada seed nyata",
            cfg.corpus_dir.clone().unwrap_or_else(default_corpus_dir)
        );
    }

    let mut rng = Rng::new(cfg.seed);

    let mut results: Vec<CaseResult> = Vec::new();
    let mut dismissed_hangs: usize = 0;
    let mut categories: BTreeMap<Category, usize> = BTreeMap::new();
    let mut seen_sigs: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut skipped_big: usize = 0;

    for i in 0..cfg.cases {
        // Ambil seed nyata dari corpus (100% — tanpa template sintetis).
        // `.mv` return RAW source, transpile setelah mutasi.
        // Target MV HANYA pakai seed `.mv` (SV di-feed ke transpile = noise).
        let base_seed = if corpus.is_empty() {
            break;
        } else if cfg.target == Target::Mv {
            // Target MV HANYA pakai seed `.mv` — sample langsung dari subset
            // `.mv` (resample acak dari corpus penuh: 21/4664 seed → hampir
            // selalu break di case pertama, kampanye MV 0-2 case/2000).
            match corpus.random_mv_seed(&mut rng) {
                Some(s) => s,
                None => break,
            }
        } else {
            match corpus.random_seed(&mut rng) {
                Some(src) => src,
                None => break,
            }
        };
        let is_mv = base_seed.is_mv;

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
                combine_tb_with_module(&base_seed.path, &base_seed.text, &corpus)
            } else {
                base_seed.text
            }
        } else {
            // MV: simpan raw; transpile terjadi SETELAH mutasi
            base_seed.text
        };

        // Mutasi 0-4x (mutator dibuat per-iterasi agar borrow rng singkat)
        let n_mut = rng.below(5);
        {
            let mut mutator = mutator::Mutator::new(&mut rng);
            for _ in 0..n_mut {
                base_source = mutator.mutate(&base_source, &corpus);
            }
        }

        // Directed mutation: 30% kasus — serangan terarah SETELAH mutasi acak
        if rng.chance(30) {
            let mut dir = directed::DirectedMutator::new(&mut rng);
            base_source = dir.mutate(&base_source);
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

        // Size guard pasca-mutasi: source raksasa (duplicate_chunk blowup)
        // lambat di tool mana pun — skip, jangan evaluasi (bukan bug mivon).
        if source.len() > SIZE_CAP {
            skipped_big += 1;
            continue;
        }

        let mut result = oracle::evaluate(cfg.target, &source, cfg.timeout_ms);
        // O6 utk jalur IN-PROCESS (lexer/parser/elab/fmt/preproc/mv): tak ada
        // subprocess stdout utk di-scan — scan `detail` (pesan error/ok)
        // utk pola KUAT HiddenBug (mis. pesan ber-"internal error") — dulu
        // jalur ini lolos tanpa O6 sama sekali. Degradasi lemah tak discan
        // di sini (pesan CleanError korpus fragment → noise).
        if !result.category.is_bug() {
            if let Some((cat, detail)) = oracle::scan_hidden_diags(&result.detail) {
                if cat == Category::HiddenBug {
                    result.category = cat;
                    result.detail = detail;
                }
            }
        }
        *categories.entry(result.category).or_insert(0) += 1;

        if result.category.is_bug() {
            let sig = result.signature();
            if !seen_sigs.contains(&sig) {
                seen_sigs.insert(sig);
                // AUTO-DISMISS hang via replay tenang (review kampanye):
                // grace runner 3× (15s) belum cukup utk kasus besar + beban
                // mesin → "hang" berulang padahal replay saat sepi SELESAI
                // (semua hang kampanye = slow, replay-ok). Replay dgn budget
                // 6×: tak selesai → hang asli (simpan sbg bug); selesai dgn
                // kategori non-bug → BUKAN bug (jangan simpan, jangan hitung).
                if result.category == Category::Hang {
                    let calmer = oracle::evaluate(
                        result.target,
                        &result.source,
                        cfg.timeout_ms.saturating_mul(6),
                    );
                    if !calmer.category.is_bug() {
                        // Pindahkan hitungan ke kategori hasil replay
                        // (slow/ok/suspicious) — summary tetap konsisten.
                        if let Some(c) = categories.get_mut(&result.category) {
                            *c = c.saturating_sub(1);
                        }
                        *categories.entry(calmer.category).or_insert(0) += 1;
                        dismissed_hangs += 1;
                        continue;
                    }
                }
                if cfg.save_bugs {
                    save_crash(&result, i);
                }
                results.push(result);
            }
        }

        if (i + 1) % 500 == 0 {
            eprint!(
                "\r  [{}/{}] bugs: {} skip_big: {}",
                i + 1,
                cfg.cases,
                results.len(),
                skipped_big
            );
        }
    }
    if cfg.cases >= 500 {
        eprintln!();
    }

    let summary = TargetSummary {
        target: cfg.target,
        total: cfg.cases,
        bugs: results.len(),
        skipped_big,
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
    let filename = format!("bug_{:04}_{}.sv", seq, kind);
    let path = dir.join(&filename);
    let _ = std::fs::write(&path, &result.source);

    // Metadata .txt — `target:` ditambahkan utk replay/auto-verify lanjutan
    // (dulu tak ada → reclassify hang butuh tebak target dari signature).
    let meta_path = dir.join(format!("bug_{:04}_{}.txt", seq, kind));
    let meta = format!(
        "oracle: {}\ncategory: {}\ntarget: {}\nsignature: {}\ndetail: {}\niter: {}\n",
        result.oracle,
        result.category.label(),
        result.target.as_str(),
        result.signature(),
        result.detail.lines().next().unwrap_or(""),
        iter,
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

/// Append satu entry ke bug database (format lama dihormati; tulis atomik
/// via temp+rename ala MICD). Gagal = diam (bug utama tetap ke-save ke .sv).
fn append_bugdb(kind: &str, source: &str, detail: &str, iter: usize) {
    use std::io::Write as _;
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
        }
    }

    /// REGRESI fuzz-dedup (temuan triage tahap 2): dua mismatch dengan baris
    /// pertama generik SAMA tapi sinyal beda wajib beda signature — dulu
    /// runtuh jadi satu grup, bug kedua terkubur tak tersimpan.
    #[test]
    fn signature_separates_same_header_different_signals() {
        let a = mk_case("SEMANTIC MISMATCH vs iverilog:\n  ref : ASRT_Q=<15>\n  mivon: ASRT_Q=<31>");
        let b = mk_case("SEMANTIC MISMATCH vs iverilog:\n  ref : ASRT_D=<17>\n  mivon: ASRT_D=<34>");
        assert_ne!(a.signature(), b.signature(), "sinyal beda = grup beda");
        let a2 = mk_case("SEMANTIC MISMATCH vs iverilog:\n  ref : ASRT_Q=<15>\n  mivon: ASRT_Q=<31>");
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
}
