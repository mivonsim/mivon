//! Mivon API — lapisan public API RTL Simulator untuk SystemVerilog.
//!
//! Hasil akhir migrasi monorepo: `src/` di package `mivon` hanya berisi
//! `main.rs` + `cli.rs` (binary-only). Seluruh logika pindah ke crates/,
//! dan API publik (compile_str/simulate_str/… + re-export mivon_*) hidup
//! di crate `mivon-api` ini. `mivon::*` di main.rs kini = `mivon_api::*`.

// Allow large Result Err variant for SimError (intentional — Diagnostic contains spans/files)
#![allow(clippy::result_large_err, clippy::type_complexity)]

// ── VPI (Verilog Procedural Interface) — pindah ke mivon-simulator (crates/) ──
// ── LSP (Language Server Protocol) — pindah ke mivon-env (crates/) ──
// `mivon::lsp::*` di main.rs tetap valid via re-export.
#[cfg(feature = "lsp")]
pub use mivon_env::lsp;

// ── Formal Verification Engine — pindah ke mivon-formal (crates/) ──
// BMC + Z3 SMT + assertion checking. `mivon::formal::*` di main.rs tetap valid.
#[cfg(feature = "formal")]
pub use mivon_formal as formal;

// ── Core Infrastructure — pindah ke workspace crate `mivon-core` (crates/) ──
// intern (Symbol/Span), arena, error, diagnostics, config, animasi, dan tipe
// nilai logika (LogicVal/LogicVec) kini hidup di crates/mivon-core. Modul
// ini tetap diakses lintas crate via `mivon_core::...`; re-export API inti
// ada di bagian bawah lib.rs.

// ── Enterprise Context Architecture (doc/env.md) — GlobalEnv + Context ──
// pindah ke mivon-env (crates/) — `mivon::env::*` di main.rs tetap valid.
pub use mivon_env::env;
use std::sync::atomic::{AtomicUsize, Ordering};

// ── Legacy Modules ──
// ast → mivon-ast, ir → mivon-ir, parser → mivon-parser, elaboration →
// mivon-elaboration, compiler → mivon-compiler, simulator/waveform/scheduler/
// debugger/vpi → mivon-simulator (crates/) — lihat migrasi monorepo.
pub use mivon_simulator::{debugger, foreign, pli, scheduler, simulator, vhpi, vpi, waveform};

// ── Mivon HDL (.mv) — bahasa baru Mivon, transpile ke SystemVerilog (MIVON-HDL.md) ──
// pindah ke mivon-mv (crates/) — lihat migrasi monorepo.
pub use mivon_mv as mv;

// ── Emulator (EMULATOR.md) — Hardware-Software Emulator; R0: MHIR ──
// `mivon::emu::mhir::*` di main.rs tetap valid via re-export.
pub use mivon_emu as emu;

// ── New Module Structure ──
// frontend/cache/micd/hir/mir/profiling + scheduler(task cluster) →
// mivon-compiler (crates/) — lihat migrasi monorepo.
// scheduler (simulasi cluster: sim_dag/clock_domain/cdc) → mivon-simulator.

// ── Plugin System — pindah ke mivon-env (crates/) bersama env ──
// (plugin hanya dipakai oleh env/plugins; `mivon::plugin` tetap tersedia)
pub use mivon_env::plugin;

// ── CLI Tools (tools.md) — pindah ke mivon-tools (crates/) ──
// 10 tool terminal (minspect/mlint/melab/msim/mcov/mwave/mfmt/mprof/mcheck/
// mbench) kini di crate mivon-tools; `mivon::tools::*` di main.rs tetap valid.
pub use mivon_tools as tools;

// ── Native GUI (egui) — pindah ke mivon-gui (crates/) ──
// `mivon::gui::run()` di main.rs + bin/mivon_gui.rs tetap valid via re-export.
#[cfg(feature = "gui")]
pub use mivon_gui as gui;

pub use mivon_compiler::frontend::compile_session::{CompileSession, SessionConfig};
pub use mivon_compiler::frontend::discovery::FileDiscovery;
pub use mivon_core::arena::{BumpArena, TypedArena};
pub use mivon_core::diagnostics::{
    DiagCode, DiagLevel, DiagSink, Diagnostic, RuntimeContext, SourceSnippet,
};
pub use mivon_core::error::SimError;
pub use mivon_core::intern::{init_string_table, Span, Symbol};
use mivon_elaboration::ElaborateMode;

use mivon_parser::lexer::Lexer;
use mivon_parser::preprocessor::Preprocessor;
use mivon_parser::Parser;
use std::fs;
use std::path::{Path, PathBuf};

/// Compare two ASTs for regression testing. Returns list of structural differences.
pub fn compare_asts(design_a: &mivon_ir::IrDesign, design_b: &mivon_ir::IrDesign) -> Vec<String> {
    let mut diffs = Vec::new();

    // Compare module count
    if design_a.modules.len() != design_b.modules.len() {
        diffs.push(format!(
            "module count: {} vs {}",
            design_a.modules.len(),
            design_b.modules.len()
        ));
    }

    // Compare signal count
    if design_a.top.signals.len() != design_b.top.signals.len() {
        diffs.push(format!(
            "top signal count: {} vs {}",
            design_a.top.signals.len(),
            design_b.top.signals.len()
        ));
    }

    // Compare process count
    if design_a.top.processes.len() != design_b.top.processes.len() {
        diffs.push(format!(
            "process count: {} vs {}",
            design_a.top.processes.len(),
            design_b.top.processes.len()
        ));
    }

    // Bandingkan signal per-NAMA (bukan pairwise index): index-pairing
    // menyesatkan saat urutan/set signal dua design beda — diff palsu dan
    // ASIMETRIS A→B != B→A (ditemukan mivon-fuzz target astdiff). Cocokkan
    // by nama, laporkan only-in-A/B eksplisit. Sort by nama → deterministik.
    {
        use std::collections::BTreeMap;
        let ma = design_a
            .top
            .signals
            .iter()
            .map(|s| (s.name.to_string(), s))
            .collect::<BTreeMap<String, &mivon_ir::SignalInfo>>();
        let mb = design_b
            .top
            .signals
            .iter()
            .map(|s| (s.name.to_string(), s))
            .collect::<BTreeMap<String, &mivon_ir::SignalInfo>>();
        let names: std::collections::BTreeSet<String> =
            ma.keys().chain(mb.keys()).cloned().collect();
        for name in &names {
            match (ma.get(name), mb.get(name)) {
                (Some(sa), Some(sb)) => {
                    if sa.width != sb.width {
                        diffs.push(format!(
                            "signal '{}' width: {} vs {}",
                            name, sa.width, sb.width
                        ));
                    }
                    if sa.is_signed != sb.is_signed {
                        diffs.push(format!(
                            "signal '{}' signed: {} vs {}",
                            name, sa.is_signed, sb.is_signed
                        ));
                    }
                }
                (Some(_), None) => diffs.push(format!("signal '{}' only in A", name)),
                (None, Some(_)) => diffs.push(format!("signal '{}' only in B", name)),
                _ => {}
            }
        }
    }

    // Compare class definitions
    if design_a.classes.len() != design_b.classes.len() {
        diffs.push(format!(
            "class count: {} vs {}",
            design_a.classes.len(),
            design_b.classes.len()
        ));
    }

    // Compare covergroups
    if design_a.covergroups.len() != design_b.covergroups.len() {
        diffs.push(format!(
            "covergroup count: {} vs {}",
            design_a.covergroups.len(),
            design_b.covergroups.len()
        ));
    }

    diffs
}

/// Read a .mivon project file and return list of .sv file paths
/// Paths in .mivon are resolved relative to the .mivon file's directory
pub fn read_project_file(path: &str) -> Result<Vec<String>, SimError> {
    let content = fs::read_to_string(path).map_err(|e| {
        SimError::with_diag(
            DiagCode::InvalidSyntax,
            format!("cannot read '{}': {}", path, e),
        )
    })?;
    let base = Path::new(path).parent().unwrap_or(Path::new("."));
    // Section `[...]` di project file .mivon ([foreign] untuk library
    // VHPI/PLI/DPI, dan header lain) — header DAN isi section (baris
    // `key = value`) bukan file .sv. Konvensi: daftar file dulu, section di
    // akhir — setelah header section pertama, sisanya di-skip. [foreign]
    // di-parse terpisah oleh read_project_with_foreign.
    let mut in_section = false;
    let mut skipped_templates = 0usize;
    let files: Vec<String> = content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| {
            if l.starts_with('[') && l.ends_with(']') {
                in_section = true;
                return false;
            }
            !in_section
        })
        .filter(|l| {
            let p = base.join(l);
            if mivon_core::template::is_template_source(&p) {
                skipped_templates += 1;
                false
            } else {
                true
            }
        })
        .map(|l| {
            let p = base.join(l);
            p.to_string_lossy().to_string()
        })
        .collect();
    // Dedup inode: file yang sama bisa terdaftar DUA KALI lewat path symlink
    // (mis. project pribadi AetherX: `rtl/core/core → rtl/core`). Tanpa ini
    // file digabung dua kali → module duplikat + MICD entry kembar. Path
    // PERTAMA dipakai apa adanya (display konsisten), sisanya dibuang.
    let files: Vec<String> = {
        let mut seen_inode: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let mut dedup: Vec<String> = Vec::with_capacity(files.len());
        for f in files {
            let canon = std::path::Path::new(&f)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(&f));
            if seen_inode.insert(canon) {
                dedup.push(f);
            }
        }
        dedup
    };
    if skipped_templates > 0 {
        eprintln!(
            "warning: filelist '{}': melewati {} file template (*.tpl*) — bukan SystemVerilog",
            path, skipped_templates
        );
    }
    if files.is_empty() {
        return Err(SimError::with_diag(
            DiagCode::ModuleNotFound,
            format!("no .sv files listed in '{}'", path),
        ));
    }
    Ok(files)
}

/// Isi file project .mivon — daftar file .sv + bagian `[foreign]`
/// (arsitektur masukan user poin 9):
///
/// ```text
/// tb_top.sv
/// rtl/counter.sv
///
/// [foreign]
/// vhpi = ["libvhpi_test.so"]
/// pli  = ["libpli_test.so"]
/// dpi  = ["libdpi_test.so"]
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectFile {
    /// File .sv (path sudah relatif ke direktori .mivon).
    pub files: Vec<String>,
    /// Library VHPI (IEEE 1076-2008) dari `[foreign] vhpi = [...]`.
    pub vhpi_libs: Vec<String>,
    /// Library PLI (IEEE 1364) dari `[foreign] pli = [...]`.
    pub pli_libs: Vec<String>,
    /// Library DPI (IEEE 1800 §35) dari `[foreign] dpi = [...]`.
    pub dpi_libs: Vec<String>,
}

/// Baca file project .mivon — daftar file .sv + bagian `[foreign]`.
/// Baris non-kosong non-komentar di luar `[foreign]` = file .sv (satu per
/// baris, path relatif ke direktori .mivon, pola lama). Bagian `[foreign]`
/// berisi list library per interface (format TOML-like `key = ["a.so", ...]`).
pub fn read_project_with_foreign(path: &str) -> Result<ProjectFile, SimError> {
    let content = fs::read_to_string(path).map_err(|e| {
        SimError::with_diag(
            DiagCode::InvalidSyntax,
            format!("cannot read '{}': {}", path, e),
        )
    })?;
    let base = Path::new(path).parent().unwrap_or(Path::new("."));
    let mut proj = ProjectFile::default();
    let mut in_foreign = false;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_foreign = line.trim_matches(['[', ']']).trim() == "foreign";
            continue;
        }
        if in_foreign {
            // Format: `key = ["lib1.so", "lib2.so"]` (atau tanpa kurung).
            if let Some((key, val)) = line.split_once('=') {
                let key = key.trim();
                let list: Vec<String> = val
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',') // komentar //
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if list.is_empty() {
                    continue;
                }
                // Path library di-resolve relatif ke direktori .mivon (pola
                // sama dengan file .sv) — dlopen butuh path absolut.
                let resolved: Vec<String> = list
                    .iter()
                    .map(|p| {
                        let pbuf = base.join(p);
                        pbuf.to_string_lossy().to_string()
                    })
                    .collect();
                match key {
                    "vhpi" => proj.vhpi_libs.extend(resolved),
                    "pli" => proj.pli_libs.extend(resolved),
                    "dpi" => proj.dpi_libs.extend(resolved),
                    _ => {
                        // Kunci tak dikenal → peringatan via stderr (tidak gagal).
                        eprintln!("warning: [foreign] key '{}' tak dikenal di '{}'", key, path);
                    }
                }
            }
        } else {
            let p = base.join(line);
            proj.files.push(p.to_string_lossy().to_string());
        }
    }
    Ok(proj)
}

/// Compile multiple .sv files into IR design
pub fn compile_files(paths: &[String]) -> Result<mivon_ir::IrDesign, SimError> {
    let mut combined = String::new();
    let mut last_timescale = None;
    // F84 (LRM §19.8): segmen `timescale per-file digeser ke koordinat input
    // gabungan. Pass-2 (`compile_str_inner`) tak melihat directive lagi
    // (sudah di-consume pass-1 per-file) → segmen gabungan inilah yang dipakai
    // parser, bukan hasil pass-2 (yang kosong).
    let mut merged_ts: Vec<(usize, (String, String))> = Vec::new();
    for path in paths {
        let mut pp = Preprocessor::new();
        let processed = pp.preprocess_file(path)?;
        if pp.timescale.is_some() {
            last_timescale = pp.timescale.clone();
        }
        // Marker menempati 1 baris input gabungan; konten file mulai sesudahnya
        // (koordinat 1-based). `merge_timescale_segments` menggeser tiap `from`
        // dengan offset ini.
        let content_offset = combined.lines().count() + 1;
        mivon_parser::preprocessor::Preprocessor::merge_timescale_segments(
            &pp.timescale_segments,
            content_offset,
            &mut merged_ts,
        );
        combined.push_str(&format!("`line 1 \"{}\"\n", path));
        combined.push_str(&processed);
        combined.push('\n');
    }
    let mut result = compile_str_inner(&combined, false, Some(merged_ts))?;
    if last_timescale.is_some() && result.timescale.is_none() {
        // Fallback: tak ada segmen sama sekali (mis. semua file tak punya
        // directive, tapi `pp.timescale` terisi dari cache lama). Basis
        // terhalus tak ada pilihan lain.
        result.timescale = last_timescale;
    }
    Ok(result)
}

/// Satu error proyek dengan lokasi sumber asli (file:line:col post-mapping
/// include). Dipakai project-wide error sweep mivon-fuzz — bukan estimasi
/// offset: `source_snippet` memberikan posisi persis di file asli.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectError {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub code: String,
    pub message: String,
}

impl ProjectError {
    /// Lokasi `file:line:col` — format standar semua tool mivon.
    pub fn loc(&self) -> String {
        format!("{}:{}:{}", self.file, self.line, self.col)
    }
}

/// Konversi `Diagnostic` mivon → `ProjectError`. Preferensi `source_snippet`
/// (file/line/col asli dari file-line-map include); fallback spans.
fn diagnostic_to_project_error(d: &mivon_core::diagnostics::Diagnostic) -> ProjectError {
    let loc = if let Some(ss) = &d.source_snippet {
        ProjectError {
            file: ss.file.clone(),
            line: ss.line,
            col: ss.col,
            code: String::new(),
            message: String::new(),
        }
    } else if let Some(span) = d.spans.first() {
        ProjectError {
            file: span.file.as_str().to_string(),
            line: span.start as usize,
            col: span.end as usize,
            code: String::new(),
            message: String::new(),
        }
    } else {
        ProjectError {
            file: "<design>".to_string(),
            line: 0,
            col: 0,
            code: String::new(),
            message: String::new(),
        }
    };
    ProjectError {
        code: d.code.as_str().to_string(),
        message: d.message.to_string(),
        ..loc
    }
}

/// Compile proyek (banyak file sebagai SATU design) dan kumpulkan SEMUA
/// error — parse + elaborasi (modus AnalysisRecovery: top tak unik / hierarki
/// tidak menggagalkan analisis) — dengan lokasi file:line:col asli.
///
/// Berbeda dari `compile_str`/`compile_files` yang early-return error PERTAMA:
/// sweep kepatuhan butuh GAMBARAN UTUH error proyek (fitur LRM belum didukung,
/// dependensi lintas file, macro, package) — bukan satu titik gagal pertama.
/// Dipakai `mivon-fuzz` project-wide seed sweep (Paper #12 corpus nyata).
pub fn compile_collect_errors(paths: &[String]) -> Vec<ProjectError> {
    compile_collect_errors_inc(paths, &[])
}

/// Varian `compile_collect_errors` dengan incdirs eksternal (search path
/// `include tambahan). Auto-parent-dirs semua file tetap ditambahkan.
pub fn compile_collect_errors_inc(paths: &[String], incdirs: &[String]) -> Vec<ProjectError> {
    // ── Search path include: parent dir tiap file + ancestor (depth ≤4,
    //    mengikuti auto-incdir scan CLI) + incdirs eksternal. OpenTitan
    //    memakai include lintas-dir (mis. `prim_mubi_pkg.sv` dari ip lain)
    //    yang TIDAK tersolve oleh parent-dir file saja → file di-drop →
    //    error nyata proyek hilang dari sweep. ──
    let mut search: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in paths {
        let path = Path::new(p);
        let mut anc = path.parent().map(|d| d.to_path_buf());
        let mut depth = 0;
        while let Some(ref d) = anc {
            if seen.insert(d.clone()) {
                search.push(d.clone());
            }
            if depth >= 4 {
                break;
            }
            anc = d.parent().map(|d| d.to_path_buf());
            depth += 1;
        }
    }
    for i in incdirs {
        let d = PathBuf::from(i);
        if !seen.contains(&d) {
            search.push(d);
        }
    }
    // Satu preprocessor bersama (defines/include-set global proyek).
    let mut base_pp = Preprocessor::new();
    for s in &search {
        if let Some(s) = s.to_str() {
            base_pp.add_search_path(s);
        }
    }
    // ── Gabung file + kumpulkan error preprocess (JANGAN drop file) ──
    let mut combined = String::new();
    let mut out: Vec<ProjectError> = Vec::new();
    for path in paths {
        let mut pp = base_pp.clone();
        match pp.preprocess_file(path) {
            Ok(processed) => {
                combined.push_str(&format!("`line 1 \"{}\"\n", path));
                combined.push_str(&processed);
                combined.push('\n');
            }
            Err(e) => {
                // Include tak terresolve / IO error: rekam sebagai gap proyek,
                // jangan drop diam-diam (sebelumnya: continue → 1679 error
                // nyata opentitan hilang jadi 16).
                out.push(ProjectError {
                    file: path.clone(),
                    line: 1,
                    col: 1,
                    code: "E1001".to_string(),
                    message: format!("preprocessor: {}", e),
                });
            }
        }
    }
    if combined.is_empty() {
        return out;
    }
    let mut pp = Preprocessor::new();
    let Ok(preprocessed) = pp.preprocess(&combined, None) else {
        return out;
    };
    let mut lexer = Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let file_line_map = lexer.file_line_map.clone();
    let first_source = if file_line_map.is_empty() {
        "<string>".to_string()
    } else {
        file_line_map[0].2.clone()
    };
    let mut parser = Parser::new(tokens, &first_source)
        .with_source_lines(&preprocessed)
        .with_file_line_map(file_line_map);
    // F84 (LRM §19.8): segmen `timescale per baris → parser menempelkannya
    // ke tiap module. Tanpa ini semua delay diskalakan dengan satu satuan
    // global dan `#5` di module `1us` diperlakukan 5ns.
    parser.timescale_segments = pp.timescale_segments.clone();
    let design = match parser.parse_design() {
        Err(_) => {
            out.extend(
                parser
                    .errors
                    .iter()
                    .filter(|d| d.is_error())
                    .map(diagnostic_to_project_error),
            );
            return out;
        }
        Ok(design) => design,
    };
    // Error parse yang dikumpulkan (bukan fatal) tetap dilaporkan.
    out.extend(
        parser
            .errors
            .iter()
            .filter(|d| d.is_error())
            .map(diagnostic_to_project_error),
    );
    // Elaborasi modus recovery: error semantik/hierarki/top dikumpulkan,
    // top tidak unik TIDAK menggagalkan analisis (sama seperti CLI run).
    let mut elaborator = mivon_elaboration::Elaborator::with_source(
        design,
        preprocessed.lines().map(|s| s.to_string()).collect(),
        first_source,
    );
    let _ = elaborator.elaborate(None, ElaborateMode::AnalysisRecovery);
    out.extend(
        elaborator
            .flush_diagnostics()
            .iter()
            .filter(|d| d.is_error())
            .map(diagnostic_to_project_error),
    );
    out
}

/// Compile a SystemVerilog source file and run simulation
pub fn simulate_file(path: &str, max_time: u64) -> Result<(), SimError> {
    let source = fs::read_to_string(path).map_err(|e| {
        SimError::with_diag(
            DiagCode::InvalidSyntax,
            format!("cannot read '{}': {}", path, e),
        )
    })?;
    simulate_str(&source, max_time)
}

/// Compile SystemVerilog source string and run simulation
pub fn simulate_str(source: &str, max_time: u64) -> Result<(), SimError> {
    let design = compile_str(source)?;
    run_simulation(design, max_time)
}

/// Jumlah diagnostik parser (warnings, errors) utk source — TANPA emisi.
/// Dipakai mivon-fuzz utk menolak klaim property-oracle pada source yang
/// di-RECOVERY parser (stray `endtask` dsb → warning E1005 → semantik bisa
/// tak konsisten/mangle — bukan bukti bug engine; temuan korpus opentitan:
/// 5× false-positive mirror `_fz_viol=1` semua dipicu baris `endtask : body`).
pub fn compile_diag_counts(source: &str) -> (usize, usize) {
    let mut pp = Preprocessor::new();
    let Ok(preprocessed) = pp.preprocess(source, None) else {
        return (0, 1);
    };
    let mut lexer = Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let file_line_map = lexer.file_line_map.clone();
    let first_source = if file_line_map.is_empty() {
        "<string>".to_string()
    } else {
        file_line_map[0].2.clone()
    };
    let mut parser = Parser::new(tokens, &first_source)
        .with_source_lines(&preprocessed)
        .with_file_line_map(file_line_map);
    let _ = parser.parse_design();
    let mut w = 0usize;
    let mut e = 0usize;
    for d in &parser.errors {
        if d.is_error() {
            e += 1;
        } else {
            w += 1;
        }
    }
    (w, e)
}

/// Compile SystemVerilog source string into IR
pub fn compile_str(source: &str) -> Result<mivon_ir::IrDesign, SimError> {
    compile_str_inner(source, false, None)
}

/// Compile SystemVerilog source string into IR — versi SENYAP: diagnostik
/// parser/elaborator TIDAK di-emit ke stderr (hanya verdict/code yang
/// diambil). Dipakai mivon-fuzz: tiap mutasi child yang gagal-compile
/// mencetak puluhan baris diagnosa (E1002/E1005/WR0102) → noise stderr +
/// biaya I/O per iterasi.
pub fn compile_str_quiet(source: &str) -> Result<mivon_ir::IrDesign, SimError> {
    compile_str_inner(source, true, None)
}

/// Inti kompilasi string. `ts_override`: segmen `timescale siap pakai
/// (F84) — dipakai `compile_files` yang directive-nya sudah di-consume
/// pass-1 per-file sehingga pass-2 tak bisa menurunkannya sendiri.
/// `None` = turunkan dari preprocess pass ini (jalur normal).
fn compile_str_inner(
    source: &str,
    quiet: bool,
    ts_override: Option<Vec<(usize, (String, String))>>,
) -> Result<mivon_ir::IrDesign, SimError> {
    let mut pp = Preprocessor::new();
    let preprocessed = pp.preprocess(source, None).map_err(|e| {
        SimError::with_diag(DiagCode::InvalidSyntax, format!("preprocessor: {}", e))
    })?;
    let mut lexer = Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }

    let file_line_map = lexer.file_line_map.clone();
    let first_source = if file_line_map.is_empty() {
        "<string>".to_string()
    } else {
        file_line_map[0].2.clone()
    };
    // source_lines harus header-aligned (konsisten dgn jalur CLI main.rs yang
    // prepend `` `line 1 "file" ``): `source_lines[0] = directive`, konten
    // baris N di [N]. Tanpa ini snippet_source_line(display_line=N) salah
    // index (off-by-one) → error EOF/di akhir file render tanpa file:line:col.
    let header_line = format!("`line 1 \"{}\"", first_source);
    let source_with_header = format!("{}\n{}", header_line, preprocessed);
    let mut parser = Parser::new(tokens, &first_source)
        .with_source_lines(&source_with_header)
        .with_file_line_map(file_line_map);
    // F84 (LRM §19.8): `timescale per-baris → tempel ke tiap module. Tanpa ini
    // semua delay diskalakan dengan satu satuan global, sehingga `#5` di
    // module `1us/1ns` diperlakukan 5 tick basis `1ns`. `compile_files`
    // memasok segmen gabungan (directive sudah di-consume pass-1).
    parser.timescale_segments = ts_override.unwrap_or_else(|| pp.timescale_segments.clone());
    let design = match parser.parse_design() {
        Ok(d) => d,
        Err(e) => {
            // Parse function returned fatal error — emit collected errors too
            if !quiet && !parser.errors.is_empty() {
                let mut emitter =
                    mivon_core::diagnostics::TerminalEmitter::new().with_simple_mode(true);
                for diag in &parser.errors {
                    let _ = emitter.emit(diag);
                }
            }
            return Err(e);
        }
    };
    // Cek accumulated parser diagnostics (warnings + errors)
    // Hanya abort untuk real errors, warnings seperti "skipping construct" tetap lanjut
    if !parser.errors.is_empty() {
        let has_real_errors = parser.errors.iter().any(|d| d.is_error());
        if !quiet {
            let mut emitter =
                mivon_core::diagnostics::TerminalEmitter::new().with_simple_mode(true);
            for diag in &parser.errors {
                let _ = emitter.emit(diag);
            }
        }
        if has_real_errors {
            return Err(SimError::from_parse_diagnostic(parser.errors[0].clone()));
        }
    }
    // F84: `design.timescale` kini di-set parser dari segmen (satuan TERHALUS
    // — basis tick elaborator). Nilai lama `pp.timescale` = directive TERAKHIR
    // dan tak boleh menimpa: `1us` terakhir akan membuat module `1ns` ikut
    // diskalakan 1000×.

    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, first_source);
    let ir_design = elaborator.elaborate(None, ElaborateMode::StrictSimulation)?;

    // SIM-29: bawa exclusion ranges dari `` `coverage_off ``/`` `coverage_on ``
    // (koordinat output preprocessed) ke design untuk engine line coverage.
    let mut ir_design = ir_design;
    ir_design.coverage_exclusions = pp.coverage_exclusions.clone();

    // Flush elaboration-time diagnostics (warnings like WR0102)
    let elab_diags = elaborator.flush_diagnostics();
    if !quiet && !elab_diags.is_empty() {
        let mut emitter = mivon_core::diagnostics::TerminalEmitter::new().with_simple_mode(true);
        for diag in &elab_diags {
            let _ = emitter.emit(diag);
        }
    }

    Ok(ir_design)
}

/// Compile SystemVerilog source string — versi ANALISIS (recovery): mesh
/// source multi-modul TANPA top / dengan error partial tetap mengembalikan
/// IrDesign (mode `AnalysisRecovery`, bukan `StrictSimulation`).
///
/// Dipakai mivon-fuzz: seed RTL murni (`03_mux_decoder.sv` berisi mux+decoder
/// tanpa testbench) tidak punya top → StrictSimulation gagal E3006 padahal
/// setiap module valid. Recovery membiarkan elaborasi selesai + error dicatat
/// (didapat via `flush_diagnostics`), hasilnya bisa dianalisis.
pub fn compile_str_analyze(source: &str) -> Result<mivon_ir::IrDesign, SimError> {
    let mut pp = Preprocessor::new();
    let preprocessed = pp.preprocess(source, None).map_err(|e| {
        SimError::with_diag(DiagCode::InvalidSyntax, format!("preprocessor: {}", e))
    })?;
    let mut lexer = Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }

    let file_line_map = lexer.file_line_map.clone();
    let first_source = if file_line_map.is_empty() {
        "<string>".to_string()
    } else {
        file_line_map[0].2.clone()
    };
    // source_lines harus header-aligned (lihat compile_str_inner).
    let header_line = format!("`line 1 \"{}\"", first_source);
    let source_with_header = format!("{}\n{}", header_line, preprocessed);
    let mut parser = Parser::new(tokens, &first_source)
        .with_source_lines(&source_with_header)
        .with_file_line_map(file_line_map);
    // F84 (LRM §19.8): sama seperti compile_str_inner — `timescale per module.
    parser.timescale_segments = pp.timescale_segments.clone();
    let design = parser.parse_design()?;
    if parser.errors.iter().any(|d| d.is_error()) {
        // Parse error fatal — recovery tetap lanjut bila modul masih utuh.
    }
    // F84: `design.timescale` diisi parser dari segmen (satuan terhalus) —
    // lihat catatan di `compile_str_inner`.

    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, first_source);
    let ir_design =
        elaborator.elaborate(None, mivon_elaboration::ElaborateMode::AnalysisRecovery)?;

    let mut ir_design = ir_design;
    ir_design.coverage_exclusions = pp.coverage_exclusions.clone();
    Ok(ir_design)
}

/// Run simulation on compiled IR
pub fn run_simulation(ir_design: mivon_ir::IrDesign, max_time: u64) -> Result<(), SimError> {
    let mut engine = simulator::SimulationEngine::new(ir_design, max_time);

    let design_name = &engine.design.top.name.clone();
    // Use a unique prefix to avoid file name collisions when running tests in parallel
    // Many tests use "top" as the module name, which would cause file conflicts
    static SIMULATION_COUNTER: AtomicUsize = AtomicUsize::new(0);
    let unique_id = SIMULATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let unique_prefix = format!("{}_{}", design_name, unique_id);
    let vcd_path = format!("{}.vcd", unique_prefix);
    let vcd = waveform::VcdWriter::new(&vcd_path, &engine.design).map_err(|e| {
        SimError::with_diag(
            DiagCode::WaveformError,
            format!("VCD creation failed: {}", e),
        )
    })?;
    engine.set_vcd(vcd);

    // Also create FST waveform
    let fst_path = format!("{}.fst", unique_prefix);
    match waveform::FstWaveWriter::new(&fst_path, &engine.design) {
        Ok(fst) => engine.set_fst(fst),
        Err(e) => {
            let diag = mivon_core::diagnostics::Diagnostic::warning(
                mivon_core::diagnostics::DiagCode::WaveformError,
                format!("FST: cannot create '{}': {}", fst_path, e),
            );
            let mut emitter =
                mivon_core::diagnostics::TerminalEmitter::new().with_simple_mode(true);
            let _ = emitter.emit(&diag);
        }
    }

    engine.run()?;

    // Flush any runtime diagnostics
    let diagnostics = engine.flush_diagnostics();
    if !diagnostics.is_empty() {
        let mut emitter = mivon_core::diagnostics::TerminalEmitter::new().with_simple_mode(true);
        for diag in &diagnostics {
            let _ = emitter.emit(diag);
        }
    }

    println!("Simulation completed at time {}", engine.state.time);
    println!("VCD waveform written to '{}'", vcd_path);
    println!("FST waveform written to '{}'", fst_path);

    Ok(())
}

/// Run simulation and return final signal values
pub fn simulate_signals(
    source: &str,
    max_time: u64,
) -> Result<Vec<(String, mivon_ir::LogicVec)>, SimError> {
    let sigs = simulate_signals_with_coverage_inner(source, max_time, false)?.0;
    Ok(sigs)
}

/// Jalur fuzzer: versi senyap `simulate_signals` (tanpa emisi diagnostik &
/// laporan coverage akhir-run; dipakai oracle nilai sinyal & fingerprint).
pub fn simulate_signals_quiet(
    source: &str,
    max_time: u64,
) -> Result<Vec<(String, mivon_ir::LogicVec)>, SimError> {
    let sigs = simulate_signals_with_coverage_inner(source, max_time, true)?.0;
    Ok(sigs)
}

/// Jalur fuzzer: fingerprint MID-SIMULATION — sinyal top disampling tiap
/// `interval` waktu (`engine.set_trace_interval`) + nilai final. Bug
/// transient (salah di delta lalu pulih sebelum akhir run) tak terlihat
/// oleh fingerprint nilai-final; trace sampling menutup celah. Senyap.
pub fn simulate_signals_with_trace_quiet(
    source: &str,
    max_time: u64,
    interval: u64,
) -> Result<(Vec<(String, mivon_ir::LogicVec)>, Vec<String>), SimError> {
    let design = compile_str_quiet(source)?;
    let mut engine = simulator::SimulationEngine::new(design, max_time);
    engine.set_coverage_report_silent();
    engine.set_trace_interval(interval);
    engine.run()?;
    let sigs: Vec<(String, mivon_ir::LogicVec)> = engine
        .design
        .top
        .signals
        .iter()
        .map(|s| {
            (
                s.name.to_string(),
                engine
                    .state
                    .read_signal(
                        engine
                            .design
                            .top
                            .signals
                            .iter()
                            .position(|x| x.name == s.name)
                            .unwrap_or(0),
                    )
                    .clone(),
            )
        })
        .collect();
    let trace = engine.trace_snapshots.clone();
    Ok((sigs, trace))
}

/// Run simulation and return final signal values PLUS coverage feedback
/// (`SimulationEngine::coverage_keys`, lihat engine/coverage.rs).
/// Coverage keys = item line/branch/toggle/FSM yang benar-benar tereksekusi —
/// dipakai mivon-fuzz sebagai umpan coverage nyata (bukan teks statistik).
pub fn simulate_signals_with_coverage(
    source: &str,
    max_time: u64,
) -> Result<(Vec<(String, mivon_ir::LogicVec)>, Vec<String>), SimError> {
    simulate_signals_with_coverage_inner(source, max_time, false)
}

/// Jalur fuzzer: versi SENYAP — compile tanpa emisi diagnostik ke stderr DAN
/// laporan coverage akhir-run di-senyapkan (tiap simulasi = engine.run();
/// report penuh per iterasi = noise + I/O, temuan kampanye seed 42).
/// Coverage keys tetap diambil via `coverage_keys()`.
pub fn simulate_signals_with_coverage_quiet(
    source: &str,
    max_time: u64,
) -> Result<(Vec<(String, mivon_ir::LogicVec)>, Vec<String>), SimError> {
    simulate_signals_with_coverage_inner(source, max_time, true)
}

fn simulate_signals_with_coverage_inner(
    source: &str,
    max_time: u64,
    quiet: bool,
) -> Result<(Vec<(String, mivon_ir::LogicVec)>, Vec<String>), SimError> {
    simulate_signals_cfg_inner(source, max_time, quiet, &EngineFlags::default())
}

/// Eksposur jalur eksekusi internal engine (deep differential fuzzing).
/// Flag memilih jalur evaluasi: packed-eval, DAG-parallel, timing wheel,
/// MIR JIT — masing-masing harus menghasilkan hasil IDENTIK utk input sama.
#[derive(Debug, Clone, Copy, Default)]
pub struct EngineFlags {
    pub use_packed_eval: bool,
    pub use_dag_parallel: bool,
    pub use_timing_wheel: bool,
    pub use_mir_jit: bool,
}

fn simulate_signals_cfg_inner(
    source: &str,
    max_time: u64,
    quiet: bool,
    flags: &EngineFlags,
) -> Result<(Vec<(String, mivon_ir::LogicVec)>, Vec<String>), SimError> {
    let design = compile_str_quiet(source)?;
    let mut engine = simulator::SimulationEngine::new(design, max_time);
    if quiet {
        engine.set_coverage_report_silent();
    }
    engine.set_use_packed_eval(flags.use_packed_eval);
    engine.set_use_dag_parallel(flags.use_dag_parallel);
    engine.set_use_timing_wheel(flags.use_timing_wheel);
    engine.set_use_mir_jit(flags.use_mir_jit);
    engine.run()?;
    let sigs: Vec<(String, mivon_ir::LogicVec)> = engine
        .design
        .top
        .signals
        .iter()
        .map(|s| {
            (
                s.name.to_string(),
                engine
                    .state
                    .read_signal(
                        engine
                            .design
                            .top
                            .signals
                            .iter()
                            .position(|x| x.name == s.name)
                            .unwrap_or(0),
                    )
                    .clone(),
            )
        })
        .collect();
    let cov = engine.coverage_keys();
    Ok((sigs, cov))
}

/// Jalur fuzzer: simulasi dgn flag jalur internal (packed/DAG/timing-wheel/
/// MIR JIT). Untuk differential across execution paths — result harus sama.
pub fn simulate_signals_with_flags_quiet(
    source: &str,
    max_time: u64,
    flags: &EngineFlags,
) -> Result<Vec<(String, mivon_ir::LogicVec)>, SimError> {
    Ok(simulate_signals_cfg_inner(source, max_time, true, flags)?.0)
}

#[cfg(test)]
#[macro_use]
pub mod test_util;

// Test suite utama pindah ke crate `mivon-tests` (crates/) — lib ini hanya
// menyimpan API publik + helper test (test_util).

#[cfg(test)]
mod project_file_tests {
    use super::*;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mivon_proj_{}_{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).expect("buat dir");
        let p = dir.join("proj.mivon");
        std::fs::write(&p, content).expect("tulis");
        p
    }

    #[test]
    fn test_read_project_with_foreign_parses_libs() {
        let p = write_temp(
            "foreign",
            r#"
rtl/top.sv
rtl/counter.sv

[foreign]
vhpi = ["libvhpi_a.so", "libvhpi_b.so"]
pli = ["libpli.so"]
dpi = ["libdpi.so"]
"#,
        );
        let proj = read_project_with_foreign(p.to_str().unwrap()).expect("parse");
        // File .sv ter-resolve relatif ke direktori project.
        assert_eq!(proj.files.len(), 2);
        let base = p.parent().unwrap();
        assert!(
            proj.files[0].starts_with(base.to_str().unwrap()),
            "path relatif ke .mivon"
        );
        // Library ter-resolve + terpisah per interface.
        assert_eq!(proj.vhpi_libs.len(), 2);
        assert!(proj.vhpi_libs[0].contains("libvhpi_a.so"));
        assert!(proj.vhpi_libs[1].contains("libvhpi_b.so"));
        assert_eq!(proj.pli_libs.len(), 1);
        assert!(proj.pli_libs[0].contains("libpli.so"));
        assert_eq!(proj.dpi_libs.len(), 1);
        assert!(proj.dpi_libs[0].contains("libdpi.so"));
        // Path library juga relatif ke direktori project.
        assert!(proj.vhpi_libs[0].starts_with(base.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn test_read_project_file_skips_foreign_section() {
        let p = write_temp(
            "skip",
            r#"
rtl/top.sv

[foreign]
vhpi = ["libvhpi.so"]
"#,
        );
        let files = read_project_file(p.to_str().unwrap()).expect("parse");
        assert_eq!(files.len(), 1, "bagian [foreign] TIDAK boleh jadi file .sv");
        assert!(files[0].contains("rtl/top.sv"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn test_read_project_with_foreign_unknown_key_warns() {
        let p = write_temp(
            "unknown",
            r#"
rtl/top.sv

[foreign]
vhpi = ["libvhpi.so"]
foo = ["libfoo.so"]
"#,
        );
        let proj = read_project_with_foreign(p.to_str().unwrap()).expect("parse");
        assert_eq!(proj.vhpi_libs.len(), 1);
        assert!(proj.dpi_libs.is_empty(), "kunci tak dikenal diabaikan");
        assert!(proj.pli_libs.is_empty());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn test_read_project_with_foreign_no_section() {
        let p = write_temp("nosect", "rtl/top.sv\n");
        let proj = read_project_with_foreign(p.to_str().unwrap()).expect("parse");
        assert_eq!(proj.files.len(), 1);
        assert!(proj.vhpi_libs.is_empty());
        assert!(proj.pli_libs.is_empty());
        assert!(proj.dpi_libs.is_empty());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
