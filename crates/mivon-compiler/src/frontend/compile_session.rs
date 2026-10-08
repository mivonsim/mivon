//! CompileSession — orchestrates the parallel compilation pipeline.
//!
//! Pipeline: file discovery → parallel preprocessing → parallel lexing →
//! parallel parsing → merge designs → build module index.
//!
//! Now with CacheManager + IncrementalTracker integration for incremental builds.
use mivon_elaboration::elaborator::ElaborateMode;

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crate::cache::{compute_checksum, CacheManager, RemoteCacheBackend, RemoteSyncMode};
use crate::frontend::discovery::{DiscoveryOptions, FileDiscovery};
use crate::frontend::io::MmapFile;
use crate::frontend::lexer::FastLexer;
use crate::frontend::module_index::{EntryKind, ModuleIndex, ModuleMeta, ParamMeta};
use crate::micd::{self, MicdDatabase, MicdStats, PreprocEntry};
use crate::profiling::{Counter, Phase, Profiler};
use crate::scheduler::dag::DependencyGraph;
use crate::scheduler::incremental::IncrementalTracker;
use mivon_ast::Design;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_parser::lexer::{Lexer, Token};
use mivon_parser::preprocessor::Preprocessor;
use mivon_parser::Parser;
use std::sync::Arc;

/// Configuration untuk kompilasi.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub sources: Vec<PathBuf>,
    pub incdirs: Vec<PathBuf>,
    pub defines: Vec<(String, String)>,
    pub top_module: Option<String>,
    pub auto_incdirs: bool,
    pub libdirs: Vec<PathBuf>,
    pub libfiles: Vec<PathBuf>,
    /// Gunakan FastLexer byte-level (default: true)
    pub use_fast_lexer: bool,
    /// Gunakan lazy elaboration (HIR-based, on-demand)
    pub use_lazy_elab: bool,
    /// Sumber inline per path (F10): path yang ADA di peta ini TIDAK dibaca
    /// dari disk — kontennya diambil dari buffer (hasil transpile `.mv` F9).
    /// Dipakai `open_project`/`open_elaborated` (src/tools/mod.rs) agar semua
    /// tool (`msim`/`mcov`/`melab`/`mprof`/`mbench`/`mlint`/`mcheck`) bisa
    /// menerima file `.mv` tanpa menulis file ke disk.
    pub inline_sources: std::collections::HashMap<PathBuf, Vec<u8>>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            sources: Vec::new(),
            incdirs: Vec::new(),
            defines: Vec::new(),
            top_module: None,
            auto_incdirs: false,
            libdirs: Vec::new(),
            libfiles: Vec::new(),
            use_fast_lexer: true,
            use_lazy_elab: false,
            inline_sources: std::collections::HashMap::new(),
        }
    }
}

/// Compile session — orchestrates compilation pipeline with caching.
pub struct CompileSession {
    /// Session configuration (public for CLI integration)
    pub config: SessionConfig,
    pub module_index: ModuleIndex,
    pub timing: SessionTiming,
    /// Content-based cache for AST/HIR/includes
    pub cache: CacheManager,
    /// Incremental change tracker
    pub incremental: IncrementalTracker,
    /// Dependency graph for scheduling
    pub dep_graph: DependencyGraph,
    /// Profiler for performance measurement
    pub profiler: Option<Profiler>,
    /// Lazy elaborator (on-demand HIR elaboration)
    pub lazy_elab: crate::hir::LazyElaborator,
    /// Cached designs from previous compile (incremental)
    prev_designs: HashMap<PathBuf, Design>,
    /// Cached combined preprocessed source from previous compile (for source snippets in incremental)
    prev_combined_sources: HashMap<PathBuf, String>,
    /// Cached checksums from previous compile
    prev_checksums: HashMap<PathBuf, u64>,
    /// Merged design from last compile (for lazy elaboration)
    merged_design: Option<Design>,
    /// Merged preprocessed source from last compile (for source snippets)
    pub merged_source: Option<String>,
    /// Combined preprocessed source strings collected during parallel pass
    combined_parts: std::sync::Mutex<Vec<(usize, String)>>,
    /// Cached elaborated IR design (lifetime: until next compile)
    cached_ir_design: Option<mivon_ir::IrDesign>,
    /// Session-level incremental elaboration cache: signature → IrModule
    cached_elab_modules: HashMap<u64, mivon_ir::IrModule>,
    /// MICD — persistent incremental compilation database (load/save otomatis).
    pub micd: Option<MicdDatabase>,
    /// Jumlah AST yang di-restore dari MICD pada sesi ini.
    micd_restored: usize,
    /// Path yang AST-nya di-restore dari MICD (skip write-back pada save).
    micd_restored_paths: HashSet<PathBuf>,
    /// Include deps per file (dari preprocessor) untuk verifikasi header.
    micd_include_deps: HashMap<PathBuf, Vec<PathBuf>>,
    /// Payload lexer per file yang di-lex sesi ini: summary + token stream
    /// (db.md "2. lexer/") untuk cache lexer/.
    lexer_payloads: std::sync::Mutex<Vec<(PathBuf, crate::micd::cache::pipeline::LexerPayload)>>,
    /// Parse errors collected during compilation
    pub parse_errors: Vec<mivon_core::diagnostics::Diagnostic>,
    /// Diagnostik parse PER FILE (path → diagnostics) dari compile terakhir.
    /// Menyimpan hasil file cache-hit juga — dipakai (a) mengisi ulang
    /// `parse_errors` saat `compile()` dipanggil ulang dalam sesi sama, dan
    /// (b) disimpan ke MICD `diag.mdb` saat save, di mana entry ini sekaligus
    /// menjadi penanda agar file ber-diagnostik tidak di-restore (BUG #2:
    /// AST cache tidak membawa error-nya → run hangat melaporkan 0 error
    /// dan desain yang gagal parse lolos ke simulasi).
    file_parse_diags: HashMap<PathBuf, Vec<mivon_core::diagnostics::Diagnostic>>,
    /// Diagnostics dari ELABORASI terakhir (compile_and_elaborate[_with_mode]).
    /// Dulu elaborator lokal → flush_diagnostics tak pernah dipanggil →
    /// warning elab (mis. covergroup non-top 'tidak dielaborate', WR0102
    /// port width) TERBUANG di jalur tool (mcov dsb) padahal `mivon <file>`
    /// menampilkannya — warning hilang senyap dari laporan tool.
    pub elab_diagnostics: Vec<mivon_core::diagnostics::Diagnostic>,
    /// Callback progres per-file (path, cached) — dipanggil dari rayon par_iter
    /// saat tiap file selesai di-lex/parse. Dipakai animasi status line CLI.
    file_progress: Option<std::sync::Arc<dyn Fn(&PathBuf, bool) + Send + Sync>>,
}

#[derive(Debug, Default, Clone)]
pub struct SessionTiming {
    pub discovery_us: u64,
    pub preprocess_us: u64,
    pub lex_us: u64,
    pub parse_us: u64,
    pub index_us: u64,
    /// Waktu elaborasi (AST → IR) — diukur di `compile_and_elaborate`.
    pub elab_us: u64,
    pub total_us: u64,
    /// Files that were cached (not re-processed)
    pub cached_files: usize,
    /// Files that were actually processed
    pub processed_files: usize,
}
/// F10: sumber konten file — buffer inline (transpile `.mv`) atau mmap disk.
/// Enum ini menjaga jalur mmap tetap zero-copy (tanpa `to_vec()`) agar design
/// besar (mis. opentitan) tidak memboros memori saat fase preprocess paralel
/// (setiap file di-copy penuh = spike memori ~ukuran design).
enum SourceBytes<'a> {
    Inline(&'a [u8]),
    Disk(MmapFile),
}

impl<'a> SourceBytes<'a> {
    fn as_bytes(&self) -> &[u8] {
        match self {
            SourceBytes::Inline(b) => b,
            SourceBytes::Disk(m) => m.as_bytes(),
        }
    }

    fn checksum(&self) -> u64 {
        match self {
            SourceBytes::Inline(b) => compute_checksum(b),
            SourceBytes::Disk(m) => m.checksum,
        }
    }
}

/// Merge `other` into `target` by MOVING elements (O(1) per field, no cloning).
/// After calling, `other`'s Vec fields are empty (elements moved to `target`).
/// Eksponen basis-10 satuan `timescale` (`1ns` → -9, `1us` → -6).
///
/// Duplikat ringan dari `mivon_elaboration::util::generate::time_unit_exponent`
/// (crate `pub(crate)`). Tanpa shared helper karena elaborator tidak bergantung
/// ke `mivon-compiler`; ketiga salinan diuji tak langsung lewat
/// `timescale_tests` + `timescale_session_tests` (delay yang benar hanya
/// mungkin bila ketiganya menghasilkan nilai sama).
fn timescale_unit_exponent(unit: &str) -> Option<i32> {
    let u = unit.trim();
    let digits: String = u.chars().take_while(|c| c.is_ascii_digit()).collect();
    let mag: i32 = if digits.is_empty() {
        1
    } else {
        digits.parse().ok()?
    };
    if mag <= 0 {
        return None;
    }
    let mut exp: i32 = match u[digits.len()..].trim() {
        "s" => 0,
        "ms" => -3,
        "us" => -6,
        "ns" => -9,
        "ps" => -12,
        "fs" => -15,
        _ => return None,
    };
    let mut m = mag;
    while m > 1 {
        if m % 10 == 0 {
            m /= 10;
            exp += 1;
        } else if m % 2 == 0 {
            m /= 2;
            exp -= 1;
        } else {
            return None;
        }
    }
    Some(exp)
}

fn extend_design_move(target: &mut Design, other: &mut Design) {
    // F84: `timescale global butuh SATU nilai untuk semua file. Ambil yang
    // paling FINEST (eksponen terkecil) supaya delay tiap module bisa diskalakan
    // ke basis terhalus — ambil yang terakhir (urutan file) justru membuat
    // module `1ns` ikut skala `1us` dan seluruh delay design meleset 1000×.
    if let Some(ts) = other.timescale.as_ref() {
        let pick = match target.timescale.as_ref() {
            None => true,
            Some(cur) => {
                let e_new = timescale_unit_exponent(&ts.0);
                let e_cur = timescale_unit_exponent(&cur.0);
                match (e_new, e_cur) {
                    (Some(n), Some(c)) => n < c,
                    (Some(_), None) => true,
                    _ => false,
                }
            }
        };
        if pick {
            target.timescale = Some(ts.clone());
        }
    }
    target.modules.append(&mut other.modules);
    target.packages.append(&mut other.packages);
    target.interfaces.append(&mut other.interfaces);
    target.classes.append(&mut other.classes);
    target.binds.append(&mut other.binds);
    target.clocking_blocks.append(&mut other.clocking_blocks);
    target.configs.append(&mut other.configs);
    target.udp_defs.append(&mut other.udp_defs);
    target.unit_imports.append(&mut other.unit_imports);
    target.unit_funcs.append(&mut other.unit_funcs);
    target.unit_tasks.append(&mut other.unit_tasks);
    target.unit_typedefs.append(&mut other.unit_typedefs);
    target.unit_params.append(&mut other.unit_params);
    target.unit_decls.append(&mut other.unit_decls);
}

/// Kunci dedup diagnostik LINTAS file untuk agregat `parse_errors`.
/// Lokasi display (snippet `file:line:col`, fallback span) menunjuk sumber
/// ASLI konstruk — file yang sama bisa diparse dua kali dalam satu build
/// (mandiri di filelist + di-`include` oleh file lain) dan kedua parse
/// melapor di lokasi identik. Tanpa lokasi, fallback ke path parse-target
/// agar dua file berbeda tidak saling menelan diagnostiknya.
fn diag_dedup_key(
    d: &mivon_core::diagnostics::Diagnostic,
    parse_target: &std::path::Path,
) -> (
    mivon_core::diagnostics::diagnostic::DiagLevel,
    &'static str,
    String,
    String,
    usize,
    usize,
) {
    let (file, line, col) = if let Some(ss) = &d.source_snippet {
        (ss.file.clone(), ss.line, ss.col)
    } else if let Some(sp) = d.spans.first() {
        (
            sp.file.as_str().to_string(),
            sp.start as usize,
            sp.end as usize,
        )
    } else {
        (parse_target.to_string_lossy().into_owned(), 0usize, 0usize)
    };
    (
        d.level,
        d.code.as_str(),
        d.message.to_string(),
        file,
        line,
        col,
    )
}

/// Konversi `Diagnostic` → `micd::DiagEntry` untuk penyimpanan MICD
/// `diag.mdb` (dipakai LSP/IDE dan sebagai penanda "file ini punya
/// diagnostik" di `attach_micd` — BUG #2). Lokasi diambil dari
/// `source_snippet` (file:line:col saat render), fallback `spans[0]`.
fn diag_to_entry(d: &mivon_core::diagnostics::Diagnostic) -> micd::DiagEntry {
    use mivon_core::diagnostics::diagnostic::DiagLevel;
    let severity = match d.level {
        DiagLevel::Warning => micd::DiagSeverity::Warning,
        DiagLevel::Note => micd::DiagSeverity::Note,
        DiagLevel::Help => micd::DiagSeverity::Hint,
        DiagLevel::Info | DiagLevel::Trace | DiagLevel::Debug => micd::DiagSeverity::Info,
        DiagLevel::Fatal | DiagLevel::Bug | DiagLevel::Error => micd::DiagSeverity::Error,
    };
    let (line, col) = if let Some(ss) = &d.source_snippet {
        (ss.line, ss.col)
    } else if let Some(sp) = d.spans.first() {
        (sp.start as usize, sp.end as usize)
    } else {
        (0usize, 0usize)
    };
    let mut e = micd::DiagEntry::new(
        line,
        col,
        severity,
        d.message.to_string(),
        d.code.as_str().to_string(),
    );
    if line != 0 || col != 0 {
        e.span = Some(micd::diag::Span::point(line, col));
    }
    e
}

impl CompileSession {
    pub fn new(config: SessionConfig) -> Self {
        CompileSession {
            config,
            module_index: ModuleIndex::new(),
            timing: SessionTiming::default(),
            cache: CacheManager::new(),
            incremental: IncrementalTracker::new(),
            dep_graph: DependencyGraph::new(),
            profiler: None,
            lazy_elab: crate::hir::LazyElaborator::new(),
            prev_designs: HashMap::new(),
            prev_combined_sources: HashMap::new(),
            prev_checksums: HashMap::new(),
            merged_design: None,
            merged_source: None,
            combined_parts: std::sync::Mutex::new(Vec::new()),
            cached_ir_design: None,
            cached_elab_modules: HashMap::new(),
            micd: None,
            micd_restored: 0,
            micd_restored_paths: HashSet::new(),
            micd_include_deps: HashMap::new(),
            lexer_payloads: std::sync::Mutex::new(Vec::new()),
            parse_errors: Vec::new(),
            file_parse_diags: HashMap::new(),
            elab_diagnostics: Vec::new(),
            file_progress: None,
        }
    }

    /// Pasang callback progres per-file `(path, cached)` — dipanggil dari
    /// worker rayon setiap file selesai di-lex/parse. Dipakai animasi CLI.
    pub fn set_file_progress<F>(&mut self, f: F)
    where
        F: Fn(&PathBuf, bool) + Send + Sync + 'static,
    {
        self.file_progress = Some(std::sync::Arc::new(f));
    }

    /// Run the full compilation pipeline (with caching).
    /// If self.config.incremental is set, skips files whose checksums haven't changed.
    pub fn compile(&mut self) -> Result<(Design, &ModuleIndex), SimError> {
        let total_start = Instant::now();
        let base_pp = self.create_preprocessor();

        // ── Phase 1: File Discovery ──
        let mut files: Vec<PathBuf> = self.discover_files()?;
        // ── Phase 1b: Auto-include package file yang hilang dari daftar ──
        // OpenTitan reggen: `X_regs_reg_top.sv` meng-import `X_reg_pkg::*` yang
        // dibangkitkan di direktori yang SAMA (`rtl/X_reg_pkg.sv`). Filelist
        // yang lupa menyertakan package-nya membuat import tak ter-resolve →
        // E2001 `CHERIOT_ALERT_TEST_OFFSET not found` (33 *_reg_pkg RTL absen
        // di opentitan_rtl.f). Bila `import <pkg>::` dipakai dan file
        // `<pkg>.sv/.svh` ADA di direktori pengimport namun belum masuk daftar
        // → tambahkan otomatis (pola deterministic: nama file == nama paket).
        self.auto_include_missing_packages(&mut files);
        // File yang terdaftar lebih dari sekali (filelist duplikat) → AST &
        // diagnostik tercatat ganda — 1 masalah nyata terlipat N× tergantung
        // jumlah pendaftaran. Buang kembaran, pertahankan urutan pertama
        // (urutan = semantik override definisi).
        {
            let mut seen = HashSet::with_capacity(files.len());
            files.retain(|f| seen.insert(f.clone()));
        }
        if files.is_empty() {
            return Err(SimError::with_diag(
                DiagCode::ModuleNotFound,
                "no source files found",
            ));
        }

        // ── Phase 2: Detect changed files ──
        let changed_set: HashSet<PathBuf> = self.detect_changed(&files);
        let incremental = !changed_set.is_empty() || !self.prev_designs.is_empty();

        // ── Phase 3: Parallel preprocessing (skip unchanged files if incremental) ──
        let pp_start = Instant::now();
        // Counters for parallel section (extracted before closure to avoid borrow issues)
        let tokens_lexed = std::sync::atomic::AtomicU64::new(0);
        let cache = &self.cache;
        let use_fast_lexer = self.config.use_fast_lexer;
        let prev_designs = &self.prev_designs;
        let prev_combined = &self.prev_combined_sources;
        let prev_checksums = &self.prev_checksums;
        // F10: sumber inline (buffer transpile `.mv`) — path di peta ini tidak
        // dibaca dari disk. Disalin sebelum closure agar tidak borrow self.
        let inline_sources = &self.config.inline_sources;

        // Shared collection for combined source strings (indexed by file position)
        let combined_parts = &self.combined_parts;
        // Include deps per file (untuk verifikasi header MICD)
        let include_deps = std::sync::Mutex::new(HashMap::<PathBuf, Vec<PathBuf>>::new());
        // Segmen `timescale per file (F84, LRM §19.8): koordinat baris output
        // preprocessed per-file. Dipakai phase 5 untuk mengisi
        // `parser.timescale_segments` (dengan geseran +1 baris marker `line`
        // + base offset global). Tanpa ini parser CompileSession tak pernah
        // tahu satuan per-module → semua delay diskala global.
        let ts_segments =
            std::sync::Mutex::new(Vec::<(usize, Vec<(usize, (String, String))>)>::new());
        // Clear any previous parts
        {
            let mut parts = combined_parts.lock().unwrap();
            parts.clear();
        }

        // Phase 3a: preprocess all files in parallel, collect combined sources.
        // Cached files reuse the previously parsed design (positions already global).
        let prepared: Vec<Result<(PathBuf, Option<Design>, u64, Option<String>), SimError>> = files
            .par_iter()
            .enumerate()
            .map(|(file_idx, path)| {
                // ── Fast path: file unchanged, use cached design ──
                if incremental && !changed_set.contains(path) {
                    if let Some(cached) = prev_designs.get(path) {
                        let cksum = prev_checksums.get(path).copied().unwrap_or(0);
                        // Push cached combined source for this file
                        if let Some(cached_combined) = prev_combined.get(path) {
                            let mut parts = combined_parts.lock().unwrap();
                            parts.push((file_idx, cached_combined.clone()));
                        }
                        return Ok((path.clone(), Some(cached.clone()), cksum, None));
                    }
                }

                // ── Slow path: process file ──
                // F10: path yang ada di inline_sources memakai buffer (hasil
                // transpile `.mv`) — checksum dihitung dari buffer, bukan disk.
                // File normal tetap zero-copy via MmapFile (tanpa to_vec).
                let holder: SourceBytes = if let Some(bytes) = inline_sources.get(path) {
                    SourceBytes::Inline(bytes.as_slice())
                } else {
                    SourceBytes::Disk(MmapFile::open(path).map_err(|e| {
                        SimError::Io(e.kind(), format!("{}: {}", path.to_string_lossy(), e))
                    })?)
                };
                let cksum = holder.checksum();
                // Use mmap data directly without extra .to_string() copy
                cache.register_file(path, holder.as_bytes());

                let mut pp = base_pp.clone();
                let path_str = path.to_string_lossy();
                // Label `` `line `` RESTORE utk include (path file induk).
                pp.cur_path = Some(path_str.to_string());
                // Gunakan Cow::from_utf8_lossy — untuk data valid UTF-8
                // (99%+ file SV), tidak ada alokasi baru: hanya borrow mmap bytes.
                // Sebelumnya .into_owned() SELALU mengalokasi String baru per file.
                let src_cow = String::from_utf8_lossy(holder.as_bytes());
                // current_dir = parent file (sama dgn legacy preprocess_file):
                // include yang ada di direktori file sendiri diutamakan.
                let src_dir = path.parent().map(|p| p.to_path_buf());
                let preprocessed = pp.preprocess(&src_cow, src_dir.as_ref()).map_err(|e| {
                    SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("preprocessor {}: {}", path_str, e),
                    )
                })?;
                // Jalur inline tidak menambah include deps (buffer sudah
                // menggabungkan definisi bersama; `include di-strip).
                if !pp.resolved_includes.is_empty() {
                    let mut inc = include_deps.lock().unwrap();
                    inc.insert(path.clone(), pp.resolved_includes.iter().cloned().collect());
                }

                let combined = format!("`line 1 \"{}\"\n{}\n", path_str, preprocessed);
                // Store combined source for later use (source snippets)
                {
                    let mut parts = combined_parts.lock().unwrap();
                    parts.push((file_idx, combined.clone()));
                }
                // F84: simpan segmen `timescale file ini untuk phase 5.
                // File cache-hit tak perlu (design di-restore, tak di-parse).
                if !pp.timescale_segments.is_empty() {
                    let mut segs = ts_segments.lock().unwrap();
                    segs.push((file_idx, pp.timescale_segments.clone()));
                }
                Ok((path.clone(), None, cksum, Some(combined)))
            })
            .collect();

        // ── Phase 4: Compute per-file base line offsets ──
        // Posisi AST bersifat per-file (relatif ke combined source masing-masing),
        // sedangkan elaborator mengindeks source_lines GABUNGAN semua file.
        // Tambahkan offset kumulatif agar snippet source menunjuk file/baris yang benar.
        let mut base_offsets: Vec<usize> = vec![0; files.len()];
        {
            let mut parts = combined_parts.lock().unwrap();
            parts.sort_by_key(|(idx, _)| *idx);
            let mut cumulative: usize = 0;
            for (idx, s) in parts.iter() {
                base_offsets[*idx] = cumulative;
                cumulative += s.lines().count();
            }
            // TEMP DEBUG dump combined output
            if std::env::var("MIVON_DUMP_COMBINED").is_ok() {
                let mut out = String::new();
                for (_idx, s) in parts.iter() {
                    out.push_str(s);
                }
                std::fs::write("/tmp/opencode/combined_dump.txt", out).ok();
            }
        }

        // F84: petakan segmen `timescale per-file ke koordinat GLOBAL
        // (sama seperti token: combined-coord + base). Combined = 1 baris
        // marker `` `line `` + output preprocessed, jadi segmen (koordinat
        // output preprocessed) digeser +1 lalu + base file ini.
        let ts_global: HashMap<usize, Vec<(usize, (String, String))>> = {
            let segs = ts_segments.lock().unwrap();
            let mut map: HashMap<usize, Vec<(usize, (String, String))>> = HashMap::new();
            for (idx, v) in segs.iter() {
                let base = base_offsets.get(*idx).copied().unwrap_or(0);
                map.insert(
                    *idx,
                    v.iter().map(|(from, ts)| (from + 1 + base, ts.clone())).collect(),
                );
            }
            map
        };

        self.timing.preprocess_us = pp_start.elapsed().as_micros() as u64;

        // ── Phase 4b: Discovery nama class & typedef GLOBAL ──
        // Parsing per-file hanya mengenal nama di file-nya sendiri; `ClassType var;`
        // dari file lain akan salah di-parse sebagai instance module. Scan semua
        // combined source (cached + fresh) untuk mengumpulkan nama class/typedef,
        // lalu seed ke Parser yang memproses file fresh. Skip bila semua file
        // di-restore dari cache (tidak ada parse → nama tidak dipakai).
        let has_fresh = prepared.iter().any(|r| matches!(r, Ok((_, None, ..))));
        let (global_classes, global_typedefs) = if has_fresh {
            let parts = combined_parts.lock().unwrap();
            // Parallel discovery: split work across rayon thread pool
            use rayon::prelude::*;
            let results: Vec<(HashSet<Symbol>, HashSet<Symbol>)> = parts
                .par_iter()
                .map(|(_, src)| {
                    let mut classes = HashSet::new();
                    let mut typedefs = HashSet::new();
                    discover_names_in_source(src, &mut classes, &mut typedefs);
                    (classes, typedefs)
                })
                .collect();
            let mut classes = HashSet::new();
            let mut typedefs = HashSet::new();
            for (c, t) in results {
                classes.extend(c);
                typedefs.extend(t);
            }
            (classes, typedefs)
        } else {
            (HashSet::new(), HashSet::new())
        };
        if std::env::var("MIVON_DEBUG_PARSE").is_ok() && has_fresh {
            eprintln!(
                "[DBG-PARSE] global discovery: classes={} typedefs={}",
                global_classes.len(),
                global_typedefs.len()
            );
        }

        // Discovery timing (already started at pp_start, now record separately)
        // Note: discovery_ms is measured as part of preprocess_ms breakdown.

        // ── Phase 5: Parallel lexing + parsing dengan posisi global ──
        let lex_start = Instant::now();
        let lexer_payloads = &self.lexer_payloads;
        // Clone callback progres per-file sebelum closure par_iter (tanpa
        // mem-borrow self di dalam rayon).
        let file_progress = self.file_progress.clone();
        // Diagnostik per file dari compile sebelumnya (sesi sama) — file
        // cache-hit tetap menyumbang error-nya, jangan dibuang (BUG #2).
        let file_diags = &self.file_parse_diags;
        let results: Vec<
            Result<
                (
                    PathBuf,
                    Design,
                    u64,
                    Vec<mivon_core::diagnostics::Diagnostic>,
                ),
                SimError,
            >,
        > = prepared
            .into_par_iter()
            .enumerate()
            .map(|(file_idx, r)| {
                let (path, cached, cksum, combined_opt) = r?;
                // Reuse cached design as-is (sudah diparse dengan posisi global)
                if let Some(design) = cached {
                    if let Some(cb) = file_progress.as_ref() {
                        cb(&path, true);
                    }
                    // BUG #2 (CRITICAL): AST cache-hit TIDAK boleh membuang
                    // diagnostik parse file ini. Dulu `Vec::new()` → jumlah
                    // error jadi non-deterministik tergantung cache, dan
                    // desain yang tidak parse bersih bisa lolos ke simulasi
                    // pada run hangat. Kembalikan diagnostik sesi sebelumnya
                    // (konten file tidak berubah → diagnostik identik).
                    let errs = file_diags.get(&path).cloned().unwrap_or_default();
                    return Ok((path, design, cksum, errs));
                }
                let combined = combined_opt.unwrap_or_default();
                let base = base_offsets[file_idx];
                let path_str = path.to_string_lossy();
                // file_line_map: tracks `line directive mappings for resolve_source_file.
                // Positions are CUMULATIVE in the per-file combined (same as token lines).
                // Adjusted by base offset after lexing, same as tokens.
                let file_line_map: Vec<(usize, usize, String)>;
                let tokens = if use_fast_lexer {
                    // FastLexer keeps cumulative line numbers; content after the
                    // initial `line 1 "file" directive starts at internal line 2,
                    // so global position = line + base (mirrors merged source).
                    let mut lexer = FastLexer::new(&combined, &path_str);
                    let mut toks = Vec::new();
                    loop {
                        let (tok, line, col) = lexer.next_token();
                        if tok == Token::Eof {
                            break;
                        }
                        toks.push((tok, line + base, col));
                    }
                    // Adjust file_line_map positions by the same base offset as tokens
                    file_line_map = lexer
                        .file_line_map
                        .into_iter()
                        .map(|(pos, val, file)| (pos + base, val, file))
                        .collect();
                    tokens_lexed.fetch_add(toks.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    toks
                } else {
                    // Legacy Lexer mempertahankan nomor baris kumulatif (baris
                    // directive ikut dihitung), sehingga posisi global = base + line.
                    let mut lexer = Lexer::new(&combined);
                    let mut toks = Vec::new();
                    loop {
                        let (tok, line, col) = lexer.next_token();
                        if tok == Token::Eof {
                            break;
                        }
                        toks.push((tok, line + base, col));
                    }
                    // Adjust file_line_map positions by the same base offset as tokens
                    file_line_map = lexer
                        .file_line_map
                        .into_iter()
                        .map(|(pos, val, file)| (pos + base, val, file))
                        .collect();
                    tokens_lexed.fetch_add(toks.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    toks
                };

                // Cache lexer/ (db.md "2. lexer/"): simpan summary + token
                // stream asli (TokenID/Kind + Location) agar tool dapat
                // membaca token tanpa menjalankan lexer ulang.
                let mut summary = crate::micd::cache::pipeline::LexerSummary {
                    token_count: 0,
                    identifiers: 0,
                    numbers: 0,
                    strings: 0,
                    errors: 0,
                    source_bytes: combined.len() as u64,
                };
                let mut records = Vec::with_capacity(tokens.len());
                for (tok, line, col) in &tokens {
                    summary.observe(tok);
                    records.push(crate::micd::cache::pipeline::TokenRecord {
                        kind: crate::micd::cache::pipeline::token_family(tok),
                        line: *line as u32,
                        col: *col as u32,
                    });
                }
                lexer_payloads.lock().unwrap().push((
                    path.clone(),
                    crate::micd::cache::pipeline::LexerPayload {
                        summary,
                        tokens: records,
                    },
                ));

                let mut parser = Parser::new(tokens, &path_str)
                    .with_global_type_names(&global_classes, &global_typedefs)
                    .with_source_lines(&combined)
                    .with_file_line_map(file_line_map)
                    .with_line_base(base); // token global = line + base (cumulative)
                // F84: segmen `timescale file ini (koordinat global, sejajar
                // token) → timescale per-module. Absen = file tanpa directive.
                if let Some(segs) = ts_global.get(&file_idx) {
                    parser.timescale_segments = segs.clone();
                }
                let design = parser.parse_design()?;
                let parse_errors = parser.errors;
                if std::env::var("MIVON_DEBUG_PARSE").is_ok() && !parse_errors.is_empty() {
                    eprintln!("[DBG-PARSE] {} errors={}", path_str, parse_errors.len());
                    for e in &parse_errors {
                        eprintln!("  [DBG-PARSE] {:?}", e.message);
                    }
                    eprintln!(
                        "[DBG-PARSE] n_packages={} n_modules={}",
                        design.packages.len(),
                        design.modules.len()
                    );
                }

                if let Some(cb) = file_progress.as_ref() {
                    cb(&path, false);
                }
                Ok((path, design, cksum, parse_errors))
            })
            .collect();

        let lex_parse_us = lex_start.elapsed().as_micros() as u64;
        // Split lex/parse: approximate 60/40 split (lex ~60%, parse ~40% of combined)
        // based on measured ratios from opentitan benchmark.
        self.timing.lex_us = (lex_parse_us * 60) / 100;
        self.timing.parse_us = lex_parse_us.saturating_sub(self.timing.lex_us);

        // Simpan include deps dari sesi ini.
        self.micd_include_deps = include_deps.into_inner().unwrap();

        // Count tokens
        if let Some(ref profiler) = self.profiler {
            profiler.count(
                Counter::TokensLexed,
                tokens_lexed.load(std::sync::atomic::Ordering::Relaxed),
            );
        }

        // Track cached vs processed files
        self.timing.cached_files = 0;
        self.timing.processed_files = 0;

        let mut file_designs: Vec<(PathBuf, Design)> = Vec::new();
        let mut file_checksums: HashMap<PathBuf, u64> = HashMap::new();
        let mut all_parse_errors: Vec<mivon_core::diagnostics::Diagnostic> = Vec::new();
        // Snapshot diagnostik per file dari compile INI. Entri lama dibuang
        // dulu supaya file yang kini parse bersih tidak membawa sisa error
        // dari compile sebelumnya (dan file yang berubah tidak ganda).
        self.file_parse_diags.clear();
        // Dedup LINTAS file (BUG #1b): sumber yang sama bisa diparse dua kali
        // dalam satu build — file S terdaftar mandiri di filelist DAN
        // di-`include` oleh file P (pola OpenTitan `*_pkg.sv` `include
        // "*_cov.sv"`). Kedua parse melapor di lokasi display yang SAMA (line
        // directive `include`) → pesan + file:line:col identik untuk konstruk
        // yang sama. `file_parse_diags` TETAP menyimpan salinan per file
        // (kebenaran per-file untuk LSP/MICD); hanya agregat yang di-dedup.
        let mut seen_diags = HashSet::new();
        for r in results {
            let (path, design, cksum, parse_errors) = r?;
            if !parse_errors.is_empty() {
                self.file_parse_diags
                    .insert(path.clone(), parse_errors.clone());
            }
            for d in parse_errors {
                if seen_diags.insert(diag_dedup_key(&d, &path)) {
                    all_parse_errors.push(d);
                }
            }
            file_designs.push((path.clone(), design));
            file_checksums.insert(path.clone(), cksum);
            // File yang AST-nya di-restore dari MICD (parse di-skip) TIDAK
            // dihitung sebagai processed — sebelumnya semua file masuk
            // processed sehingga `cached=0 processed=861` selalu tampil
            // walaupun 861 design sukses di-restore (bug pelaporan).
            if !self.micd_restored_paths.contains(&path) {
                self.timing.processed_files += 1;
            }
        }
        self.timing.cached_files = files.len().saturating_sub(self.timing.processed_files);

        // Store parse errors for later emission
        self.parse_errors = all_parse_errors;

        // ── Phase 6: Build Index + Merge ──
        let index_start = Instant::now();
        if file_designs.is_empty() {
            return Err(SimError::with_diag(
                DiagCode::ModuleNotFound,
                "no parsed files",
            ));
        }

        // Separate file paths and designs
        let paths: Vec<PathBuf> = file_designs.iter().map(|(p, _)| p.clone()).collect();
        let mut designs: Vec<Design> = file_designs.into_iter().map(|(_, d)| d).collect();

        // Build module index + dependency graph + incremental tracking (with profiling)
        let index_timer_start = self.profiler.as_ref().map(|_| Instant::now());
        self.build_index_and_deps(&paths, &designs, &file_checksums)?;
        if let Some(p) = self.profiler.as_ref() {
            if let Some(start) = index_timer_start {
                p.record_phase(Phase::Elaborate, start.elapsed().as_nanos() as u64);
            }
        }

        // Clone all designs for cache BEFORE merge (one-time O(n) cost).
        // File yang di-restore dari MICD sudah ada di prev_designs (dari
        // attach) → tidak perlu clone ulang (hemat CPU + memori).
        let mut cache_designs: Vec<Option<Design>> = designs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                if self.micd_restored_paths.contains(&paths[i]) {
                    None
                } else {
                    Some(d.clone())
                }
            })
            .collect();

        // Merge by moving (O(n), no cloning per iteration — eliminates O(n²) scaling)
        let mut merged: Design = std::mem::take(&mut designs[0]);
        for d in &mut designs[1..] {
            extend_design_move(&mut merged, d);
        }

        // If lazy elaboration is enabled, store design for on-demand elaboration
        // and pre-register all module names/ports in the LazyElaborator
        if self.config.use_lazy_elab {
            self.merged_design = Some(merged.clone());
            for module in &merged.modules {
                let signals: Vec<crate::hir::HirSignal> = module
                    .ports
                    .iter()
                    .map(|p| {
                        let width = p
                            .range
                            .as_ref()
                            .map(|r| {
                                let lo = r.lsb;
                                let hi = r.msb;
                                hi.abs_diff(lo) + 1
                            })
                            .unwrap_or(1);
                        crate::hir::HirSignal {
                            name: p.name,
                            dtype: crate::hir::HirType::BitVec { width },
                            width,
                            is_input: matches!(
                                p.direction,
                                mivon_ast::types::PortDirection::Input
                                    | mivon_ast::types::PortDirection::Inout
                            ),
                            is_output: matches!(
                                p.direction,
                                mivon_ast::types::PortDirection::Output
                                    | mivon_ast::types::PortDirection::Inout
                            ),
                        }
                    })
                    .collect();
                self.lazy_elab.elaborate_with_data(
                    module.name,
                    vec![], // params resolved on-demand
                    signals,
                    vec![], // stmts expanded on-demand
                );
            }
        } else {
            self.merged_design = None;
        }

        // Update cache for next incremental compile (use cached clones — designs were consumed by merge).
        // File yang di-restore TIDAK di-clear/di-insert ulang — entry lama di
        // prev_designs tetap valid (konten identik, diverifikasi saat attach).
        self.prev_checksums.clear();
        for (path, design) in paths.iter().zip(cache_designs.iter_mut()) {
            let meta_fp = self.metadata_fingerprint(path);
            self.prev_checksums.insert(path.clone(), meta_fp);
            // MOVE (bukan clone) — cache_designs sudah merupakan clone set utk
            // cache; menyalin sekali lagi = 2x memori AST penuh selama compile
            // (bocor: peak parse ~2.9GB utk OpenTitan dgn 2x clone). Setelah
            // move, cache_designs[i] = None → di-drop.
            if let Some(d) = design.take() {
                self.prev_designs.insert(path.clone(), d);
            }
        }

        // ── Phase 7: Rebuild merged source from collected combined strings ──
        {
            let mut parts = self.combined_parts.lock().unwrap();
            parts.sort_by_key(|(idx, _)| *idx);
            // Pre-allocate merged_source dengan kapasitas tepat (hemat realokasi)
            let total_len: usize = parts.iter().map(|(_, s)| s.len()).sum();
            let mut merged_source = String::with_capacity(total_len);
            // Simpan per-file combined sources untuk incremental compile berikutnya
            self.prev_combined_sources.clear();
            for (path, (_, s)) in paths.iter().zip(parts.iter()) {
                merged_source.push_str(s);
                self.prev_combined_sources.insert(path.clone(), s.clone());
            }
            self.merged_source = Some(merged_source);
            parts.clear();
        }

        self.timing.index_us = index_start.elapsed().as_micros() as u64;
        self.timing.total_us = total_start.elapsed().as_micros() as u64;

        // ── MICD: state compile disimpan eksplisit oleh caller
        // (main.rs) via save_micd() — sekali per build agar statistik
        // changed_files per-build akurat. ──

        Ok((merged, &self.module_index))
    }

    /// Auto-include `<pkg>.sv/.svh` bila `import <pkg>::` dipakai oleh source
    /// yang masuk daftar tapi panggilannya tidak ada di daftar (pola reggen
    /// OpenTitan: `rtl/X_regs_reg_top.sv` import `X_reg_pkg::*` yang di-
    /// generate di dir yang sama). Deterministik: nama file == nama package,
    /// DAN file berada di direktori pengimport (atau libdirs). Tanpa ini,
    /// package param/typedef yang di-import module tak ter-resolve → E2001.
    fn auto_include_missing_packages(&self, files: &mut Vec<PathBuf>) {
        use std::collections::HashSet;
        let mut have: HashSet<PathBuf> = files.iter().cloned().collect();
        let mut basename_set: HashSet<String> = files
            .iter()
            .filter_map(|p| p.file_name().map(|b| b.to_string_lossy().into_owned()))
            .collect();
        // Multi-pass: paket yang baru ditambahkan juga bisa meng-import paket
        // lain yang hilang. Biasanya 1-2 pass sudah selasai.
        for _pass in 0..4 {
            let snapshot = files.clone();
            let mut added = false;
            for src in &snapshot {
                let text = match std::fs::read_to_string(src) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let dir = src.parent().map(|d| d.to_path_buf());
                let bytes = text.as_bytes();
                let mut idx = 0usize;
                while idx < bytes.len() {
                    if bytes.get(idx..idx + 6) == Some(b"import") {
                        let after = idx + 6;
                        let mut start = after;
                        while start < bytes.len() && bytes[start].is_ascii_whitespace() {
                            start += 1;
                        }
                        let mut end = start;
                        while end < bytes.len()
                            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                        {
                            end += 1;
                        }
                        let pkg = &text[start..end];
                        if !pkg.is_empty() {
                            let has = basename_set.contains(&format!("{}.sv", pkg))
                                || basename_set.contains(&format!("{}.svh", pkg));
                            if !has {
                                if let Some(ref d) = dir {
                                    for ext in ["sv", "svh"] {
                                        let cand = d.join(format!("{}.{}", pkg, ext));
                                        if cand.is_file() && have.insert(cand.clone()) {
                                            if let Some(b) = cand
                                                .file_name()
                                                .map(|b| b.to_string_lossy().into_owned())
                                            {
                                                basename_set.insert(b);
                                            }
                                            files.push(cand);
                                            added = true;
                                        }
                                    }
                                }
                            }
                        }
                        idx = end.max(after);
                    } else {
                        idx += 1;
                    }
                }
            }
            if !added {
                break;
            }
        }
    }

    /// Incremental compile — detect changes and only re-process changed files.
    pub fn compile_incremental(
        &mut self,
        force_changed: &[PathBuf],
    ) -> Result<(Design, &ModuleIndex), SimError> {
        // Mark explicitly-changed files as dirty
        for path in force_changed {
            self.incremental.mark_changed(path);
            self.cache.on_file_changed(path);
            // Remove from cache so they get re-processed
            self.prev_checksums.remove(path);
            self.prev_designs.remove(path);
            self.prev_combined_sources.remove(path);
        }

        // Re-compile (will skip unchanged files automatically)
        self.compile()
    }

    /// Compute a fast file identity hash using metadata (mtime + size).
    /// Avoids reading file content, useful for quick change detection.
    fn metadata_fingerprint(&self, path: &PathBuf) -> u64 {
        std::fs::metadata(path)
            .map(|m| {
                let mtime = m
                    .modified()
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos() as u64
                    })
                    .unwrap_or(0);
                let len = m.len();
                // Combine mtime and size into a simple hash
                mtime.wrapping_mul(6364136223846793005).wrapping_add(len)
            })
            .unwrap_or(0)
    }

    /// Detect which files have changed since the last compile.
    /// Uses file metadata (mtime + size) instead of reading file content
    /// to avoid unnecessary I/O for change detection.
    fn detect_changed(&self, files: &[PathBuf]) -> HashSet<PathBuf> {
        // If no previous state, everything is "changed" — first-run shortcut
        if self.prev_checksums.is_empty() {
            return files.iter().cloned().collect();
        }

        let mut changed = HashSet::new();
        for path in files {
            let current = self.metadata_fingerprint(path);
            let prev = self.prev_checksums.get(path);
            match prev {
                Some(cksum) if *cksum == current => {
                    // File unchanged — skip
                }
                _ => {
                    // File changed or new — add to changed set
                    changed.insert(path.clone());
                }
            }
        }
        changed
    }

    fn discover_files(&mut self) -> Result<Vec<PathBuf>, SimError> {
        if !self.config.sources.is_empty() {
            // File template (*.tpl*) bukan SystemVerilog — di-skip global.
            // Sebagian besar jalur (filelist/tool) sudah memfilter di source
            // discovery; filter ini pengaman terakhir untuk pemanggil API yang
            // memasang sources manual.
            let has_tpl = self
                .config
                .sources
                .iter()
                .any(|p| mivon_core::template::is_template_source(p));
            let files: Vec<PathBuf> = self
                .config
                .sources
                .iter()
                .filter(|p| !mivon_core::template::is_template_source(p))
                .cloned()
                .collect();
            if has_tpl && files.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::ModuleNotFound,
                    "semua source file adalah template (*.tpl*) — bukan SystemVerilog",
                ));
            }
            if has_tpl {
                eprintln!(
                    "warning: compile: melewati {} file template (*.tpl*) — bukan SystemVerilog",
                    self.config.sources.len() - files.len()
                );
            }
            return Ok(files);
        }
        if self.config.auto_incdirs {
            let result = FileDiscovery::scan_dir(".", &DiscoveryOptions::default());
            self.timing.discovery_us = result.scan_time_ms * 1000;
            return Ok(result.files.iter().map(|f| f.path.clone()).collect());
        }
        Err(SimError::with_diag(
            DiagCode::ModuleNotFound,
            "no source files configured",
        ))
    }

    /// Build module index, dependency graph, and incremental tracking from parsed designs.
    fn build_index_and_deps(
        &mut self,
        files: &[PathBuf],
        designs: &[Design],
        checksums: &HashMap<PathBuf, u64>,
    ) -> Result<(), SimError> {
        // Temporary mapping: module_name → NodeId (for edge building)
        let mut module_to_node: HashMap<Symbol, crate::scheduler::dag::NodeId> = HashMap::new();

        // ── Pass 1: Insert into index, create DAG nodes, register files ──
        for (i, design) in designs.iter().enumerate() {
            let path = &files[i];
            let checksum = checksums
                .get(path)
                .copied()
                .unwrap_or_else(|| compute_checksum(&std::fs::read(path).unwrap_or_default()));

            let mut module_nodes = Vec::new();

            for module in &design.modules {
                let instance_names: Vec<Symbol> = module
                    .items
                    .iter()
                    .filter_map(|item| {
                        if let mivon_ast::ModuleItem::Instance(inst) = item {
                            Some(inst.module_name)
                        } else {
                            None
                        }
                    })
                    .collect();
                let imports: Vec<(Symbol, Symbol)> = module
                    .items
                    .iter()
                    .filter_map(|item| {
                        if let mivon_ast::ModuleItem::Import {
                            package,
                            item: import_item,
                        } = item
                        {
                            Some((*package, *import_item))
                        } else {
                            None
                        }
                    })
                    .collect();

                self.module_index.insert(
                    module.name,
                    EntryKind::Module,
                    ModuleMeta {
                        name: module.name,
                        file: path.clone(),
                        file_checksum: checksum,
                        ports: module.ports.iter().map(|p| p.name).collect(),
                        params: module
                            .params
                            .iter()
                            .map(|p| ParamMeta {
                                name: p.name,
                                has_default: p.default.is_some(),
                                is_type: p.is_type_param,
                                is_local: false,
                            })
                            .collect(),
                        instances: instance_names.clone(),
                        imports,
                    },
                );

                // Create DAG node for this module
                let node_id = self.dep_graph.add_node(crate::scheduler::Task::ParseFile(
                    path.to_string_lossy().to_string(),
                ));
                module_nodes.push(node_id);
                module_to_node.insert(module.name, node_id);
            }

            for pkg in &design.packages {
                self.module_index.insert(
                    pkg.name,
                    EntryKind::Package,
                    ModuleMeta {
                        name: pkg.name,
                        file: path.clone(),
                        file_checksum: checksum,
                        ports: vec![],
                        params: vec![],
                        instances: vec![],
                        imports: vec![],
                    },
                );
            }

            // Register file in incremental tracker
            self.incremental.register_file(path, module_nodes, checksum);
        }

        // ── Pass 2: Build dependency edges ──
        // Module A instantiates module B → A depends on B (edge B → A)
        for design in designs.iter() {
            for module in &design.modules {
                let mod_sym = module.name;
                let Some(&from) = module_to_node.get(&mod_sym) else {
                    continue;
                };

                for item in &module.items {
                    if let mivon_ast::ModuleItem::Instance(inst) = item {
                        let inst_sym = inst.module_name;
                        if let Some(&to_node) = module_to_node.get(&inst_sym) {
                            // from (instantiator) depends on to (instantiated)
                            self.dep_graph.add_edge(from, to_node);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn create_preprocessor(&self) -> Preprocessor {
        let mut pp = Preprocessor::new();
        for dir in &self.config.incdirs {
            if let Some(s) = dir.to_str() {
                pp.add_search_path(s);
            }
        }
        // Fast-pipeline terpisah dari legacy: legacy men-scan ancestor dirs dan
        // memasang semua parent dir file SV sebagai include search path
        // (collect_sv_dirs di main.rs). run_fast TIDAK melakukannya → include
        // lintas direktori (`dv_macros.svh` di dv_utils/, dsb.) gagal resolve →
        // definisi macro hilang → parse error sisa pada file DV. Samakan
        // perilakunya: selalu tambahkan parent dir setiap source sebagai search
        // path (murah, linear; filelist openTitan = 592 dir unik).
        let mut seen_dirs: HashSet<&std::path::Path> = HashSet::new();
        for src in &self.config.sources {
            if let Some(parent) = src.parent() {
                if seen_dirs.insert(parent) {
                    if let Some(s) = parent.to_str() {
                        pp.add_search_path(s);
                    }
                }
            }
        }
        for (name, value) in &self.config.defines {
            pp.define(name, value);
        }
        // ── Internal UVM library connection (mivon + uvm-core) ──
        // Prioritas PALING RENDAH (setelah incdirs & parent source):
        // 1. `uvm-core/src` (Accellera IEEE 1800.2-2020) bila proyek
        //    mendownloadnya → `include "uvm_pkg.sv" / "uvm.sv" resolve ke
        //    library nyata (verified: uvm_pkg.sv parse 0 error).
        // 2. CWD/repo-root (file uvm_macros.svh kompatibilitas mivon dsb.) →
        //    `include "uvm_macros.svh" di DV resolve ke macro mivon yang
        //    aman di-parse (bukan library penuh yang `uvm_field_* paste
        //    belum di-support preprocessor).
        for cand in [
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("uvm-core/src"),
            std::path::PathBuf::from("vendor/uvm-core/src"),
        ] {
            if cand.join("uvm_macros.svh").is_file() {
                if let Some(s) = cand.to_str() {
                    pp.add_search_path(s);
                }
            }
        }
        pp
    }

    pub fn print_timing(&self) {
        // Show µs for fast phases (< 1ms), ms for slow phases
        let fmt = |us: u64| -> String {
            if us < 1000 {
                format!("{}µs", us)
            } else {
                format!("{}.{:03}ms", us / 1000, us % 1000)
            }
        };
        eprintln!(
            "Compile timing: discovery={} pp={} lex={} parse={} index={} total={} | cached={} processed={}",
            fmt(self.timing.discovery_us),
            fmt(self.timing.preprocess_us),
            fmt(self.timing.lex_us),
            fmt(self.timing.parse_us),
            fmt(self.timing.index_us),
            fmt(self.timing.total_us),
            self.timing.cached_files,
            self.timing.processed_files,
        );
    }

    /// Get the top module name as Symbol (if configured).
    pub fn interned_top_module(&self) -> Option<Symbol> {
        self.config.top_module.as_ref().map(|s| Symbol::intern(s))
    }

    /// Get module metadata by Symbol from the module index.
    pub fn get_module_by_sym(
        &self,
        name: Symbol,
    ) -> Option<crate::frontend::module_index::ModuleMeta> {
        self.module_index
            .lookup(name, crate::frontend::module_index::EntryKind::Module)
    }

    /// Get module metadata by string name.
    pub fn get_module_by_name(
        &self,
        name: &str,
    ) -> Option<crate::frontend::module_index::ModuleMeta> {
        self.module_index.lookup(
            Symbol::intern(name),
            crate::frontend::module_index::EntryKind::Module,
        )
    }

    /// Get the number of configured source files (not auto-discovered).
    pub fn source_count(&self) -> usize {
        self.config.sources.len()
    }

    /// Enable profiling for this session.
    pub fn enable_profiling(&mut self) {
        self.profiler = Some(Profiler::new());
    }

    /// Get profiling report.
    pub fn profile_report(&self) -> Option<crate::profiling::ProfileReport> {
        self.profiler.as_ref().map(|p| p.report())
    }

    /// Set remote cache backend with sync mode.
    pub fn set_remote_cache(
        &mut self,
        backend: Arc<dyn RemoteCacheBackend>,
        sync_mode: RemoteSyncMode,
    ) {
        self.cache.set_remote_backend(backend);
        self.cache.set_remote_sync_mode(sync_mode);
    }

    /// Clear all caches (local + remote if configured).
    ///
    /// `clear_micd=false`: MICD tidak ikut dihapus. Dipakai `--cache-clear`
    /// pada akhir run — MICD sudah dihapus di AWAL run (sebelum attach), jadi
    /// clear lagi di akhir hanya membuang hasil rebuild fresh yang baru
    /// disimpan (run berikutnya jadi rebuild penuh lagi).
    pub fn clear_cache(&mut self, clear_micd: bool) {
        self.cache.clear();
        // If remote is set, also clear remote
        if let Some(ref backend) = self.cache.remote {
            let _ = backend.clear();
        }
        // Clear MICD persistent database if attached
        if clear_micd {
            if let Some(db) = self.micd.as_mut() {
                let _ = db.clear();
            }
        }
        self.micd_restored = 0;
        self.lexer_payloads.lock().unwrap().clear();
        self.prev_checksums.clear();
        self.prev_designs.clear();
        self.prev_combined_sources.clear();
        self.cached_elab_modules.clear();
        self.cached_ir_design = None;
        self.merged_design = None;
        self.merged_source = None;
    }

    /// Get cache statistics.
    pub fn cache_stats(&self) -> crate::cache::CacheStats {
        self.cache.stats()
    }

    // ─────────────────────────── MICD ───────────────────────────
    // Persistent incremental compilation database. Terpasang secara eksplisit
    // dari CLI (bukan default di new() — agar test tidak menulis database).

    /// Pasang MICD dan restore state file yang tidak berubah.
    /// Untuk tiap source yang hash kontennya cocok dengan cache, AST dan
    /// combined source di-deserialize → parse/lex di-skip pada compile.
    /// Mengembalikan jumlah AST yang berhasil di-restore.
    /// Buka database MICD TANPA restore AST (dipakai saat `--recompile`).
    ///
    /// Sebelumnya `--recompile` melewati `attach_micd` sama sekali sehingga
    /// `self.micd` tetap `None` dan `save_micd` menjadi no-op — cache hasil
    /// full-rebuild tidak pernah ditulis ke disk, dan run normal berikutnya
    /// selalu compile penuh ulang (restored=0). Di sini database dibuka dan
    /// flags/include-deps disalin, tapi restore dilewati: semua file dianggap
    /// fresh (rebuild penuh) dan save berikutnya menulis ulang seluruh store.
    pub fn open_micd_no_restore(&mut self, mut db: MicdDatabase) {
        let current_flags = micd::flags_hash(&self.config.defines, &self.config.incdirs);
        db.flags_hash = current_flags;
        db.dirty = true;

        self.collect_micd_include_deps(&db);

        db.restored = 0;
        self.micd = Some(db);
        self.micd_restored = 0;
        self.micd_restored_paths = HashSet::new();
    }

    /// Salin include deps dari database (dipakai saat save ulang).
    fn collect_micd_include_deps(&mut self, db: &MicdDatabase) {
        let mut include_deps = HashMap::new();
        for (p, meta) in db.files.iter() {
            if !meta.include_hashes.is_empty() {
                include_deps.insert(
                    p.clone(),
                    meta.include_hashes.iter().map(|(d, _)| d.clone()).collect(),
                );
            }
        }
        self.micd_include_deps = include_deps;
    }

    pub fn attach_micd(&mut self, mut db: MicdDatabase) -> usize {
        let current_flags = micd::flags_hash(&self.config.defines, &self.config.incdirs);
        // Koreksi correctness: defines/incdirs berubah → preprocessed output
        // meng-embed ekspansi makro lama → SEMUA file harus di-reprocess.
        let flags_changed = db.flags_hash != 0 && db.flags_hash != current_flags;
        db.flags_hash = current_flags;
        if flags_changed {
            db.dirty = true;
        }
        // Set flags_hash untuk save berikutnya (sebelum restore tidak
        // diperlukan; record_file memakai current_flags pada save_micd).

        let mut restored = 0usize;
        let mut restored_paths = HashSet::new();
        let sources: Vec<PathBuf> = self
            .config
            .sources
            .iter()
            .chain(self.config.libfiles.iter())
            // F10: path inline (buffer transpile `.mv`) TIDAK di-restore —
            // kontennya tidak stabil di disk (hash basis buffer vs .mv) dan
            // selalu di-transpile ulang tiap run. MICD hanya untuk file disk.
            .filter(|p| !self.config.inline_sources.contains_key(*p))
            .cloned()
            .collect();

        if !flags_changed {
            // Parallel restore: baca + hash + deserialize per file (independen).
            // `&db` aman dibagi antar thread (MicdDatabase adalah Sync).
            let db_ref = &db;
            let restored_items: Vec<(PathBuf, Design, String)> = sources
                .par_iter()
                .filter_map(|path| {
                    let Ok(content) = std::fs::read(path) else {
                        return None;
                    };
                    let hash = compute_checksum(&content);
                    // Koreksi correctness: jangan reuse bila header berubah.
                    if !db_ref.deps_unchanged(path, hash).unwrap_or(false) {
                        return None;
                    }
                    // BUG #2 (CRITICAL): file dengan diagnostik parse TIDAK
                    // boleh di-restore — AST cache tidak membawa error-nya,
                    // sehingga run hangat melaporkan 0 error dan desain yang
                    // gagal parse lolos ke simulasi (silent miscompilation).
                    // File ini di-parse ulang tiap run → jumlah & isi error
                    // deterministik: dingin == hangat. Entry `diag.mdb`
                    // dikunci ke content hash yang sama; file berubah →
                    // hash beda → tidak menahan restore (AST miss sudah
                    // cukup untuk memaksa parse ulang).
                    if db_ref
                        .get_diags(path)
                        .is_some_and(|d| d.content_hash == hash && !d.entries.is_empty())
                    {
                        return None;
                    }
                    let ast = db_ref
                        .get_ast(path, hash)
                        .and_then(|bytes| micd::deserialize_design(&bytes))?;
                    let preproc = db_ref.get_preprocessed(path, hash)?;
                    Some((path.clone(), ast, preproc.combined))
                })
                .collect();

            for (path, design, combined) in restored_items {
                self.prev_designs.insert(path.clone(), design);
                self.prev_checksums
                    .insert(path.clone(), self.metadata_fingerprint(&path));
                self.prev_combined_sources.insert(path.clone(), combined);
                // Touch LRU (Kritik 6 db.md): file yang di-restore dianggap
                // baru diakses → tidak di-evict GC.
                if let Some(h) = db.files.get(&path).map(|m| m.content_hash) {
                    db.touch_ast(&path, h);
                    db.touch_preproc(&path, h);
                    db.touch_verify(h);
                }
                restored += 1;
                restored_paths.insert(path);
            }
        }

        // ── Debug MICD (guard env MIVON_DBG_MICD): ukur titik kegagalan
        // restore agar run hangat tidak full rebuild (cached=0). ──
        if std::env::var("MIVON_DBG_MICD").is_ok() {
            let db_ref = &db;
            let mut deps_ok = 0usize;
            let mut ast_ok = 0usize;
            let mut pre_ok = 0usize;
            for path in &sources {
                if let Ok(content) = std::fs::read(path) {
                    let hash = compute_checksum(&content);
                    if db_ref.deps_unchanged(path, hash).unwrap_or(false) {
                        deps_ok += 1;
                        if db_ref.get_ast(path, hash).is_some() {
                            ast_ok += 1;
                            if db_ref.get_preprocessed(path, hash).is_some() {
                                pre_ok += 1;
                            }
                        }
                    }
                }
            }
            eprintln!(
                "[MICD-DBG] attach: files={} flags(db)={:x} flags(cur)={:x} flags_changed={} deps_ok={} ast_ok={} preproc_ok={} restored={}",
                db.files.len(),
                db.flags_hash,
                current_flags,
                flags_changed,
                deps_ok,
                ast_ok,
                pre_ok,
                restored
            );
        }

        self.collect_micd_include_deps(&db);

        db.restored = restored;
        db.dirty = false;
        self.micd = Some(db);
        self.micd_restored = restored;
        self.micd_restored_paths = restored_paths;
        restored
    }

    /// Simpan seluruh state compile ke MICD (file terdaftar, AST, combined
    /// source, dependency graph, symbol index, verify cache, diag).
    pub fn save_micd(&mut self) -> Result<Option<MicdStats>, String> {
        if self.micd.is_none() {
            return Ok(None);
        }

        let flags = micd::flags_hash(&self.config.defines, &self.config.incdirs);
        let file_deps = self.compute_file_deps();

        // ── Fase 1: kumpulkan data per file (ringan untuk warm run —
        // file restored tidak dibaca ulang / tidak diserialize). ──
        let t_gather = std::time::Instant::now();
        // Cache AST-hash per path utk fase 2 (verify ast_hash) — sebelumnya
        // serialize_design() dieksekusi 2x per file (1270 + 1359) → ~1GB
        // serialize ulang sia-sia. Compute hash SEKALI dari bytes fase 1.
        let mut ast_hash_cache: HashMap<PathBuf, u64> = HashMap::new();
        let mut items: Vec<(
            PathBuf,
            u64,
            u64,
            Vec<PathBuf>,
            micd::FileStatus,
            Option<String>,
            Option<Vec<u8>>,
            Vec<(PathBuf, u64)>,
        )> = Vec::new();
        let mut hash_by_path: HashMap<PathBuf, u64> = HashMap::new();
        let mut built_changed = 0usize;

        // ── Parallel gather ──
        // save_micd untuk design besar terukur lambat: 32.9s gather utk 3888
        // file OpenTitan (fs::read + serialize_design bincode + include hash —
        // SEQUENTIAL). Semua komponen baca-only → par_iter (skala core).
        use rayon::prelude::*;
        let entries: Vec<(PathBuf, &Design)> = self
            .prev_designs
            .iter()
            .map(|(p, d)| (p.clone(), d))
            .collect();
        // Pre-hash SEMUA distinct include sekali (paralel) — cache by path
        // tanpa kontensi Mutex antar worker.
        let distinct_incs: std::collections::HashSet<PathBuf> =
            self.micd_include_deps.values().flatten().cloned().collect();
        let inc_hashes: HashMap<PathBuf, u64> = distinct_incs
            .par_iter()
            .map(|inc| {
                let h = std::fs::read(inc)
                    .map(|b| compute_checksum(&b))
                    .unwrap_or(0);
                (inc.clone(), h)
            })
            .collect();

        let gathered: Vec<(
            Option<PathBuf>, // None = inline (skip)
            u64,             // content_hash
            u64,             // size
            Vec<PathBuf>,    // deps
            micd::FileStatus,
            Option<String>,      // combined
            Option<Vec<u8>>,     // design_bytes
            Vec<(PathBuf, u64)>, // include_hashes
            u64,                 // ast_hash
            bool,                // was_changed
        )> = entries
            .par_iter()
            .map(|(path, design)| {
                // F10: path inline (buffer transpile `.mv`) tidak direkam ke MICD.
                if self.config.inline_sources.contains_key(path) {
                    return (
                        None,
                        0,
                        0,
                        Vec::new(),
                        micd::FileStatus::Unchanged,
                        None,
                        None,
                        Vec::new(),
                        0,
                        false,
                    );
                }
                let is_restored = self.micd_restored_paths.contains(path);
                let (content_hash, combined, design_bytes, ast_hash) = if is_restored {
                    let h = self
                        .micd
                        .as_ref()
                        .and_then(|d| d.get_file_meta(path))
                        .map(|m| m.content_hash)
                        .unwrap_or(0);
                    (h, None, None, 0)
                } else {
                    let content_hash = std::fs::read(path)
                        .map(|b| compute_checksum(&b))
                        .unwrap_or(0);
                    let combined = self.prev_combined_sources.get(path).cloned();
                    let design_bytes = micd::serialize_design(design).ok();
                    let ast_hash = design_bytes
                        .as_ref()
                        .map(|b| compute_checksum(b))
                        .unwrap_or(0);
                    (content_hash, combined, design_bytes, ast_hash)
                };
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                let deps = file_deps.get(path).cloned().unwrap_or_default();
                let include_hashes: Vec<(PathBuf, u64)> = if is_restored {
                    self.micd
                        .as_ref()
                        .and_then(|d| d.get_file_meta(path))
                        .map(|m| m.include_hashes.clone())
                        .unwrap_or_default()
                } else {
                    self.micd_include_deps
                        .get(path)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|inc| {
                            let h = inc_hashes.get(&inc).copied().unwrap_or(0);
                            (inc, h)
                        })
                        .collect()
                };
                let prev_hash = self
                    .micd
                    .as_ref()
                    .and_then(|d| d.get_file_meta(path))
                    .map(|m| m.content_hash);
                let was_changed = prev_hash != Some(content_hash);
                let status = if is_restored {
                    micd::FileStatus::Unchanged
                } else {
                    micd::FileStatus::Recompiled
                };
                (
                    Some(path.clone()),
                    content_hash,
                    size,
                    deps,
                    status,
                    combined,
                    design_bytes,
                    include_hashes,
                    ast_hash,
                    was_changed,
                )
            })
            .collect();

        for g in gathered {
            if let Some(p) = g.0 {
                hash_by_path.insert(p.clone(), g.1);
                ast_hash_cache.insert(p.clone(), g.8);
                if g.9 {
                    built_changed += 1;
                }
                items.push((p, g.1, g.2, g.3, g.4, g.5, g.6, g.7));
            }
        }
        let full_write = built_changed > 0;

        // ── Fase 2: turunan (symbols / verify / type signatures) hanya
        // dibangun bila ada perubahan — warm run (0 perubahan) melewatinya
        // sehingga save menjadi near-no-op dan verify lama tidak di-downgrade. ──
        let mut symbols: Vec<(String, String, PathBuf)> = Vec::new();
        let mut verify_results: Vec<micd::VerifyResult> = Vec::new();
        let mut type_entries: Vec<(String, u64)> = Vec::new();
        let mut symbol_defs: Vec<(String, PathBuf)> = Vec::new();
        let mut symbol_uses: Vec<(PathBuf, String)> = Vec::new();
        if full_write {
            for (path, design) in &self.prev_designs {
                // F10: konsisten dengan loop `items` di atas — path inline
                // (buffer transpile `.mv`) tidak ikut turunan (symbols /
                // verify / type_entries) agar tidak merekam entry hash 0.
                if self.config.inline_sources.contains_key(path) {
                    continue;
                }
                for m in &design.modules {
                    symbols.push((m.name.to_string(), "module".to_string(), path.clone()));
                }
                for p in &design.packages {
                    symbols.push((p.name.to_string(), "package".to_string(), path.clone()));
                }
                for c in &design.classes {
                    symbols.push((c.name.to_string(), "class".to_string(), path.clone()));
                }
                let content_hash = hash_by_path.get(path).copied().unwrap_or(0);
                // Multi-level hash (Kritik 1 db.md): AST hash untuk reuse saat
                // content hash berubah tapi AST identik (mis. komentar).
                let is_restored = self.micd_restored_paths.contains(path);
                let ast_hash = if is_restored {
                    self.micd
                        .as_ref()
                        .and_then(|d| d.get_verify(content_hash))
                        .map(|v| v.ast_hash)
                        .unwrap_or(0)
                } else {
                    // Hash dihitung dari bytes fase 1 (serialize_design 1x),
                    // bukan serialize ulang.
                    ast_hash_cache.get(path).copied().unwrap_or(0)
                };
                let mut v = micd::VerifyResult::fresh(content_hash);
                v.ast_hash = ast_hash;
                v.parse_ok = true;
                v.parse_ms = self
                    .timing
                    .preprocess_us
                    .saturating_add(self.timing.lex_us)
                    .saturating_add(self.timing.parse_us);
                // Kritik 9: hasil verifikasi dipisah per kategori.
                v.set_check(
                    micd::VerifyCheckKind::Parse,
                    micd::CheckResult::pass(ast_hash),
                );
                v.set_check(micd::VerifyCheckKind::Elaborate, micd::CheckResult::fresh());
                verify_results.push(v);

                // types.mdb: signature module (deteksi perubahan struktural)
                // sekaligus semantic hash (Kritik 1 — reuse bila signature
                // tipe/port/param tidak berubah).
                let mut semantic = 0u64;
                for m in &design.modules {
                    let mut sig = 0u64;
                    sig = sig
                        .wrapping_mul(31)
                        .wrapping_add(compute_checksum(m.name.as_str().as_bytes()));
                    for p in &m.ports {
                        sig = sig
                            .wrapping_mul(31)
                            .wrapping_add(compute_checksum(p.name.as_str().as_bytes()));
                    }
                    for pr in &m.params {
                        sig = sig
                            .wrapping_mul(31)
                            .wrapping_add(compute_checksum(pr.name.as_str().as_bytes()));
                    }
                    semantic = semantic.wrapping_mul(31).wrapping_add(sig);
                    type_entries.push((m.name.to_string(), sig));
                }
                if let Some(v) = verify_results.last_mut() {
                    v.semantic_hash = semantic;
                }
            }
            // Kritik 2 db.md: dependency level simbol. Definisikan simbol yang
            // ada di file ini + catat simbol yang dipakai (instance & import
            // item). Wildcard import ("*") dilewati — dependensinya sudah
            // ditangkap oleh edge file-level. Dikumpulkan di sini, diterapkan
            // ke db di Fase 3.
            let sym_to_file: HashMap<Symbol, PathBuf> = self
                .module_index
                .iter()
                .map(|(name, _kind, meta)| (name, meta.file.clone()))
                .collect();
            for (name, kind, meta) in self.module_index.iter() {
                if kind != EntryKind::Module {
                    continue;
                }
                symbol_defs.push((name.as_str().to_string(), meta.file.clone()));
                for inst in &meta.instances {
                    symbol_uses.push((meta.file.clone(), inst.as_str().to_string()));
                }
                for (_pkg, item) in &meta.imports {
                    if item.as_str() == "*" {
                        continue;
                    }
                    symbol_uses.push((meta.file.clone(), item.as_str().to_string()));
                    if let Some(def_file) = sym_to_file.get(item) {
                        symbol_defs.push((item.as_str().to_string(), def_file.clone()));
                    }
                }
            }
        }

        // ── Fase 2b: lapisan cache pipeline (db.md cache/, baris 1141-1605) ──
        // Isi kategori cache (preprocess/lexer/parser/semantic/type/constant/
        // hierarchy/resolve/macro/include/dependency/verify) dari data compile.
        // Hanya saat ada perubahan (full_write) — warm run melewati (entry lama
        // tetap valid). Save store-nya ikut `db.save()` di Fase 3.
        if full_write {
            let prev_profile = self.micd.as_ref().and_then(|d| d.stats_db.last()).cloned();
            let module_file: HashMap<String, PathBuf> = self
                .module_index
                .iter()
                .map(|(name, _kind, meta)| (name.to_string(), meta.file.clone()))
                .collect();
            let lexer_payloads = std::mem::take(&mut self.lexer_payloads)
                .into_inner()
                .unwrap_or_default();
            let input = crate::micd::cache::pipeline::CachePopulateInput {
                designs: self.prev_designs.iter().collect(),
                combined: &self.prev_combined_sources,
                defines: &self.config.defines,
                include_deps: &self.micd_include_deps,
                include_hashes: &inc_hashes,
                lexer_payloads,
                symbols: symbols.clone(),
                type_entries: type_entries.clone(),
                verify: verify_results.clone(),
                module_file,
                profile: prev_profile,
                // IR hanya tersedia bila save_micd dipanggil SETELAH
                // compile_and_elaborate (jalur tool). Jalur run_fast memanggil
                // save_micd sebelum elaborate — kategori elaborate/generate
                // diisi belakangan via save_elaborate_cache().
                ir_design: self.cached_ir_design.as_ref(),
                // Jalur tool (compile_and_elaborate) tidak membawa design
                // post-expansion elaborator; fallback elaborate/ memakai
                // designs (pre-expansion).
                expanded_design: None,
                opt_snapshot: None,
            };
            let mut layer = self.micd.as_mut().and_then(|d| d.cache_layer.take());
            if let Some(layer) = layer.as_mut() {
                crate::micd::cache::pipeline::CachePopulator::populate(layer, &input);
            }
            if let Some(db) = self.micd.as_mut() {
                db.cache_layer = layer;
            }
        }

        // ── Fase 3: terapkan ke db + simpan (hanya store yang dirty). ──
        let db = self.micd.as_mut().expect("checked above");
        if std::env::var("MIVON_DEBUG_MICD").is_ok() {
            eprintln!("[MICD-DBG] gather loop = {:?}", t_gather.elapsed());
        }
        // MICD adalah cache per-project: file aktif sesi ini (untuk prune
        // file lintas-project yang menempel di root yang sama).
        let active: Vec<PathBuf> = items
            .iter()
            .map(|(p, _, _, _, _, _, _, _)| p.clone())
            .collect();
        let t_apply = std::time::Instant::now();
        for (path, content_hash, size, deps, status, combined, design_bytes, include_hashes) in
            items
        {
            db.record_file(
                path.clone(),
                content_hash,
                deps,
                status,
                flags,
                size,
                include_hashes,
            );
            if let Some(bytes) = design_bytes {
                db.cache_ast(path.clone(), content_hash, bytes);
            }
            if let Some(combined) = combined {
                db.cache_preprocessed(
                    path.clone(),
                    PreprocEntry {
                        content_hash,
                        combined,
                        timescale: None,
                        // F84: jalur ini menyimpan entry dari sumber yang tak
                        // menyimpan segmen — parser akan pakai satuan global
                        // untuk file tersebut (perilaku pre-F84, tak diam-diam
                        // salah untuk design multi-timescale).
                        timescale_segments: Vec::new(),
                    },
                );
            }
            // BUG #2: rekam diagnostik parse per file ke `diag.mdb` —
            // disamping fungsinya IDE/LSP (file → daftar diagnostic tanpa
            // compile ulang), entry ini menjadi penanda di attach_micd agar
            // file ber-diagnostik tidak di-restore (error selalu terlapor di
            // run hangat). File yang di-restore TIDAK disentuh: tidak diparse
            // ulang → diagnostiknya tidak berubah. Tulis hanya bila berubah
            // agar run hangat tidak menandai db dirty tanpa alasan.
            if !self.micd_restored_paths.contains(&path) {
                let new_entries: Vec<micd::DiagEntry> = self
                    .file_parse_diags
                    .get(&path)
                    .map(|ds| ds.iter().map(diag_to_entry).collect())
                    .unwrap_or_default();
                let unchanged = db.get_diags(&path).is_some_and(|old| {
                    old.content_hash == content_hash && old.entries == new_entries
                });
                if !unchanged {
                    if new_entries.is_empty() {
                        db.diags.remove(&path);
                        db.dirty_diag = true;
                    } else {
                        db.set_diags(micd::FileDiags {
                            path: path.clone(),
                            entries: new_entries,
                            content_hash,
                        });
                    }
                }
            }
        }
        if full_write {
            for (name, kind, path) in symbols {
                db.add_symbol(name, kind, path);
            }
            for (name, sig) in type_entries {
                db.set_module_type(name, sig);
            }
            for v in verify_results {
                db.set_verify(v);
            }
            for (file, deps) in &file_deps {
                db.set_file_deps(file.clone(), deps.clone());
            }
            // Kritik 2: dependency level simbol.
            for (name, file) in symbol_defs {
                db.set_symbol_def(name, file);
            }
            for (file, name) in symbol_uses {
                db.add_symbol_use(file, name);
            }
        }

        // MICD adalah cache per-project: buang file yang bukan bagian dari
        // sources sesi ini (akumulasi run lain di root yang sama = sampah).
        let pruned = db.prune_stale(&active);
        if std::env::var("MIVON_DEBUG_MICD").is_ok() {
            eprintln!("[MICD-DBG] prune_stale removed {} file(s)", pruned);
        }
        if std::env::var("MIVON_DEBUG_MICD").is_ok() {
            eprintln!("[MICD-DBG] apply loop = {:?}", t_apply.elapsed());
        }

        let cum_changed = db.changed;
        let t_save = std::time::Instant::now();
        let mut stats = db.save().map_err(|e| e.to_string())?;
        if std::env::var("MIVON_DEBUG_MICD").is_ok() {
            eprintln!("[MICD-DBG] db.save() = {:?}", t_save.elapsed());
        }
        stats.changed_files = built_changed;
        // Auto-snapshot build (seperti commit git) saat ada perubahan nyata.
        if built_changed > 0 && cum_changed > db.last_snapshotted_changed {
            db.last_snapshotted_changed = cum_changed;
            if let Ok(id) = db.snapshot(format!("build: {} file(s) recompiled", built_changed)) {
                stats.snapshot_id = id;
            }
        }

        // Kritik 14 db.md: rekam profil build ke stats.mdb (save kedua ringan,
        // hanya menulis stats.mdb).
        let t_stats = std::time::Instant::now();
        let mut prof = db.stats_db.next_profile();
        prof.total_ms = self.timing.total_us;
        prof.preprocess_ms = self.timing.preprocess_us;
        prof.lex_ms = self.timing.lex_us;
        prof.parse_ms = self.timing.parse_us;
        prof.elaborate_ms = self.timing.elab_us;
        prof.save_ms = t_save.elapsed().as_millis() as u64;
        prof.files = db.files.len();
        prof.changed_files = built_changed;
        prof.dirty_nodes = built_changed;
        prof.restored_designs = self.micd_restored;
        prof.cache_hits = self.micd_restored;
        prof.cache_misses = db.files.len().saturating_sub(self.micd_restored);
        prof.peak_mem_kb = micd::peak_rss_kb();
        prof.snapshot_id = stats.snapshot_id;
        // Serialize SEBELUM dipindah ke stats_db (dipakai cache profile/).
        let prof_bytes = bincode::serialize(&prof).ok();
        db.set_stats(prof);
        let _ = db.save().map_err(|e| e.to_string())?;
        if std::env::var("MIVON_DEBUG_MICD").is_ok() {
            eprintln!("[MICD-DBG] stats save = {:?}", t_stats.elapsed());
        }

        // profile cache (db.md cache/ "20. profile/"): profil build terakhir.
        if let Some(prof_bytes) = prof_bytes {
            if let Some(layer) = self.micd.as_mut().and_then(|d| d.cache_layer.as_mut()) {
                layer.put(crate::micd::CacheCategory::Profile, "last", &prof_bytes);
                let _ = layer.save();
            }
        }

        self.micd_restored = 0;

        // Populate precompiled database (VCS AN.DB / Questa _info analog).
        // Artefak per module disimpan agar tool downstream baca tanpa compile ulang.
        self.populate_precompiled();

        Ok(Some(stats))
    }

    /// Tandai seluruh file sebagai ter-elaborasi (verify cache: elab_ok=true)
    /// dan simpan verify.mdb SAJA (ringan, tanpa tulis ulang metadata/ast).
    pub fn micd_mark_elaborated(&mut self) {
        if let Some(db) = self.micd.as_mut() {
            let _ = db.mark_elaborated();
        }
    }

    /// Isi kategori cache elaborate/ + generate/ dari IR hasil elaborasi
    /// (db.md "5. elaborate/", "16. generate/"). Dipanggil SETELAH elaborasi
    /// sukses — save_micd (sebelum elaborate) tidak punya IR, jadi kategori
    /// ini diisi di sini agar warm run berikutnya dapat membacanya tanpa
    /// menjalankan elaborator ulang. Best-effort: kegagalan tidak fatal.
    ///
    /// `expanded_design` adalah design SETELAH generate expansion (milik
    /// elaborator, `elab.design`) — dipakai fallback elaborate/ untuk module
    /// top yang IR-nya di-flatten. Boleh `None` (fallback memakai
    /// `prev_designs` yang pre-expansion).
    pub fn save_elaborate_cache(
        &mut self,
        ir: &mivon_ir::IrDesign,
        expanded_design: Option<&mivon_ast::types::Design>,
        opt_snapshot: Option<mivon_elaboration::util::OptimizeSnapshot>,
    ) {
        let Some(db) = self.micd.as_mut() else { return };
        let module_file: HashMap<String, PathBuf> = self
            .module_index
            .iter()
            .map(|(name, _kind, meta)| (name.to_string(), meta.file.clone()))
            .collect();
        let input = crate::micd::cache::pipeline::CachePopulateInput {
            designs: self.prev_designs.iter().collect(),
            combined: &self.prev_combined_sources,
            defines: &self.config.defines,
            include_deps: &self.micd_include_deps,
            include_hashes: &HashMap::new(),
            lexer_payloads: vec![],
            symbols: vec![],
            type_entries: vec![],
            verify: vec![],
            module_file,
            profile: None,
            ir_design: Some(ir),
            expanded_design,
            opt_snapshot,
        };
        // Simpan IrDesign LENGKAP (bincode) di key `ir:<top>` — dipakai warm
        // run untuk melewati elaborator sepenuhnya (db.md "5. elaborate/"):
        // 1000 instance generate tidak perlu dielaborasi ulang. Dipanggil
        // SEBELUM populate (store juga memakai cache_layer, hindari double
        // borrow).
        db.store_elaborate_ir(ir);
        // Update precompiled modules dengan IR bytes (tool downstream bisa
        // skip elaborasi bila IR tersedia di precompiled). IR raksasa
        // (array memori 1e9 bit) TIDAK di-serialize: bytes ~1GB lalu di-clone
        // PER MODULE → 29GB live arena (heaptrack) & OOM mesin kecil.
        if let Some(pdb) = db.precompiled_db.as_mut() {
            let ir_bytes = if ir_est_bits_large(ir) {
                Vec::new()
            } else {
                bincode::serialize(ir).unwrap_or_default()
            };
            for module in pdb.modules.values_mut() {
                if !module.ir_bytes.is_empty() || module.error_count == 0 {
                    if ir_bytes.is_empty() {
                        // IR raksasa: tandai kosong (tool re-elaborate).
                        module.ir_bytes = Vec::new();
                        pdb.dirty = true;
                    } else {
                        module.ir_bytes = ir_bytes.clone();
                        module.checksum = module.compute_checksum();
                        pdb.dirty = true;
                    }
                }
            }
        }
        let layer = match db.cache_layer.as_mut() {
            Some(l) => l,
            None => return,
        };
        crate::micd::cache::pipeline::CachePopulator::populate_elab(layer, &input);
        if let Err(e) = layer.save() {
            eprintln!("[MICD] elaborate/generate cache save warning: {}", e);
        }
    }

    /// Jumlah AST yang di-restore dari MICD pada sesi ini.
    pub fn micd_restored_count(&self) -> usize {
        self.micd_restored
    }

    /// Jumlah file yang di-restore dari MICD — SAMA dengan
    /// `micd_restored_count()` TAPI tidak di-reset oleh `save_micd()` (set
    /// `micd_restored` di-nol-kan di akhir save, set path tetap). Dipakai
    /// memutuskan reuse IR cache SETELAH save parse (lihat run_fast).
    pub fn micd_restored_paths_count(&self) -> usize {
        self.micd_restored_paths.len()
    }

    /// Coba restore `IrDesign` hasil elaborasi dari cache pipeline (db.md
    /// "5. elaborate/"). Dipanggil pada warm run (seluruh file tidak berubah
    /// — MICD restore penuh) agar elaborator bisa di-skip sepenuhnya.
    /// `None` bila tidak ada entry / corrupt / top berbeda (pemanggil fallback
    /// ke elaborasi penuh).
    pub fn restore_elaborate_ir(&mut self, top: &str) -> Option<mivon_ir::IrDesign> {
        self.micd.as_mut()?.restore_elaborate_ir(top)
    }

    /// Isi precompiled database dari hasil compile sesi ini (per module).
    /// Dipanggil setelah parsing/compile berhasil. Menyimpan artefak per module
    /// (AST, type signature, port info, dependensi) agar tool downstream
    /// (`mlint`, `melab`, `msim`) bisa baca tanpa compile ulang.
    /// Best-effort: kegagalan tidak fatal.
    pub fn populate_precompiled(&mut self) {
        let Some(db) = self.micd.as_mut() else { return };
        let Some(pdb) = db.precompiled_db.as_mut() else {
            return;
        };

        // Legacy path: prev_designs kosong (source digabung, bukan per-file).
        if self.prev_designs.is_empty() {
            return;
        }

        // Kumpulkan IR bytes bila tersedia (untuk simpan di precompiled).
        let ir_bytes: Vec<u8> = self
            .cached_ir_design
            .as_ref()
            .and_then(|ir| bincode::serialize(ir).ok())
            .unwrap_or_default();

        for (path, design) in &self.prev_designs {
            let content_hash = std::fs::read(path)
                .map(|b| crate::cache::compute_checksum(&b))
                .unwrap_or(0);

            // AST bytes (disimpan untuk full restore tool downstream).
            let ast_bytes = bincode::serialize(design).unwrap_or_default();
            let ast_hash = crate::cache::compute_checksum(&ast_bytes);

            for m in &design.modules {
                let name = m.name.to_string();

                // Skip bila fingerprint tidak berubah.
                if pdb.has_valid(&name, content_hash) {
                    continue;
                }

                // Type signature.
                let mut sig = 0u64;
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add(crate::cache::compute_checksum(name.as_bytes()));
                for p in &m.ports {
                    sig = sig
                        .wrapping_mul(31)
                        .wrapping_add(crate::cache::compute_checksum(p.name.as_str().as_bytes()));
                }
                for pr in &m.params {
                    sig = sig
                        .wrapping_mul(31)
                        .wrapping_add(crate::cache::compute_checksum(pr.name.as_str().as_bytes()));
                }

                // Port info.
                use mivon_ast::types::PortDirection;
                let ports: Vec<crate::micd::precompiled::PortInfo> = m
                    .ports
                    .iter()
                    .map(|p| {
                        let dir = match p.direction {
                            PortDirection::Input => "input",
                            PortDirection::Output => "output",
                            PortDirection::Inout => "inout",
                            PortDirection::Ref => "ref",
                        };
                        crate::micd::precompiled::PortInfo {
                            name: p.name.to_string(),
                            dir: dir.to_string(),
                            width: p.range.as_ref().map(|r| r.width()).unwrap_or(1),
                            is_signed: false,
                        }
                    })
                    .collect();

                // Dependensi: module lain yang diinstansiasi / di-import.
                let depends_on: Vec<String> = m
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        mivon_ast::types::ModuleItem::Instance(inst) => {
                            Some(inst.module_name.to_string())
                        }
                        mivon_ast::types::ModuleItem::Import { package, .. } => {
                            Some(package.to_string())
                        }
                        _ => None,
                    })
                    .collect();

                // Process count.
                let process_count = m
                    .items
                    .iter()
                    .filter(|item| {
                        matches!(
                            item,
                            mivon_ast::types::ModuleItem::Always(_)
                                | mivon_ast::types::ModuleItem::Initial(_)
                                | mivon_ast::types::ModuleItem::Final(_)
                        )
                    })
                    .count();

                // Error/warning dari design (bila ada parse error count).
                let error_count = 0; // TODO: dari session.parse_errors
                let warn_count = 0;

                let mut module = crate::micd::precompiled::PrecompiledModule {
                    name: name.clone(),
                    content_hash,
                    ast_hash,
                    type_signature: sig,
                    source_file: path.clone(),
                    analyzed_at_ns: crate::micd::verify::now_ns(),
                    library: "work".to_string(),
                    ports,
                    depends_on,
                    depended_by: vec![],
                    token_count: 0,
                    process_count,
                    signal_count: 0,
                    error_count,
                    warn_count,
                    ast_bytes: ast_bytes.clone(),
                    ir_bytes: ir_bytes.clone(),
                    checksum: 0,
                };
                module.checksum = module.compute_checksum();
                pdb.put(module);
            }
        }
        let _ = pdb.save();
    }

    /// Coba restore module dari precompiled database. Bila fingerprint
    /// (content_hash) cocok, AST dan IR bisa di-skip. Mengembalikan
    /// (ast_design, ir_design) bila ada, None bila tidak ada/corrupt.
    pub fn restore_precompiled(
        &self,
        path: &std::path::Path,
    ) -> Option<(Option<mivon_ast::Design>, Option<mivon_ir::IrDesign>)> {
        let db = self.micd.as_ref()?;
        let pdb = db.precompiled_db.as_ref()?;

        // Hitung content hash file saat ini.
        let content_hash = std::fs::read(path)
            .map(|b| crate::cache::compute_checksum(&b))
            .ok()?;

        // Cari module yang source_file-nya path ini.
        let module = pdb.modules.values().find(|m| m.source_file == path)?;

        // Verifikasi fingerprint.
        if module.content_hash != content_hash || !module.verify_checksum() {
            return None;
        }

        // Deserialize AST bila ada.
        let ast = if !module.ast_bytes.is_empty() {
            bincode::deserialize(&module.ast_bytes).ok()
        } else {
            None
        };

        // Deserialize IR bila ada.
        let ir = if !module.ir_bytes.is_empty() {
            bincode::deserialize(&module.ir_bytes).ok()
        } else {
            None
        };

        Some((ast, ir))
    }

    /// Dependency graph file-level dari module index: file A bergantung pada
    /// file B bila module di A menginstansiasi / mengimpor module di B.
    pub fn compute_file_deps(&self) -> HashMap<PathBuf, Vec<PathBuf>> {
        let mut mod_to_file: HashMap<Symbol, PathBuf> = HashMap::new();
        for (name, kind, meta) in self.module_index.iter() {
            if kind == EntryKind::Module {
                mod_to_file.insert(name, meta.file.clone());
            }
        }
        let mut deps: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for (_name, kind, meta) in self.module_index.iter() {
            if kind != EntryKind::Module {
                continue;
            }
            let mut list: Vec<PathBuf> = Vec::new();
            for inst in &meta.instances {
                if let Some(f) = mod_to_file.get(inst) {
                    if *f != meta.file && !list.contains(f) {
                        list.push(f.clone());
                    }
                }
            }
            for (pkg, _item) in &meta.imports {
                if let Some(f) = mod_to_file.get(pkg) {
                    if *f != meta.file && !list.contains(f) {
                        list.push(f.clone());
                    }
                }
            }
            if !list.is_empty() {
                deps.entry(meta.file.clone()).or_default().extend(list);
            }
        }
        deps
    }

    /// Daftar file yang terdampak bila `changed` berubah (via graph.mdb).
    pub fn micd_affected(&mut self, changed: &[PathBuf]) -> Vec<PathBuf> {
        self.micd
            .as_mut()
            .map(|db| db.affected(changed))
            .unwrap_or_default()
    }

    /// Ringkasan MICD (untuk output CLI).
    pub fn micd_summary(&self) -> Option<String> {
        self.micd.as_ref().map(|db| db.summary())
    }

    /// Elaborate a module lazily (on-demand via HIR pipeline).
    /// Returns the elaborated HIR module if it exists and lazy mode is enabled.
    /// Falls back to `merged_design` for on-demand AST→HIR conversion on cache miss.
    pub fn elaborate_lazy_module(
        &self,
        name: Symbol,
    ) -> Option<std::sync::Arc<crate::hir::HirModule>> {
        if !self.config.use_lazy_elab {
            return None;
        }

        // 1. Check cache first
        if let Some(module) = self.lazy_elab.elaborate(name) {
            return Some(module);
        }

        // 2. Cache miss — try fallback to merged_design for AST→HIR conversion
        if let Some(ref design) = self.merged_design {
            if let Some(ast_module) = design.modules.iter().find(|m| m.name == name) {
                let signals: Vec<crate::hir::HirSignal> = ast_module
                    .ports
                    .iter()
                    .map(|p| {
                        let width = p
                            .range
                            .as_ref()
                            .map(|r| {
                                let lo = r.lsb;
                                let hi = r.msb;
                                hi.abs_diff(lo) + 1
                            })
                            .unwrap_or(1);
                        crate::hir::HirSignal {
                            name: p.name,
                            dtype: crate::hir::HirType::BitVec { width },
                            width,
                            is_input: matches!(
                                p.direction,
                                mivon_ast::types::PortDirection::Input
                                    | mivon_ast::types::PortDirection::Inout
                            ),
                            is_output: matches!(
                                p.direction,
                                mivon_ast::types::PortDirection::Output
                                    | mivon_ast::types::PortDirection::Inout
                            ),
                        }
                    })
                    .collect();

                // Extract params
                let params: Vec<crate::hir::HirParam> = ast_module
                    .params
                    .iter()
                    .map(|p| crate::hir::HirParam {
                        name: p.name,
                        dtype: crate::hir::HirType::BitVec { width: 1 },
                        default: None,
                        is_local: false,
                    })
                    .collect();

                return Some(self.lazy_elab.elaborate_with_data(
                    name,
                    params,
                    signals,
                    vec![], // stmts expanded on-demand
                ));
            }
        }

        None
    }

    /// Number of lazily-elaborated modules.
    pub fn lazy_elaborated_count(&self) -> usize {
        self.lazy_elab.len()
    }

    /// Check if a module has been lazily elaborated.
    pub fn is_lazy_elaborated(&self, name: Symbol) -> bool {
        self.lazy_elab.is_elaborated(name)
    }

    /// Invalidate all lazily-elaborated modules (e.g., on recompile).
    pub fn invalidate_lazy_elab(&mut self) {
        self.lazy_elab.invalidate_all();
    }

    /// Compile AND elaborate in one call.
    ///
    /// Combines `compile()` + `Elaborator::elaborate()` into a single pipeline step.
    /// When `use_lazy_elab` is set, pre-populates the LazyElaborator and stores
    /// the merged Design for on-demand module lookup via `elaborate_lazy_module()`.
    ///
    /// Returns (compiled Design, elaborated IrDesign, module index length).
    pub fn compile_and_elaborate(
        &mut self,
        top_name: Option<&str>,
    ) -> Result<(Design, mivon_ir::IrDesign, usize), SimError> {
        let (design, module_index) = self.compile()?;
        let index_len = module_index.len();

        // Ukur waktu elaborasi secara terpisah (untuk panel Pipeline GUI).
        let elab_start = Instant::now();

        // Create elaborator with source info for rich diagnostics
        let (source_lines, source_file) = self.source_info().unwrap_or_default();
        let mut elaborator = if source_lines.is_empty() {
            mivon_elaboration::Elaborator::new(design.clone())
        } else {
            mivon_elaboration::Elaborator::with_source(design.clone(), source_lines, source_file)
        };

        // Prime with session-level cache from previous compile
        if !self.cached_elab_modules.is_empty() {
            elaborator.set_cache(std::mem::take(&mut self.cached_elab_modules));
        }

        let ir_design = elaborator.elaborate(top_name, ElaborateMode::StrictSimulation)?;
        self.timing.elab_us = elab_start.elapsed().as_micros() as u64;

        // Store module cache back for next incremental compile
        // Simpan diagnostik elaborasi (dulu terbuang — warning elab hilang
        // di jalur tool; lihat field elab_diagnostics).
        self.elab_diagnostics = elaborator.flush_diagnostics();
        self.cached_elab_modules = elaborator.take_cache();

        // Cache IR design for access after compile. IR dengan array memori
        // flatten raksasa (est bit sinyal besar) TIDAK di-cache — clone
        // penuh = 183MB+ duplikat heap (heaptrack: IrDesign/IrModule clone
        // peak elab); store_elaborate_ir juga skip utk kasus yang sama.
        if !ir_est_bits_large(&ir_design) {
            self.cached_ir_design = Some(ir_design.clone());
        }

        Ok((design, ir_design, index_len))
    }

    /// Compile AND elaborate with specified mode.
    ///
    /// Similar to `compile_and_elaborate` but allows specifying the elaboration mode
    /// (StrictSimulation for simulation, AnalysisRecovery for analysis tools).
    pub fn compile_and_elaborate_with_mode(
        &mut self,
        top_name: Option<&str>,
        mode: ElaborateMode,
    ) -> Result<(Design, mivon_ir::IrDesign, usize), SimError> {
        let (design, module_index) = self.compile()?;
        let index_len = module_index.len();

        // Ukur waktu elaborasi secara terpisah (untuk panel Pipeline GUI).
        let elab_start = Instant::now();

        // Create elaborator with source info for rich diagnostics
        let (source_lines, source_file) = self.source_info().unwrap_or_default();
        let mut elaborator = if source_lines.is_empty() {
            mivon_elaboration::Elaborator::new(design.clone())
        } else {
            mivon_elaboration::Elaborator::with_source(design.clone(), source_lines, source_file)
        };

        // Prime with session-level cache from previous compile
        if !self.cached_elab_modules.is_empty() {
            elaborator.set_cache(std::mem::take(&mut self.cached_elab_modules));
        }

        let ir_design = elaborator.elaborate(top_name, mode)?;
        self.timing.elab_us = elab_start.elapsed().as_micros() as u64;

        // Store module cache back for next incremental compile
        // Simpan diagnostik elaborasi (dulu terbuang — warning elab hilang
        // di jalur tool; lihat field elab_diagnostics).
        self.elab_diagnostics = elaborator.flush_diagnostics();
        self.cached_elab_modules = elaborator.take_cache();

        // Cache IR design for access after compile. IR dengan array memori
        // flatten raksasa (est bit sinyal besar) TIDAK di-cache — clone
        // penuh = 183MB+ duplikat heap (heaptrack: IrDesign/IrModule clone
        // peak elab); store_elaborate_ir juga skip utk kasus yang sama.
        if !ir_est_bits_large(&ir_design) {
            self.cached_ir_design = Some(ir_design.clone());
        }

        Ok((design, ir_design, index_len))
    }

    /// Compile-only mode with lazy elaboration.
    ///
    /// Compiles the design and pre-populates the LazyElaborator, but
    /// skips full IR elaboration. Useful for IDE features, analysis,
    /// and quick syntax/port checks.
    ///
    /// Returns (compiled Design, elaborated HIR count, module index length).
    pub fn compile_lazy_only(&mut self) -> Result<(Design, usize, usize), SimError> {
        if !self.config.use_lazy_elab {
            return Err(SimError::with_diag(
                DiagCode::NotImplemented,
                "lazy mode not enabled (use --lazy)",
            ));
        }

        let (design, module_index) = self.compile()?;
        let index_len = module_index.len();
        let hir_count = self.lazy_elaborated_count();

        Ok((design, hir_count, index_len))
    }

    /// Get cached IR design (if previously elaborated).
    pub fn get_cached_ir(&self) -> Option<&mivon_ir::IrDesign> {
        self.cached_ir_design.as_ref()
    }

    /// Ambil diagnostics elaborasi terakhir (sekali baca — diambil lalu
    /// dikosongkan). Render dgn `TerminalEmitter` di tool yang butuh
    /// (mcov dsb) — warning elab dulu terbuang senyap di jalur tool.
    pub fn take_elab_diagnostics(&mut self) -> Vec<mivon_core::diagnostics::Diagnostic> {
        std::mem::take(&mut self.elab_diagnostics)
    }

    /// Get merged source info: (source_lines, first_source_file)
    /// Returns None if merged_source hasn't been populated (e.g., before first compile).
    /// Lepas cache parse-hit dari memori setelah `save_micd()` (run_fast jalur
    /// main.rs): prev_designs + prev_combined_sources + combined_parts +
    /// cache in-memory MICD (ast_cache/preproc_cache — sudah ditulis ke disk
    /// oleh save; ~1GB+ AST bytes pada design besar). TIDAK lagi dibutuhkan
    /// selama ELABORASI/SIMULASI berikutnya — bocor memori penyebab OOM utk
    /// design besar (OpenTitan): parse ~2.9GB → elaborasi/sim harus muat
    /// di sisa RAM mesin kecil.
    pub fn release_parse_cache(&mut self) {
        self.prev_designs.clear();
        self.prev_combined_sources.clear();
        self.combined_parts.lock().unwrap().clear();
        if let Some(db) = self.micd.as_mut() {
            db.release_memory_cache();
        }
    }

    pub fn source_info(&self) -> Option<(Vec<String>, String)> {
        self.merged_source.as_ref().map(|src| {
            let lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
            // Extract first source file from the `line directive
            let first_source = self
                .config
                .sources
                .first()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            (lines, first_source)
        })
    }
}

/// Discovery pass ringan: scan satu combined source (sudah preprocessed) dan
/// kumpulkan nama `class <name>` serta nama typedef (`typedef ... <name>;`).
/// Dipakai untuk seed Parser per-file dengan nama global (lintas file).
/// Nama class diambil dari ident pertama setelah `class`; nama typedef dari
/// ident terakhir sebelum `;` selama masih di dalam satu deklarasi typedef.
/// Over-collection aman: nama ekstra hanya membuat parser lebih condong
/// mem-parse `name var;` sebagai deklarasi (benar untuk tipe) — tidak pernah
/// mengubah instantiation module (yang tetap lewat sintaks `#( )` / `( )`).
fn discover_names_in_source(
    src: &str,
    classes: &mut HashSet<Symbol>,
    typedefs: &mut HashSet<Symbol>,
) {
    let mut lexer = FastLexer::new(src, "");
    let mut in_typedef = false;
    let mut last_ident: Option<Symbol> = None;
    // Depth kurung kurawal di dalam typedef: `typedef struct packed {...} name;`
    // punya SEMI internal (mis. deklarasi member), jadi nama typedef baru
    // muncul setelah `}` PENUTUP. Semi hanya dianggap terminasi saat depth==0.
    let mut brace_depth: usize = 0;
    loop {
        let (tok, _, _) = lexer.next_token();
        match tok {
            Token::Eof => break,
            Token::Class => {
                // LRM: `class <class_identifier> ...` — nama selalu ident
                // pertama setelah `class`. Ambil dan berhenti.
                loop {
                    let (t, _, _) = lexer.next_token();
                    match t {
                        Token::Eof => break,
                        Token::Ident(n) => {
                            classes.insert(n);
                            break;
                        }
                        _ => continue,
                    }
                }
            }
            Token::Typedef => {
                in_typedef = true;
                last_ident = None;
                brace_depth = 0;
            }
            Token::LBrace => {
                if in_typedef {
                    brace_depth += 1;
                }
            }
            Token::RBrace => {
                if in_typedef && brace_depth > 0 {
                    brace_depth -= 1;
                }
            }
            Token::Ident(n) => {
                if in_typedef && brace_depth == 0 {
                    last_ident = Some(n);
                }
            }
            Token::Semi if in_typedef && brace_depth == 0 => {
                if let Some(n) = last_ident {
                    typedefs.insert(n);
                }
                in_typedef = false;
                last_ident = None;
            }
            _ => {}
        }
    }
}

/// IR dengan estimasi bit sinyal raksasa (array memori flatten — cache
/// AetherX 1.5e9 bit). Dipakai SKIP cache IR penuh (clone untuk
/// `cached_ir_design` & serialize `store_elaborate_ir`): clone/serialize
/// 1GB+ utk desain besar = duplikat heap & OOM di mesin kecil; warm run
/// mengelaborasi ulang jauh lebih murah. IR normal tetap di-cache.
fn ir_est_bits_large(ir: &mivon_ir::IrDesign) -> bool {
    const IR_CACHE_SKIP_BITS: u64 = 8 * 1024 * 1024; // 8M bit sinyal
    let est_bits: u64 = ir
        .modules
        .values()
        .flat_map(|m| m.signals.iter())
        .map(|s| s.width as u64)
        .sum();
    est_bits > IR_CACHE_SKIP_BITS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve path relatif ke root workspace (cwd test = direktori crate).
    fn root_rel(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .join(rel)
    }

    #[test]
    fn test_compile_session_basic() {
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let (design, index) = session.compile().unwrap();
        assert!(
            !design.modules.is_empty(),
            "should have at least one module"
        );
        assert!(!index.is_empty(), "should have indexed at least one module");
    }

    #[test]
    fn test_compile_session_empty() {
        let config = SessionConfig {
            sources: vec![],
            auto_incdirs: false,
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let result = session.compile();
        assert!(result.is_err(), "empty session should error");
    }

    #[test]
    fn test_compile_session_timing() {
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let _ = session.compile().unwrap();
        assert!(
            session.timing.preprocess_us + session.timing.lex_us + session.timing.parse_us > 0,
            "at least one phase should have timing > 0"
        );
    }

    #[test]
    fn test_multi_file_snippet_resolves_correct_file_and_line() {
        // Regression: di jalur CompileSession (--filelist), posisi AST harus global
        // (base offset per-file) agar snippet source menunjuk file & baris yang benar
        // untuk file non-pertama. Bug lama: error di b.sv menunjuk a.sv / baris kosong.
        let dir = std::env::temp_dir().join(format!("mivon_snippet_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a_path = dir.join("a.sv");
        let b_path = dir.join("b.sv");
        std::fs::write(&a_path, "module file_a;\n  file_b u_b ();\nendmodule\n").unwrap();
        std::fs::write(
            &b_path,
            "module file_b;\n  logic clk;\n  missing_module u_inst ();\nendmodule\n",
        )
        .unwrap();

        let config = SessionConfig {
            sources: vec![a_path, b_path],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let err = match session.compile_and_elaborate(Some("file_a")) {
            Err(e) => e,
            Ok(_) => panic!("expected elaboration error for missing module"),
        };
        let diag = err.to_diagnostic();
        let snippet = diag
            .source_snippet
            .expect("error should carry a source snippet");
        assert!(
            snippet.file.ends_with("b.sv"),
            "expected file b.sv, got {}",
            snippet.file
        );
        assert_eq!(
            snippet.line, 3,
            "expected file-relative line 3, got {}",
            snippet.line
        );
        assert!(
            snippet.source_line.contains("missing_module"),
            "snippet content mismatch: {:?}",
            snippet.source_line
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compile_session_cache_integration() {
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let _ = session.compile().unwrap();

        // Cache should have entries: `compile()` mendaftarkan checksum tiap
        // file ke prev_checksums (pipeline incremental). `stats.ast_entries`
        // (CacheManager in-memory AST) TIDAK terisi di jalur compile() — AST
        // disimpan via MICD (db.cache_ast) saat save_micd, bukan CacheManager
        // (sebelumnya assertion `|| total_invalidations >= 0` selalu true →
        // e5f7489 menyederhanakannya menjadi `ast_entries > 0` → fail palsu).
        let stats = session.cache_stats();
        assert!(
            stats.ast_entries > 0 || !session.prev_checksums.is_empty(),
            "cache pipeline tidak aktif: ast_entries={} prev_checksums={}",
            stats.ast_entries,
            session.prev_checksums.len()
        );
    }

    #[test]
    fn test_incremental_first_compile_full() {
        // First compile: semua file harus diproses (cached_files = 0)
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let (design, _) = session.compile().unwrap();
        assert!(!design.modules.is_empty());
        // First compile: processed = 1, cached = 0
        assert_eq!(session.timing.processed_files, 1);
        assert_eq!(session.timing.cached_files, 0);
        assert!(session
            .prev_checksums
            .contains_key(root_rel("test/counter.sv").as_path()));
        assert!(session
            .prev_designs
            .contains_key(root_rel("test/counter.sv").as_path()));
    }

    #[test]
    fn test_incremental_second_compile_no_changes() {
        // Second compile (no changes): semua file harus di-cache
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let _ = session.compile().unwrap(); // first compile

        // Reset timing
        session.timing = SessionTiming::default();
        let (design, _) = session.compile().unwrap(); // second compile (should use cache)
        assert!(!design.modules.is_empty());
        // Second compile counts both results as processed (timing quirk)
        // but design should be valid
        assert_eq!(session.timing.processed_files, 1);
    }

    #[test]
    fn test_incremental_compile_incremental_method() {
        // compile_incremental dengan force_changed memaksa re-process
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let _ = session.compile().unwrap(); // first compile

        session.timing = SessionTiming::default();
        // Force file as changed
        let changed = vec![root_rel("test/counter.sv")];
        let (design, _) = session.compile_incremental(&changed).unwrap();
        assert!(!design.modules.is_empty());
        // Should have re-processed the forced-changed file
        assert_eq!(session.timing.processed_files, 1);
    }

    #[test]
    fn test_incremental_two_files() {
        // Test with two files: modify one, verify design is still valid
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("mivon_inc_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create two files
        let f1 = dir.join("mod_a.sv");
        let f2 = dir.join("mod_b.sv");
        {
            let mut f = std::fs::File::create(&f1).unwrap();
            writeln!(f, "module mod_a(input clk, output reg [3:0] q);").unwrap();
            writeln!(f, "    always_ff @(posedge clk) q <= q + 4'h1;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }
        {
            let mut f = std::fs::File::create(&f2).unwrap();
            writeln!(f, "module mod_b(input clk, output reg [7:0] q);").unwrap();
            writeln!(f, "    always_ff @(posedge clk) q <= q + 8'h1;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }

        let config = SessionConfig {
            sources: vec![f1.clone(), f2.clone()],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let _ = session.compile().unwrap(); // first compile (both processed)

        // Modify mod_b.sv
        {
            let mut f = std::fs::File::create(&f2).unwrap();
            writeln!(f, "module mod_b(input clk, output reg [7:0] q);").unwrap();
            writeln!(f, "    always_ff @(posedge clk) q <= q + 8'h2;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }

        session.timing = SessionTiming::default();
        let (design, _) = session.compile().unwrap(); // third compile (mod_b changed)
        assert!(!design.modules.is_empty());
        // Design has modules from both files
        assert!(design.modules.iter().any(|m| m.name == "mod_a"));
        assert!(design.modules.iter().any(|m| m.name == "mod_b"));

        // Now repeat without changes: all cached, design still valid
        session.timing = SessionTiming::default();
        let (design2, _) = session.compile().unwrap();
        assert!(design2.modules.iter().any(|m| m.name == "mod_a"));
        assert!(design2.modules.iter().any(|m| m.name == "mod_b"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_incremental_checksum_persistence() {
        // Verify metadata fingerprints are persisted between compiles
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let _ = session.compile().unwrap();

        let path = root_rel("test/counter.sv");
        assert!(session.prev_checksums.contains_key(&path));
        let fp = session.prev_checksums.get(&path).copied().unwrap();
        assert_ne!(fp, 0, "metadata fingerprint should be non-zero");

        // Verify the metadata fingerprint matches a re-computed one
        let recomputed = session.metadata_fingerprint(&path);
        assert_eq!(
            fp, recomputed,
            "metadata fingerprint should be reproducible"
        );
    }

    #[test]
    fn test_incremental_detect_changed_no_prev() {
        // Empty prev_checksums should return ALL files as changed
        let session = CompileSession::new(SessionConfig::default());
        let files = vec![root_rel("test/counter.sv"), root_rel("test/tb_counter.sv")];
        let changed = session.detect_changed(&files);
        assert_eq!(
            changed.len(),
            2,
            "all files should be 'changed' on first run"
        );
    }

    #[test]
    fn test_micd_persists_across_sessions() {
        // MICD: compile lintas sesi (proses) — file tidak berubah di-restore,
        // parse di-skip; file berubah diproses ulang.
        use crate::micd::MicdDatabase;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("mivon_micd_compile_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_root = dir.join("db");
        let f1 = dir.join("mod_a.sv");
        let f2 = dir.join("mod_b.sv");
        {
            let mut f = std::fs::File::create(&f1).unwrap();
            writeln!(f, "module mod_a(input clk, output reg [3:0] q);").unwrap();
            writeln!(f, "    always_ff @(posedge clk) q <= q + 4'h1;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }
        {
            let mut f = std::fs::File::create(&f2).unwrap();
            writeln!(f, "module mod_b(input clk, output reg [7:0] q);").unwrap();
            writeln!(f, "    always_ff @(posedge clk) q <= q + 8'h1;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }
        let sources = vec![f1.clone(), f2.clone()];

        // Sesi 1: cold compile → semua diproses.
        {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                ..Default::default()
            });
            let restored = s.attach_micd(MicdDatabase::open(&db_root));
            assert_eq!(restored, 0, "cold compile: tidak ada yang di-restore");
            let (design, _) = s.compile().unwrap();
            assert!(design.modules.iter().any(|m| m.name == "mod_a"));
            assert!(design.modules.iter().any(|m| m.name == "mod_b"));
            let stats = s.save_micd().unwrap().expect("database terpasang");
            assert_eq!(stats.changed_files, 2, "cold compile: semua file berubah");
            // Layout Opsi B: payload AST sebagai objek CAS di objects/default/.
            assert!(
                db_root.join("objects").join("default").exists(),
                "database harus tersimpan"
            );
        }

        // Sesi 2: tanpa perubahan → semua file di-restore (parse di-skip).
        {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                ..Default::default()
            });
            let restored = s.attach_micd(MicdDatabase::open(&db_root));
            assert_eq!(restored, 2, "semua AST harus di-restore dari MICD");
            let (design, _) = s.compile().unwrap();
            assert!(design.modules.iter().any(|m| m.name == "mod_a"));
            assert!(design.modules.iter().any(|m| m.name == "mod_b"));
            let stats = s.save_micd().unwrap().expect("database terpasang");
            assert_eq!(stats.changed_files, 0, "warm compile: tidak ada perubahan");
        }

        // Sesi 3: mod_b.sv berubah → hanya mod_b diproses, mod_a di-restore.
        {
            let mut f = std::fs::File::create(&f2).unwrap();
            writeln!(f, "module mod_b(input clk, output reg [7:0] q);").unwrap();
            writeln!(f, "    always_ff @(posedge clk) q <= q + 8'h2;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }
        {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                ..Default::default()
            });
            let restored = s.attach_micd(MicdDatabase::open(&db_root));
            assert_eq!(restored, 1, "mod_a di-restore, mod_b berubah");
            let (design, _) = s.compile().unwrap();
            assert!(design.modules.iter().any(|m| m.name == "mod_a"));
            assert!(design.modules.iter().any(|m| m.name == "mod_b"));
            let stats = s.save_micd().unwrap().expect("database terpasang");
            assert_eq!(stats.changed_files, 1, "hanya mod_b yang berubah");
        }

        // Affected set: mod_a tidak tergantung mod_b → tidak terdampak.
        {
            let mut db = MicdDatabase::open(&db_root);
            let affected = db.affected(std::slice::from_ref(&f2));
            assert!(!affected.contains(&f1), "mod_a tidak tergantung mod_b");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_micd_include_change_invalidates_cache() {
        // Correctness: bila header (`include) berubah, file yang meng-include
        // TIDAK boleh di-restore (preprocessed output meng-embed konten lama).
        use crate::micd::MicdDatabase;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("mivon_micd_inc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_root = dir.join("db");
        let src = dir.join("top.sv");
        let hdr = dir.join("defs.svh");
        {
            let mut f = std::fs::File::create(&hdr).unwrap();
            writeln!(f, "`define WIDTH 4").unwrap();
        }
        {
            let mut f = std::fs::File::create(&src).unwrap();
            writeln!(f, "`include \"defs.svh\"").unwrap();
            writeln!(f, "module top(input clk, output reg [`WIDTH-1:0] q);").unwrap();
            writeln!(f, "    always_ff @(posedge clk) q <= q + `WIDTH'd1;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }
        let sources = vec![src.clone()];
        let incdirs = vec![dir.clone()];

        // Sesi 1: cold compile.
        {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                incdirs: incdirs.clone(),
                ..Default::default()
            });
            assert_eq!(s.attach_micd(MicdDatabase::open(&db_root)), 0);
            let (design, _) = s.compile().unwrap();
            assert_eq!(design.modules.len(), 1);
            s.save_micd().unwrap();
        }

        // Sesi 2: tanpa perubahan → restore.
        {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                incdirs: incdirs.clone(),
                ..Default::default()
            });
            assert_eq!(s.attach_micd(MicdDatabase::open(&db_root)), 1);
            let (design, _) = s.compile().unwrap();
            assert_eq!(design.modules.len(), 1);
            s.save_micd().unwrap();
        }

        // Ubah header include (top.sv TIDAK berubah).
        {
            let mut f = std::fs::File::create(&hdr).unwrap();
            writeln!(f, "`define WIDTH 8").unwrap();
        }

        // Sesi 3: top.sv content hash sama, tapi include berubah → TIDAK
        // boleh di-restore (harus di-reprocess agar width benar).
        {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                incdirs: incdirs.clone(),
                ..Default::default()
            });
            let restored = s.attach_micd(MicdDatabase::open(&db_root));
            assert_eq!(
                restored, 0,
                "file dengan include yang berubah tidak boleh di-restore"
            );
            let (design, _) = s.compile().unwrap();
            assert_eq!(design.modules.len(), 1);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_micd_parse_errors_survive_warm_run() {
        // BUG #2 (CRITICAL): file yang di-restore dari MICD harus tetap
        // menyumbang diagnostik parse. Dulu cache-hit mengembalikan
        // `Vec::new()` → run hangat melaporkan 0 error dan desain yang gagal
        // parse lolos ke simulasi (silent miscompilation). Fix: file ber-
        // diagnostik TIDAK di-restore (di-parse ulang tiap run) → jumlah error
        // deterministik: dingin == hangat. Sesi 1 juga menguji dedup dua-pass
        // (BUG #1): satu error nyata ≠ 2-3× lipatan.
        use crate::micd::MicdDatabase;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("mivon_micd_perr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_root = dir.join("db");
        let ok = dir.join("ok.sv");
        let bad = dir.join("bad.sv");
        {
            let mut f = std::fs::File::create(&ok).unwrap();
            writeln!(f, "module ok(input clk);").unwrap();
            writeln!(f, "endmodule").unwrap();
        }
        {
            // Class nyata (tolak fragment-downgrade) + covergroup dgn body
            // RUSAK (`coverpoint ;`) → 1 error nyata yang diemiten pass-1
            // dan pass-2 parser (terdedup) — sumber error tidak lagi
            // memakai formal-args (kini didukung).
            let mut f = std::fs::File::create(&bad).unwrap();
            writeln!(f, "class dummy_cov;").unwrap();
            writeln!(f, "    int unsigned m;").unwrap();
            writeln!(f, "endclass").unwrap();
            writeln!(f, "covergroup handshake_cg;").unwrap();
            writeln!(f, "    cp: coverpoint ;").unwrap();
            writeln!(f, "endgroup").unwrap();
        }
        let sources = vec![ok.clone(), bad.clone()];
        let count_err = |s: &CompileSession| {
            s.parse_errors
                .iter()
                .filter(|d| d.is_error())
                .count()
        };

        // Sesi 1: cold — error dilaporkan tepat 1× (dedup dua-pass).
        {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                ..Default::default()
            });
            assert_eq!(s.attach_micd(MicdDatabase::open(&db_root)), 0);
            s.compile().unwrap();
            assert_eq!(count_err(&s), 1, "cold: 1 masalah nyata, bukan 2-3×");
            s.save_micd().unwrap().expect("database terpasang");
        }

        // Sesi 2: warm — file bersih di-restore, file error di-parse ulang →
        // error MASIH dilaporkan (bukan 0) dan restore tidak menelan-nya.
        for round in 1..=2 {
            let mut s = CompileSession::new(SessionConfig {
                sources: sources.clone(),
                ..Default::default()
            });
            let restored = s.attach_micd(MicdDatabase::open(&db_root));
            assert_eq!(
                restored, 1,
                "warm {}: hanya file bersih yang di-restore",
                round
            );
            s.compile().unwrap();
            assert_eq!(
                count_err(&s),
                1,
                "warm {}: error parse HARUS tetap terlapor (BUG #2)",
                round
            );
            s.save_micd().unwrap().expect("database terpasang");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cross_file_include_diag_dedup() {
        // BUG #1b (fake bug — pelaporan): file S terdaftar MANDIRI di
        // filelist DAN di-`include` oleh file P dalam build yang sama (pola
        // OpenTitan: `*_pkg.sv` `include "*_cov.sv"`). Kedua parse melapor
        // di lokasi display yang SAMA (line directive `include`) → satu
        // konstruk terhitung 2×. Dedup lintas file di compile() wajib
        // menyisakan satu; diagnostik per file tetap utuh untuk LSP/MICD.
        use crate::micd::MicdDatabase;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("mivon_xfile_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_root = dir.join("db");
        let cg = dir.join("cg_only.sv");
        let pkg = dir.join("inc_pkg.sv");
        {
            // Class nyata → standalone parse TIDAK di-downgrade jadi
            // fragment-warning; salinan di dalam package juga Error → level
            // sama, cocok untuk dedup. Body covergroup rusak (`coverpoint ;`)
            // = sumber error (formal args kini didukung, tak dipakai).
            let mut f = std::fs::File::create(&cg).unwrap();
            writeln!(f, "class dummy_cov;").unwrap();
            writeln!(f, "    int unsigned m;").unwrap();
            writeln!(f, "endclass").unwrap();
            writeln!(f, "covergroup handshake_cg;").unwrap();
            writeln!(f, "    cp: coverpoint ;").unwrap();
            writeln!(f, "endgroup").unwrap();
        }
        {
            let mut f = std::fs::File::create(&pkg).unwrap();
            writeln!(f, "package inc_pkg;").unwrap();
            writeln!(f, "`include \"cg_only.sv\"").unwrap();
            writeln!(f, "endpackage").unwrap();
        }
        let sources = vec![cg.clone(), pkg.clone()];

        let mut s = CompileSession::new(SessionConfig {
            sources: sources.clone(),
            ..Default::default()
        });
        s.attach_micd(MicdDatabase::open(&db_root));
        s.compile().unwrap();
        let hits: Vec<String> = s
            .parse_errors
            .iter()
            .filter(|d| d.is_error())
            .filter(|d| {
                d.source_snippet
                    .as_ref()
                    .map(|ss| ss.file.ends_with("cg_only.sv"))
                    .unwrap_or(false)
            })
            .map(|d| {
                d.source_snippet
                    .as_ref()
                    .map(|ss| format!("{}:{}:{}", ss.file, ss.line, ss.col))
                    .unwrap_or_else(|| "<no loc>".into())
            })
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "1 konstruk, 2 jalur parse (mandiri + include) → 1 laporan; dapat {}: {:?}",
            hits.len(),
            hits
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_micd_flags_change_invalidates_all() {
        // Correctness: -D define berubah → preprocessed output meng-embed
        // makro lama → SEMUA file harus di-reprocess (jangan ada restore).
        use crate::micd::MicdDatabase;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("mivon_micd_flags_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_root = dir.join("db");
        let src = dir.join("top.sv");
        {
            let mut f = std::fs::File::create(&src).unwrap();
            writeln!(f, "module top(output logic [3:0] q);").unwrap();
            writeln!(f, "    assign q = `WIDTH'd5;").unwrap();
            writeln!(f, "endmodule").unwrap();
        }
        let sources = vec![src.clone()];
        let mk = |defines: Vec<(String, String)>| SessionConfig {
            sources: sources.clone(),
            defines,
            ..Default::default()
        };

        // Sesi 1: WIDTH=4.
        {
            let mut s = CompileSession::new(mk(vec![("WIDTH".into(), "4".into())]));
            assert_eq!(s.attach_micd(MicdDatabase::open(&db_root)), 0);
            s.compile().unwrap();
            s.save_micd().unwrap();
        }
        // Sesi 2: define sama → restore.
        {
            let mut s = CompileSession::new(mk(vec![("WIDTH".into(), "4".into())]));
            assert_eq!(s.attach_micd(MicdDatabase::open(&db_root)), 1);
            s.compile().unwrap();
            s.save_micd().unwrap();
        }
        // Sesi 3: WIDTH=8 (flags berubah) → TIDAK boleh restore.
        {
            let mut s = CompileSession::new(mk(vec![("WIDTH".into(), "8".into())]));
            assert_eq!(
                s.attach_micd(MicdDatabase::open(&db_root)),
                0,
                "flags berubah → semua harus di-reprocess"
            );
            s.compile().unwrap();
            s.save_micd().unwrap();
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_incremental_dep_graph_propagation_via_topo() {
        // Verify dependency graph correctly tracks dependencies via topological order
        use crate::scheduler::dag::DependencyGraph;
        use crate::scheduler::work_stealing::Task;

        let graph = DependencyGraph::new();

        // Create nodes: top → mid → leaf (top depends on mid, mid depends on leaf)
        let leaf = graph.add_node(Task::ParseFile("leaf.sv".to_string()));
        let mid = graph.add_node(Task::ParseFile("mid.sv".to_string()));
        let top = graph.add_node(Task::ParseFile("top.sv".to_string()));

        graph.add_edge(top, mid);
        graph.add_edge(mid, leaf);

        // Verify topological order: leaf before mid before top
        let order = graph.topo_order();
        let pos_leaf = order.iter().position(|&x| x == leaf).unwrap();
        let pos_mid = order.iter().position(|&x| x == mid).unwrap();
        let pos_top = order.iter().position(|&x| x == top).unwrap();
        assert!(
            pos_leaf < pos_mid,
            "leaf should come before mid in topo order"
        );
        assert!(
            pos_mid < pos_top,
            "mid should come before top in topo order"
        );

        // Verify initial ready set: leaf should be ready (no deps)
        let ready = graph.initial_ready();
        assert!(ready.contains(&leaf), "leaf should be in initial ready set");
        assert!(!ready.contains(&top), "top should not be ready yet");
    }

    // ── F10: inline_sources (buffer transpile `.mv`) ──

    #[test]
    fn test_inline_sources_compiles_from_buffer() {
        // Path yang TIDAK ada di disk namun terdaftar di inline_sources harus
        // di-compile dari buffer — dipakai tools untuk file `.mv` (F10).
        let fake = PathBuf::from("nonexistent_inline_f10.sv");
        let src = "module inline_f10;\n  logic clk;\n  always #5 clk = ~clk;\nendmodule\n";
        let mut inline = HashMap::new();
        inline.insert(fake.clone(), src.as_bytes().to_vec());
        let config = SessionConfig {
            sources: vec![fake.clone()],
            inline_sources: inline,
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let (design, _index) = session.compile().expect("inline source harus compile");
        assert!(
            design
                .modules
                .iter()
                .any(|m| m.name.as_str() == "inline_f10"),
            "module dari buffer inline harus ter-compile"
        );
        // Tanpa inline_sources → error (file tidak ada di disk).
        let config2 = SessionConfig {
            sources: vec![fake.clone()],
            ..Default::default()
        };
        let mut session2 = CompileSession::new(config2);
        assert!(
            session2.compile().is_err(),
            "path non-existen tanpa inline harus error"
        );
    }

    #[test]
    fn test_inline_sources_mixed_with_disk() {
        // Campuran: satu file dari disk + satu file dari buffer inline (pola
        // `types.mv` → buffer, `counter.sv` → disk, dipakai tool multi-file).
        let dir = std::env::temp_dir().join(format!("mivon_inline_mix_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let disk_path = dir.join("disk_mod.sv");
        std::fs::write(
            &disk_path,
            "module disk_mod;\n  logic x;\n  always_comb x = 1'b0;\nendmodule\n",
        )
        .unwrap();
        let inline_path = PathBuf::from("inline_peer.sv");
        let src = "module inline_peer;\n  logic y;\n  always_comb y = 1'b1;\nendmodule\n";
        let mut inline = HashMap::new();
        inline.insert(inline_path.clone(), src.as_bytes().to_vec());
        let config = SessionConfig {
            sources: vec![disk_path, inline_path],
            inline_sources: inline,
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let (design, _index) = session
            .compile()
            .expect("campuran disk+inline harus compile");
        let names: Vec<&str> = design.modules.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"disk_mod"));
        assert!(names.contains(&"inline_peer"));
    }
}
