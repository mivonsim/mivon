//! Backend GUI — memanggil API library `mivon` langsung (tanpa IPC).
//!
//! Compile/elaborate via `CompileSession`, simulasi via `SimulationEngine`.
//! Operasi berat dijalankan di worker thread; hasil dikirim melalui channel
//! sehingga UI tetap responsif.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use mivon_compiler::frontend::compile_session::{CompileSession, SessionConfig};
use mivon_compiler::frontend::module_index::EntryKind;
use mivon_core::error::SimError;
use mivon_ir::{IrDesign, LogicVal, LogicVec};
use mivon_simulator::simulator::SimulationEngine;

use super::state::{
    blocking_assign_pos, word_count, AssertionRow, CompileInfo, CoverageInfo, CovergroupRow,
    DepRow, DiagEntry, DiagLevel, FileNode, GuiEvent, InstanceRow, MacroRow, MicdInfo, ParamRow,
    PipelineStage, QuickFix, QuickFixKind, SignalRow, SimInfo, TextHit, TraceEvent, WaveformSignal,
    STAGE_SIMULATOR,
};

/// Scan direktori → pohon file (rekursif, sinkron).
///
/// Penanganan symlink (agar GUI TIDAK stack overflow di proyek dengan
/// symlink-loop seperti `rtl/fabric/fabric → rtl/fabric`):
/// - `entry.file_type()` TIDAK mengikuti symlink (jadi isi symlink tidak
///   pernah di-rekursi secara buta).
/// - `descend()` mencatat path canonical direktori yang sedang terbuka di
///   rantai rekursi; bila target symlink sudah ada di rantai itu (loop), node
///   diperlakukan sebagai LEAF (tidak di-rekursi). Symlink ke direktori asli
///   yang aman tetap dieksplor.
pub fn scan_tree(root: &Path) -> Vec<FileNode> {
    let mut trail: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    descend(root, &mut trail)
}

/// Rekursi satu level: baca entry `dir` dan bangun children. Dipanggil untuk
/// direktori biasa maupun target symlink yang sudah di-resolve & di-guard
/// siklus (lihat `descend`).
fn build_level(dir: &Path, trail: &mut std::collections::HashSet<PathBuf>) -> Vec<FileNode> {
    let mut nodes = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return nodes;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden & build artifacts
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        // file_type() TIDAK mengikuti symlink — isi symlink tidak pernah
        // di-rekursi tanpa pemeriksaan siklus.
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            let children = descend(&path, trail);
            nodes.push(FileNode {
                name,
                path,
                is_dir: true,
                children,
            });
            continue;
        }
        if ft.is_symlink() {
            // Symlink: resolve target. Bila target = direktori asli dan tidak
            // membentuk siklus (target canonical belum ada di rantai aktif)
            // → dieksplor sebagai direktori; selain itu (loop / target rusak /
            // bukan dir) → leaf. Guard ada di `descend`.
            if let Ok(target) = path.canonicalize() {
                if target.is_dir() {
                    let children = descend(&target, trail);
                    nodes.push(FileNode {
                        name,
                        path,
                        is_dir: true,
                        children,
                    });
                    continue;
                }
            }
            nodes.push(FileNode {
                name,
                path,
                is_dir: false,
                children: Vec::new(),
            });
            continue;
        }
        nodes.push(FileNode {
            name,
            path,
            is_dir: false,
            children: Vec::new(),
        });
    }
    nodes.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    nodes
}

/// Masuk ke direktori `dir` dengan guard siklus: canonical path didaftarkan di
/// `trail`; bila sudah ada (path ini sedang dibuka di rantai) → loop, kembalikan
/// kosong (tidak di-rekursi ulang).
fn descend(dir: &Path, trail: &mut std::collections::HashSet<PathBuf>) -> Vec<FileNode> {
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !trail.insert(canon.clone()) {
        // Siklus (symlink ke ancestor / dir yang sedang terbuka) → prune.
        return Vec::new();
    }
    let out = build_level(dir, trail);
    trail.remove(&canon);
    out
}

/// Stack worker thread GUI (compile/sim/terminal) — 16MB, cermin rayon pool
/// CLI (lib.rs::run + src/main.rs). Simulator bisa merekursi dalam (eval
/// ekspresi / desain besar); stack default ~8MB berisiko overflow di hamparan
/// proses. Kegagalan spawn dilaporkan via stderr (jarang).
const WORKER_STACK: usize = 16 * 1024 * 1024;

fn spawn_worker<F: FnOnce() + Send + 'static>(f: F) {
    if let Err(e) = std::thread::Builder::new()
        .name("mivon-worker".into())
        .stack_size(WORKER_STACK)
        .spawn(f)
    {
        eprintln!("[backend] spawn worker gagal: {}", e);
    }
}

/// Compile + elaborate project di worker thread. `project_root` dipakai untuk
/// mencari database MICD (`.mivon/database`) — bila ada, compile menjadi
/// incremental (file tak berubah di-restore AST, lexer+parser di-skip).
pub fn spawn_compile(tx: Sender<GuiEvent>, paths: Vec<PathBuf>, project_root: Option<PathBuf>) {
    spawn_worker(move || {
        let result = compile_project(&paths, project_root.as_deref());
        let _ = tx.send(GuiEvent::CompileDone(result));
    });
}

/// Ubah `SimError` menjadi daftar `DiagEntry` (dengan `file`/`line` bila tersedia
/// dari source snippet). Dipakai supaya Mini Map editor & Problems tab bisa
/// menunjuk baris yang salah, bukan cuma error global.
fn simerr_to_diags(err: &SimError) -> Vec<DiagEntry> {
    let diag = err.to_diagnostic();
    let mut out = Vec::new();
    if let Some(snip) = &diag.source_snippet {
        out.push(DiagEntry {
            file: snip.file.clone(),
            line: snip.line,
            message: diag.message.to_string(),
            level: DiagLevel::Error,
            fix: None,
        });
    }
    if out.is_empty() {
        out.push(DiagEntry {
            file: String::new(),
            line: 0,
            message: diag.message.to_string(),
            level: DiagLevel::Error,
            fix: None,
        });
    }
    out
}

/// Compile + elaborate semua file. Mengembalikan info + design ter-elaborasi,
/// atau daftar diagnostics (error) dengan lokasi file/line.
/// `project_root` dipakai mencari database MICD (`.mivon/database`) — bila
/// ada, compile incremental: file tak berubah di-restore AST-nya (lexer+
/// parser di-skip), lalu state disimpan kembali setelah compile.
pub fn compile_project(
    paths: &[PathBuf],
    project_root: Option<&std::path::Path>,
) -> Result<(CompileInfo, IrDesign), Vec<DiagEntry>> {
    let start = std::time::Instant::now();

    let mut config = SessionConfig::default();
    for p in paths {
        config.sources.push(p.clone());
    }
    config.use_lazy_elab = true;
    config.auto_incdirs = true;

    let mut session = CompileSession::new(config);

    // ── MICD: attach database persisten (incremental compile lintas run) ──
    // Bila database belum ada (compile pertama), open memberi DB kosong dan
    // save_micd() di bawah akan membuatnya — run berikutnya file yang hash
    // kontennya sama di-restore AST-nya (skip lexer+parser). Root database =
    // `<project>/.mivon/database` (sama dengan CLI). Scoped per project
    // (ProjectID dari root + sources): OpenTitan dan test/counter.sv tidak
    // pernah berbagi database. Payload CAS di objects/<pid>/, index di
    // state/<pid>/.
    let micd_root = project_root.map(|r| r.join(".mivon").join("database"));
    if let Some(micd_root) = micd_root.as_deref() {
        let proot = project_root.expect("micd_root hanya ada bila project_root ada");
        let pid = mivon_compiler::micd::MicdDatabase::project_id(proot, paths, &[], &[]);
        let db = mivon_compiler::micd::MicdDatabase::open_project_with_context(
            micd_root, &pid, proot, paths,
        );
        session.attach_micd(db);
    }

    let (_design, ir_design, _idx) = match session.compile_and_elaborate(None) {
        Ok(v) => v,
        Err(e) => return Err(simerr_to_diags(&e)),
    };

    // ── MICD: tandai ter-elaborasi + simpan database + kumpulkan statistik ──
    // untuk panel Pipeline. Best-effort: kegagalan save tidak menggagalkan
    // compile (statistik hanya berisi 0/None).
    let micd = if session.micd.is_some() {
        session.micd_mark_elaborated();
        let stats = session.save_micd().ok().flatten();
        let db_ref = session.micd.as_ref();
        Some(MicdInfo {
            present: true,
            files: db_ref.map(|d| d.files.len()).unwrap_or(0),
            restored_ast: stats
                .as_ref()
                .map(|s| s.restored_designs)
                .unwrap_or_else(|| session.micd_restored_count()),
            changed_files: stats.as_ref().map(|s| s.changed_files).unwrap_or(0),
            verify_hits: stats
                .as_ref()
                .map(|s| s.verify_hits)
                .unwrap_or_else(|| db_ref.map(micd_verify_hits).unwrap_or(0)),
            verify_misses: stats
                .as_ref()
                .map(|s| s.verify_misses)
                .unwrap_or_else(|| db_ref.map(micd_verify_misses).unwrap_or(0)),
            snapshots: db_ref.map(|d| d.snapshots.len()).unwrap_or(0),
            db_bytes: micd_db_bytes(micd_root.as_deref()),
        })
    } else {
        None
    };

    // ── Pipeline timing per tahap (dari session.timing + estimasi tahap
    // elaboration) untuk panel Pipeline. Optimizer & Simulator belum
    // dijalankan saat compile — status "waiting". ──
    let t = &session.timing;
    let pipeline = vec![
        PipelineStage {
            name: "Discovery".into(),
            ms: t.discovery_us / 1000,
            status: "ok".into(),
        },
        PipelineStage {
            name: "Preprocessor".into(),
            ms: t.preprocess_us / 1000,
            status: "ok".into(),
        },
        PipelineStage {
            name: "Lexer".into(),
            ms: t.lex_us / 1000,
            status: "ok".into(),
        },
        PipelineStage {
            name: "Parser".into(),
            ms: t.parse_us / 1000,
            status: "ok".into(),
        },
        PipelineStage {
            name: "Elaborator".into(),
            ms: t.elab_us / 1000,
            status: "ok".into(),
        },
        PipelineStage {
            name: "Optimizer".into(),
            ms: 0,
            status: "waiting".into(),
        },
        PipelineStage {
            name: STAGE_SIMULATOR.into(),
            ms: 0,
            status: "waiting".into(),
        },
    ];
    let cached_files = t.cached_files;
    let processed_files = t.processed_files;

    let modules: Vec<String> = session
        .module_index
        .iter()
        .filter(|(_, kind, _)| *kind == EntryKind::Module)
        .map(|(name, _, _)| name.to_string())
        .collect();
    // Module → file path (untuk klik-ke-buka di Architecture viewer)
    let module_files: HashMap<String, PathBuf> = session
        .module_index
        .iter()
        .filter(|(_, kind, _)| *kind == EntryKind::Module)
        .map(|(name, _, meta)| (name.to_string(), meta.file.clone()))
        .collect();
    let packages: Vec<String> = session
        .module_index
        .iter()
        .filter(|(_, kind, _)| *kind == EntryKind::Package)
        .map(|(name, _, _)| name.to_string())
        .collect();
    let interfaces: Vec<String> = session
        .module_index
        .iter()
        .filter(|(_, kind, _)| *kind == EntryKind::Interface)
        .map(|(name, _, _)| name.to_string())
        .collect();

    // ── Graf dependency: setiap module → module yang diinstansiasinya.
    // Dibangun dari IrDesign (sub_instances), deduplikasi per (parent, child)
    // dengan hitungan instance. Root = module top. ──
    let mut deps = build_dep_graph(&ir_design);
    // Taruh module top di paling atas agar mudah ditemukan.
    if let Some(pos) = deps
        .iter()
        .position(|d| d.module == ir_design.top.name.as_str())
    {
        let root = deps.remove(pos);
        deps.insert(0, root);
    }

    // ── Reference counts: module → berapa kali di-instansiasi di seluruh
    // design (termasuk top). Precompute sekali di sini supaya Code Lens editor
    // tidak perlu meng-iterasi design setiap frame. ──
    let ref_counts = build_ref_counts(&ir_design);

    // ── Signal info: nama signal → (tipe, lebar bit) untuk Hover tooltip
    // editor. Dibangun dari SEMUA module (top + submodule) — signal bisa
    // berada di file mana pun yang sedang dibuka. Precompute sekali di sini.
    // ──
    let mut signal_info: HashMap<String, (String, usize)> = HashMap::new();
    {
        let mut add_mod = |m: &mivon_ir::IrModule| {
            for s in &m.signals {
                signal_info
                    .entry(s.name.to_string())
                    .or_insert_with(|| (signal_type_str(s.kind.clone()).to_string(), s.width));
            }
        };
        add_mod(&ir_design.top);
        for m in ir_design.modules.values() {
            add_mod(m);
        }
    }

    // ── Symbol → file asal (module/interface/package) untuk Go To Definition
    // (Ctrl+Click). Precompute dari module_index sekali saat compile. ──
    let mut symbol_files: HashMap<String, PathBuf> = HashMap::new();
    for (name, kind, meta) in session.module_index.iter() {
        if matches!(
            kind,
            EntryKind::Module | EntryKind::Package | EntryKind::Interface
        ) {
            symbol_files.insert(name.to_string(), meta.file.clone());
        }
    }

    // ── Indeks parameter: module → (param name, file) dari module_index.
    // Tab Search → Find parameter. Deduplikasi per (name, module) — satu
    // module bisa terdaftar di beberapa file (mis. dua definisi). ──
    let mut param_index: Vec<ParamRow> = Vec::new();
    {
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for (mname, kind, meta) in session.module_index.iter() {
            if !matches!(kind, EntryKind::Module | EntryKind::Interface) {
                continue;
            }
            for p in &meta.params {
                let key = (p.name.to_string(), mname.to_string());
                if seen.insert(key.clone()) {
                    param_index.push(ParamRow {
                        name: key.0,
                        module: key.1,
                        file: meta.file.clone(),
                    });
                }
            }
        }
        param_index.sort_by(|a, b| a.name.cmp(&b.name));
    }

    // ── Indeks macro (`` `define ``): scan source mentah per file — nama +
    // baris deklarasi (untuk lompat). Heuristik per-baris: token pertama
    // setelah `` `define `` adalah nama macro (tanpa argumen `` `define FOO(x)``).
    let mut macro_index: Vec<MacroRow> = Vec::new();
    for p in paths {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        for (i, raw) in text.lines().enumerate() {
            let code = raw.split("//").next().unwrap_or(raw).trim_start();
            let Some(rest) = code.strip_prefix('`') else {
                continue;
            };
            let rest = rest.trim_start();
            if !rest.starts_with("define") {
                continue;
            }
            let after = rest["define".len()..].trim_start();
            // Nama macro: identifier pertama; hentikan di `(` (macro berargumen)
            // atau spasi/whitespace.
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            macro_index.push(MacroRow {
                name,
                file: p.clone(),
                line: i + 1,
            });
        }
    }
    macro_index.sort_by(|a, b| a.name.cmp(&b.name));

    // ── Indeks instance: dari sub_instances seluruh design (top + semua
    // module). File = module yang diinstansiasi; baris = posisi instansiasi.
    let mut instance_index: Vec<InstanceRow> = Vec::new();
    {
        let mut add_mod = |m: &mivon_ir::IrModule| {
            for inst in &m.sub_instances {
                let mod_name = inst.module_name.to_string();
                let file = module_files.get(&mod_name).cloned().unwrap_or_default();
                instance_index.push(InstanceRow {
                    name: inst.instance_name.to_string(),
                    module: mod_name,
                    file,
                    line: inst.line,
                });
            }
        };
        add_mod(&ir_design.top);
        for m in ir_design.modules.values() {
            if m.name != ir_design.top.name {
                add_mod(m);
            }
        }
        instance_index.sort_by(|a, b| a.name.cmp(&b.name));
    }

    // ── Lint ringan (GUI): unused signal + blocking assignment di blok
    // sequential — contoh diagnostic dengan Quick Fix di Problems tab. Scan
    // source mentah (bukan AST) supaya cepat & tidak bergantung elaborasi. ──
    let lint = lint_sources(paths);

    Ok((
        CompileInfo {
            success: true,
            modules,
            packages,
            interfaces,
            total_time_ms: start.elapsed().as_secs_f64() * 1000.0,
            module_files,
            deps,
            ref_counts,
            signal_info,
            symbol_files,
            param_index,
            macro_index,
            instance_index,
            lint,
            micd,
            pipeline,
            cached_files,
            processed_files,
        },
        ir_design,
    ))
}

/// Helper: jumlah hit verification cache (dari MicdDatabase — dipakai bila
/// MicdStats tidak tersedia).
fn micd_verify_hits(db: &mivon_compiler::micd::MicdDatabase) -> usize {
    db.verify.values().filter(|v| v.ok()).count()
}

/// Helper: jumlah miss verification cache.
fn micd_verify_misses(db: &mivon_compiler::micd::MicdDatabase) -> usize {
    db.files.len().saturating_sub(micd_verify_hits(db))
}

/// Helper: total ukuran database MICD di disk (bytes). Menghitung SEMUA file
/// di database root — store `.mdb`, objek CAS `.ast`/`.preproc`, snapshot,
/// `registry.json`, `VERSION` (sesuai layout Opsi B db.md).
fn micd_db_bytes(root: Option<&std::path::Path>) -> u64 {
    let Some(root) = root else {
        return 0;
    };
    fn walk(dir: &std::path::Path, total: &mut u64) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            // file_type() TIDAK mengikuti symlink — mencegah infinite
            // recursion bila database berisi symlink ke direktori induk.
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                walk(&p, total);
            } else if let Ok(m) = std::fs::metadata(&p) {
                *total += m.len();
            }
        }
    }
    let mut total = 0u64;
    walk(root, &mut total);
    total
}

/// ────────────────────────── Lint ringan (Quick Fix) ──────────────────────────
/// Scan source file untuk masalah umum RTL. Setiap temuan adalah `DiagEntry`
/// level Warning dengan `fix` (aksi perbaikan otomatis di Problems tab).
/// Ini bukan pengganti lint penuh compiler — cukup deteksi pola yang jelas
/// dari teks: (1) signal dideklarasikan tapi tak pernah dipakai, (2) assignment
/// blocking `=` di dalam blok sequential (`always_ff` / `always @(posedge)`).
fn lint_sources(paths: &[PathBuf]) -> Vec<DiagEntry> {
    let mut out: Vec<DiagEntry> = Vec::new();
    for p in paths {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        let fname = p.display().to_string();
        lint_unused_signals(&text, &fname, &mut out);
        lint_blocking_in_sequential(&text, &fname, &mut out);
    }
    out
}

/// Deteksi signal yang dideklarasikan tapi tidak pernah direferensikan di
/// file yang sama (heuristik teks per-file; cukup untuk Quick Fix "Hapus").
fn lint_unused_signals(text: &str, fname: &str, out: &mut Vec<DiagEntry>) {
    // Kata kunci tipe yang memulai deklarasi signal internal.
    const DECL_TYPES: &[&str] = &[
        "logic",
        "reg",
        "wire",
        "bit",
        "int",
        "integer",
        "byte",
        "shortint",
        "longint",
        "time",
        "real",
        "tri",
        "logic signed",
        "wire signed",
    ];
    let mut declared: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let code = raw.split("//").next().unwrap_or(raw).trim();
        if code.is_empty() {
            continue;
        }
        // Deklarasi harus diakhiri `;` dan TIDAK di dalam blok always/initial
        // (assignment `x = ...` bukan deklarasi). Cek kata pertama: tipe.
        if !code.ends_with(';') {
            continue;
        }
        let first_word = code.split_whitespace().next().unwrap_or("");
        if !DECL_TYPES.contains(&first_word) {
            continue;
        }
        // Ambil nama signal: token pertama setelah tipe + opsional range
        // `[7:0]`. Contoh: `logic [7:0] data;` → `data`, `wire a, b;` → a,b.
        let mut rest = code;
        if first_word.contains(" ") {
            rest = rest.split_once(' ').map(|x| x.1).unwrap_or("");
        }
        let rest = rest.trim_start();
        // Buang range `[...]` di awal (mengandung angka/`:`/`-`).
        let rest = rest
            .strip_prefix('[')
            .and_then(|r| r.split_once(']').map(|(_, after)| after.trim_start()))
            .unwrap_or(rest);
        // Token identifier pertama (hentikan di `,`, `=`, `;`, spasi).
        let mut names: Vec<String> = Vec::new();
        let mut cur = String::new();
        for c in rest.chars() {
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '$' => cur.push(c),
                ',' => {
                    if !cur.is_empty() {
                        names.push(std::mem::take(&mut cur));
                    }
                }
                ' ' | '=' | ';' | '(' | ')' | '[' | ']' | ':' => {
                    if !cur.is_empty() {
                        names.push(std::mem::take(&mut cur));
                        break;
                    }
                }
                _ => {
                    if !cur.is_empty() {
                        break;
                    }
                }
            }
            if !cur.is_empty() && matches!(c, '=' | ';' | '(' | '[') {
                break;
            }
        }
        if !cur.is_empty() {
            names.push(cur);
        }
        for name in names {
            // Lewati nama yang terlihat seperti keyword/number.
            if name
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(true)
            {
                continue;
            }
            if is_sv_keyword(&name) {
                continue;
            }
            declared.push((line_no, name));
        }
    }

    for (line_no, name) in declared {
        // Hitung semua kemunculan kata utuh (termasuk baris deklarasi).
        let count = word_count(text, &name);
        if count <= 1 {
            out.push(DiagEntry {
                file: fname.to_string(),
                line: line_no,
                message: format!("unused signal '{}' — tidak pernah direferensikan", name),
                level: DiagLevel::Warning,
                fix: Some(QuickFix {
                    action: format!("Hapus '{}'", name),
                    kind: QuickFixKind::RemoveLine,
                }),
            });
        }
    }
}

/// Deteksi assignment blocking `=` di dalam blok sequential. Heuristik: baris
/// `always_ff` / `always @(posedge|negedge ...)` memulai blok; di dalamnya,
/// baris dengan `=` (bukan `<=`/`==`/dll, via `blocking_assign_pos`) di-flag
/// sebagai calon bug — ganti `=` → `<=` via Quick Fix.
fn lint_blocking_in_sequential(text: &str, fname: &str, out: &mut Vec<DiagEntry>) {
    let mut in_sequential = false;
    let mut block_depth: usize = 0;
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let code = raw.split("//").next().unwrap_or(raw);
        let trimmed = code.trim();
        if trimmed.is_empty() {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let first = words.first().copied().unwrap_or("");

        // Mulai blok sequential: `always_ff ...` atau `always @(posedge ...)`.
        if !in_sequential
            && (first == "always_ff"
                || (first == "always"
                    && (trimmed.contains("posedge") || trimmed.contains("negedge"))))
        {
            in_sequential = true;
            block_depth = 0;
        }
        if !in_sequential {
            continue;
        }

        // Hitung kedalaman begin/end pada baris ini (sebelum cek assignment).
        for w in &words {
            if *w == "begin" {
                block_depth += 1;
            } else if *w == "end" {
                block_depth = block_depth.saturating_sub(1);
            }
        }

        // Assignment blocking di baris ini? (Lewati baris deklarasi `;` awal
        // blok — baris selalu `always_ff ... begin` tidak punya `=`.)
        if block_depth > 0 && trimmed.contains('=') {
            if let Some(_pos) = blocking_assign_pos(trimmed) {
                out.push(DiagEntry {
                    file: fname.to_string(),
                    line: line_no,
                    message: "blocking assignment '=' di dalam blok sequential (gunakan '<=')"
                        .into(),
                    level: DiagLevel::Warning,
                    fix: Some(QuickFix {
                        action: "Ubah '=' → '<='".into(),
                        kind: QuickFixKind::BlockingToNonBlocking,
                    }),
                });
            }
        }

        if block_depth == 0 {
            in_sequential = false;
        }
    }
}

/// Apakah token adalah keyword SystemVerilog (bukan nama signal valid).
fn is_sv_keyword(w: &str) -> bool {
    matches!(
        w,
        "module"
            | "endmodule"
            | "interface"
            | "endinterface"
            | "package"
            | "endpackage"
            | "always"
            | "always_ff"
            | "always_comb"
            | "always_latch"
            | "initial"
            | "final"
            | "begin"
            | "end"
            | "if"
            | "else"
            | "for"
            | "while"
            | "repeat"
            | "case"
            | "casez"
            | "casex"
            | "default"
            | "input"
            | "output"
            | "inout"
            | "parameter"
            | "localparam"
            | "genvar"
            | "assign"
            | "function"
            | "endfunction"
            | "task"
            | "endtask"
            | "typedef"
            | "enum"
            | "struct"
            | "union"
            | "class"
            | "endclass"
            | "return"
            | "break"
            | "continue"
            | "logic"
            | "reg"
            | "wire"
            | "bit"
            | "int"
            | "integer"
            | "byte"
            | "shortint"
            | "longint"
            | "time"
            | "real"
            | "tri"
            | "signed"
            | "unsigned"
            | "var"
            | "void"
            | "import"
            | "export"
    )
}

/// Tipe display signal dari `SignalKind` (untuk Hover tooltip editor).
fn signal_type_str(kind: mivon_ir::SignalKind) -> &'static str {
    match kind {
        mivon_ir::SignalKind::Wire => "wire",
        mivon_ir::SignalKind::Reg => "reg",
        mivon_ir::SignalKind::Logic => "logic",
        mivon_ir::SignalKind::Input => "input",
        mivon_ir::SignalKind::Output => "output",
        mivon_ir::SignalKind::Inout => "inout",
    }
}

/// Hitung jumlah instansiasi per module name di seluruh design (top + semua
/// module lain). Dipakai Code Lens — dihitung sekali saat compile.
pub fn build_ref_counts(design: &IrDesign) -> std::collections::HashMap<String, usize> {
    let mut refs: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut count_insts = |module: &mivon_ir::IrModule| {
        for inst in &module.sub_instances {
            let n = inst.module_name.to_string();
            *refs.entry(n).or_insert(0) += 1;
        }
    };
    count_insts(&design.top);
    for m in design.modules.values() {
        // Guard: jangan hitung dua kali bila top module juga ada di design.modules.
        if m.name != design.top.name {
            count_insts(m);
        }
    }
    refs
}

/// Bangun graf dependency module → (child_module, count) dari design ter-elaborasi.
/// Hanya module yang punya instance yang dimasukkan; count = jumlah instance.
pub fn build_dep_graph(design: &IrDesign) -> Vec<DepRow> {
    let mut rows: Vec<DepRow> = Vec::new();
    let top_name = design.top.name.to_string();

    // Kumpulkan semua module (top + yang ada di design.modules)
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(top_name.clone());
    for name in design.modules.keys() {
        seen.insert(name.to_string());
    }

    let mut module_list: Vec<String> = seen.into_iter().collect();
    module_list.sort();

    for name in module_list {
        let module = if name == top_name {
            &design.top
        } else {
            match design.modules.get(&mivon_core::Symbol::intern(&name)) {
                Some(m) => m,
                None => continue,
            }
        };
        if module.sub_instances.is_empty() {
            continue;
        }
        let mut children: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for inst in &module.sub_instances {
            let child = inst.module_name.to_string();
            *children.entry(child).or_insert(0) += 1;
        }
        let mut children: Vec<(String, usize)> = children.into_iter().collect();
        children.sort_by(|a, b| a.0.cmp(&b.0));
        rows.push(DepRow {
            module: name,
            children,
        });
    }
    rows
}

/// Jalankan perintah shell di worker thread; stdout/stderr di-stream baris per
/// baris ke channel (`TermOutput`), dan `TermExit(code)` dikirim saat selesai.
/// `cwd` = direktori kerja (project root). Aman dipanggil berkali-kali — tiap
/// pemanggilan membuat child process sendiri.
pub fn spawn_term(tx: Sender<GuiEvent>, cmd: String, cwd: Option<PathBuf>) {
    spawn_worker(move || {
        let mut child = match std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .current_dir(cwd.unwrap_or_else(std::env::temp_dir))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(GuiEvent::TermOutput(format!("error: {}", e), true));
                let _ = tx.send(GuiEvent::TermExit(1));
                return;
            }
        };

        // Pipe stdout & stderr secara bersamaan ke channel. `tx` asli
        // dipakai untuk TermExit setelah reader selesai (jangan dipindah ke
        // thread reader — akan jadi use-after-move).
        let out = child.stdout.take();
        let err = child.stderr.take();
        let (tx_out, tx_err) = (tx.clone(), tx.clone());
        if let Some(out) = out {
            std::thread::Builder::new()
                .name("mivon-term-in".into())
                .spawn(move || {
                    use std::io::BufRead;
                    for l in std::io::BufReader::new(out).lines() {
                        let Ok(l) = l else { break };
                        let _ = tx_out.send(GuiEvent::TermOutput(l, false));
                    }
                })
                .ok();
        }
        if let Some(err) = err {
            std::thread::Builder::new()
                .name("mivon-term-err".into())
                .spawn(move || {
                    use std::io::BufRead;
                    for l in std::io::BufReader::new(err).lines() {
                        let Ok(l) = l else { break };
                        let _ = tx_err.send(GuiEvent::TermOutput(l, true));
                    }
                })
                .ok();
        }
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = tx.send(GuiEvent::TermExit(code));
    });
}

/// Jalankan simulasi di worker thread. `cancel` adalah flag bersama dengan GUI:
/// jika di-set true (tombol Stop), simulasi berhenti lebih awal.
pub fn spawn_sim(tx: Sender<GuiEvent>, design: IrDesign, max_time: u64, cancel: Arc<AtomicBool>) {
    spawn_worker(move || {
        let result = run_simulation(&design, max_time, cancel);
        let _ = tx.send(GuiEvent::SimDone(result));
    });
}

/// Jalankan simulasi pada design ter-elaborasi. Mengembalikan nilai akhir signal
/// plus trace waveform (ditangkap via VCD temporer → di-parse).
pub fn run_simulation(
    design: &IrDesign,
    max_time: u64,
    cancel: Arc<AtomicBool>,
) -> Result<SimInfo, String> {
    let start = std::time::Instant::now();
    let mut engine = SimulationEngine::new(design.clone(), max_time);
    engine.cancel_flag = Some(cancel.clone());

    // ── Waveform capture: VCD temporer yang ditulis engine per time step,
    // lalu di-parse menjadi trace transisi untuk Waveform viewer. File dihapus
    // setelah dibaca — tidak ada artefak tersisa. Gagal VCD ≠ gagal sim. ──
    let vcd_path = std::env::temp_dir().join(format!(
        "mivon_gui_{}_{}.vcd",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let vcd_path_str = vcd_path.to_string_lossy().to_string();
    match mivon_simulator::waveform::vcd::VcdWriter::new(&vcd_path_str, design) {
        Ok(mut vcd) => {
            // Batasi ukuran dump (anti-bloat untuk desain besar).
            vcd.max_dump_size = Some(256 * 1024 * 1024);
            engine.set_vcd(vcd);
        }
        Err(e) => {
            eprintln!("waveform capture skipped: {}", e);
        }
    }

    let run_result = engine.run();

    // ── Baca VCD → trace waveform, lalu bersihkan file temporer. Cleanup
    // dijalankan di SEMUA jalur (sukses, error sim, maupun Stop) supaya file
    // .vcd tidak menumpuk di /tmp. ──
    let waveform = if run_result.is_ok() && !engine.is_cancelled() {
        std::fs::read_to_string(&vcd_path)
            .map(|text| parse_vcd(&text))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let _ = std::fs::remove_file(&vcd_path);

    run_result.map_err(|e| e.to_string())?;
    if engine.is_cancelled() {
        return Err("Simulasi dihentikan (Stop)".into());
    }

    let signals: Vec<SignalRow> = engine
        .design
        .top
        .signals
        .iter()
        .enumerate()
        .map(|(id, sig)| {
            let lv = engine.state.read_signal(id);
            SignalRow {
                name: sig.name.to_string(),
                width: lv.width,
                value: logicvec_to_hex(lv),
                kind: format!("{:?}", sig.kind),
            }
        })
        .collect();

    let counters = &engine.sim_perf.counters;

    // ── Coverage: ringkasan dari coverage_stats() + detail covergroup ──
    let cov = engine.coverage_stats();
    let mut covergroups: Vec<CovergroupRow> = Vec::new();
    for cg in &engine.design.covergroups {
        let mut total = 0u64;
        let mut hits = 0u64;
        for cp in &cg.coverpoints {
            let key = mivon_core::Symbol::intern(&format!("{}.{}", cg.name, cp.name));
            total += engine.cover_total.get(&key).copied().unwrap_or(0);
            hits += engine.cover_hits.get(&key).copied().unwrap_or(0);
        }
        for cross in &cg.crosses {
            let key = mivon_core::Symbol::intern(&format!("{}.{}", cg.name, cross.name));
            total += engine.cover_total.get(&key).copied().unwrap_or(0);
            hits += engine.cover_hits.get(&key).copied().unwrap_or(0);
        }
        covergroups.push(CovergroupRow {
            name: cg.name.to_string(),
            total,
            hits,
        });
    }
    let coverage = CoverageInfo {
        line_items: cov.get("line_items").copied().unwrap_or(0.0) as u64,
        line_hits: cov.get("line_total_hits").copied().unwrap_or(0.0) as u64,
        toggle_signals: cov.get("toggle_signals").copied().unwrap_or(0.0) as u64,
        toggle_transitions: cov.get("toggle_transitions").copied().unwrap_or(0.0) as u64,
        branch_total: cov.get("branch_total").copied().unwrap_or(0.0) as u64,
        branch_covered: cov.get("branch_covered").copied().unwrap_or(0.0) as u64,
        branch_percent: cov.get("branch_percent").copied().unwrap_or(0.0),
        fsm_signals: cov.get("fsm_signals").copied().unwrap_or(0.0) as u64,
        fsm_states: cov.get("fsm_states").copied().unwrap_or(0.0) as u64,
        covergroups,
    };

    // ── Assertions: pass/fail per lokasi dari engine.assertion_stats (keyed
    // (line, col) → (pass, fail)). Urut: gagal terbanyak dulu, lalu total. ──
    let mut assertions: Vec<AssertionRow> = engine
        .assertion_stats
        .iter()
        .map(|((l, c), (p, f))| AssertionRow {
            line: *l,
            col: *c,
            pass: *p,
            fail: *f,
        })
        .collect();
    assertions.sort_by(|a, b| {
        b.fail
            .cmp(&a.fail)
            .then_with(|| (b.pass + b.fail).cmp(&(a.pass + a.fail)))
            .then_with(|| a.line.cmp(&b.line).then_with(|| a.col.cmp(&b.col)))
    });

    Ok(SimInfo {
        signals,
        cycles: engine.current_time,
        sim_time_ms: start.elapsed().as_secs_f64() * 1000.0,
        waveform,
        delta_cycles: counters.delta_cycles,
        events_processed: counters.events_processed,
        processes_evaluated: counters.processes_evaluated,
        nba_commits: counters.nba_commits,
        sensitive_triggers: counters.sensitive_triggers,
        events_per_delta: engine.sim_perf.events_per_delta(),
        coverage,
        assertions,
    })
}

/// Resolve file asal baris assertion (1-based, baris mengandung "assert") di
/// antara file module ter-compile. HANYA mengembalikan file bila EXAK SATU
/// file yang cocok — ambiguous atau tidak ada → None (jangan menebak lokasi).
/// Dipanggil saat klik baris assertion di panel (file module tersedia di GUI
/// thread via `CompileInfo.module_files`).
pub fn resolve_assert_file(
    module_files: &HashMap<String, PathBuf>,
    line: usize,
) -> Option<PathBuf> {
    // Dedupe: module berbeda bisa menunjuk file yang sama (hindari false
    // ambiguous), baca tiap file sekali.
    let mut uniq: Vec<(PathBuf, String)> = Vec::new();
    {
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for path in module_files.values() {
            if seen.insert(path.clone()) {
                if let Ok(text) = std::fs::read_to_string(path) {
                    uniq.push((path.clone(), text));
                }
            }
        }
    }
    resolve_assert_in_sources(&uniq, line)
}

/// Inti resolusi (testable, tanpa I/O): cari file yang baris `line`-nya
/// mengandung "assert"; 0 atau >1 kecocokan → None (ambiguous).
fn resolve_assert_in_sources(sources: &[(PathBuf, String)], line: usize) -> Option<PathBuf> {
    let mut cand: Option<PathBuf> = None;
    for (path, text) in sources {
        let Some(l) = text.lines().nth(line.saturating_sub(1)) else {
            continue;
        };
        if !l.contains("assert") {
            continue;
        }
        if cand.is_some() {
            return None;
        }
        cand = Some(path.clone());
    }
    cand
}

/// Parse konten VCD (format yang ditulis `VcdWriter`) menjadi trace transisi
/// per signal. Nilai VCD: `$var wire <w> <code> <name> ... $end`, `#<time>`,
/// `<v><code>` (1-bit) dan `b<value> <code>` (multi-bit). Nilai bertahan sampai
/// transisi berikut — sudah cukup untuk rendering waveform.
pub fn parse_vcd(text: &str) -> Vec<WaveformSignal> {
    let mut vars: HashMap<String, (String, usize)> = HashMap::new();
    let mut traces: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    let mut cur_time: u64 = 0;
    // Scope stack: VcdWriter menulis `$scope module <top> $end` lalu scope
    // hierarkis per sinyal. Scope pertama = modul top (dilewati — nama sinyal
    // top-level adalah nama bare); scope berikutnya digabung utk nama penuh
    // (mis. "u1.q") agar konsisten dgn tab Signals.
    let mut scope: Vec<String> = Vec::new();
    let mut seen_top_scope = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("$scope ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                if !seen_top_scope {
                    seen_top_scope = true; // modul top — jangan jadi prefix nama
                } else {
                    scope.push(parts[1].to_string());
                }
            }
            continue;
        }
        if line.starts_with("$upscope") {
            scope.pop();
            continue;
        }
        if let Some(rest) = line.strip_prefix("$var ") {
            // "wire <w> <code> <name> [range] $end" (range hilang utk width 1)
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 4 {
                if let Ok(width) = parts[1].parse::<usize>() {
                    let code = parts[2].to_string();
                    let bare = parts[3].to_string();
                    let name = if scope.is_empty() {
                        bare
                    } else {
                        format!("{}.{}", scope.join("."), bare)
                    };
                    vars.insert(code.clone(), (name, width));
                    traces.entry(code).or_default();
                }
            }
            continue;
        }
        if let Some(t) = line.strip_prefix('#') {
            cur_time = t.trim().parse().unwrap_or(cur_time);
            continue;
        }
        if let Some(b) = line.strip_prefix('b') {
            // b<value> <code>
            if let Some((val, code)) = b.split_once(char::is_whitespace) {
                let code = code.trim();
                if !code.is_empty() {
                    traces
                        .entry(code.to_string())
                        .or_default()
                        .push((cur_time, val.trim().to_string()));
                }
            }
            continue;
        }
        // 1-bit: "0s0" / "1s0" / "xs0" / "zs0"
        if line.len() >= 2 {
            let (val, code) = line.split_at(1);
            if matches!(val, "0" | "1" | "x" | "z" | "X" | "Z") && !code.trim().is_empty() {
                traces
                    .entry(code.trim().to_string())
                    .or_default()
                    .push((cur_time, val.to_string()));
            }
        }
    }

    let mut out: Vec<WaveformSignal> = vars
        .into_iter()
        .map(|(code, (name, width))| {
            let mut trace = traces.remove(&code).unwrap_or_default();
            trace.sort_by_key(|(t, _)| *t);
            // Dedupe nilai berurutan yang sama (pertahankan waktu terakhir)
            let mut dedup: Vec<(u64, String)> = Vec::with_capacity(trace.len());
            for (t, v) in trace {
                match dedup.last_mut() {
                    Some((lt, lv)) if *lv == v => *lt = t,
                    _ => dedup.push((t, v)),
                }
            }
            WaveformSignal {
                name,
                width,
                trace: dedup,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Konversi LogicVec ke hex string (mendukung X/Z dan width > 64-bit).
pub fn logicvec_to_hex(lv: &LogicVec) -> String {
    if lv.width == 0 {
        return "0".into();
    }
    let all_x = lv.bits.iter().all(|b| *b == LogicVal::X);
    let all_z = lv.bits.iter().all(|b| *b == LogicVal::Z);
    if all_x {
        return format!("'x{}", lv.width);
    }
    if all_z {
        return format!("'z{}", lv.width);
    }
    let nibbles = lv.width.div_ceil(4);
    let mut hex = String::with_capacity(nibbles);
    for nib in (0..nibbles).rev() {
        let mut val = 0u8;
        let mut has_x = false;
        let mut has_z = false;
        for bit in 0..4 {
            let idx = nib * 4 + bit;
            if idx < lv.width {
                match lv.bits[idx] {
                    LogicVal::One => val |= 1 << bit,
                    LogicVal::X => has_x = true,
                    LogicVal::Z => has_z = true,
                    LogicVal::Zero => {}
                }
            }
        }
        if has_x {
            hex.push('x');
        } else if has_z {
            hex.push('z');
        } else {
            hex.push(std::char::from_digit(val as u32, 16).unwrap_or('0'));
        }
    }
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.to_string()
    }
}

/// Serialisasi trace waveform (hasil sim) ke teks VCD — kebalikan dari
/// `parse_vcd`. Semua signal ditulis dalam satu scope `module top`; nama
/// signal dipertahankan apa adanya (scope bertitik ikut sebagai nama, konsisten
/// dengan penyebutan tab Waveform). Dipakai tombol "⇓ VCD" (export).
pub fn signals_to_vcd(signals: &[WaveformSignal]) -> String {
    let mut out = String::new();

    // Header VCD.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    out.push_str("$date\n  ");
    out.push_str(&now.to_string());
    out.push_str("\n$end\n");
    out.push_str("$version Mivon GUI\n$end\n");
    out.push_str("$timescale 1ns $end\n");
    out.push_str("$scope module top $end\n");
    // Kode identifier per signal: "s<i>" (unik, urut index).
    for (i, s) in signals.iter().enumerate() {
        let w = s.width.max(1);
        out.push_str(&format!("$var wire {} s{} {} $end\n", w, i, s.name));
    }
    out.push_str("$upscope $end\n");
    out.push_str("$enddefinitions $end\n");

    // Saluran tanpa transisi: definisikan biner 0 di t=0 agar tetap terlihat
    // di viewer. Signal ber-trace: tulis SEMUA transisi (termasuk t=0) —
    // nilai berlaku sejak timestamp-nya; nilai t=0 yang berbeda dari 0 tidak
    // boleh dibuang (round-trip parse_vcd).
    for (i, s) in signals.iter().enumerate() {
        if s.trace.is_empty() {
            out.push_str("#0\n");
            out.push_str(&format!("b{} s{}\n", "0".repeat(s.width.max(1)), i));
        }
    }
    for (i, s) in signals.iter().enumerate() {
        let code = format!("s{}", i);
        for (t, v) in &s.trace {
            out.push_str(&format!("#{}\n", t));
            if s.width == 1 {
                out.push_str(&format!("{}{}\n", v, code));
            } else {
                out.push_str(&format!("b{} {}\n", v, code));
            }
        }
    }
    out
}

/// Pencarian teks (case-insensitive) di file RTL/header/mv proyek. Kap maksimal
/// `max_results` hasil & lewati file tak terbaca. Dipakai tab Search → Text;
/// klik hasil → buka file & lompat baris.
pub fn search_text_in_files(files: &[PathBuf], needle: &str, max_results: usize) -> Vec<TextHit> {
    let q = needle.trim().to_lowercase();
    if q.is_empty() || max_results == 0 {
        return Vec::new();
    }
    let mut out: Vec<TextHit> = Vec::new();
    'file: for p in files {
        if out.len() >= max_results {
            break;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "sv" | "svh" | "v" | "vh" | "mv" | "mvh") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        for (i, raw) in text.lines().enumerate() {
            if out.len() >= max_results {
                continue 'file;
            }
            if raw.to_lowercase().contains(&q) {
                out.push(TextHit {
                    file: p.clone(),
                    line: i + 1,
                    text: raw.trim().to_string(),
                });
            }
        }
    }
    out
}

/// Flatten trace waveform → daftar event perubahan nilai, urut waktu.
/// Entry trace pertama (t=0) = nilai INISIAL, bukan perubahan — dibuang.
/// Entry berikutnya = nilai baru yang berlaku sejak `t`. Fakta dari trace
/// asli (untuk tab Trace), bukan dugaan sebab-akibat.
pub fn flatten_events(signals: &[WaveformSignal]) -> Vec<TraceEvent> {
    let mut out: Vec<TraceEvent> = Vec::new();
    for s in signals {
        for (i, (t, v)) in s.trace.iter().enumerate() {
            if i == 0 {
                continue;
            }
            out.push(TraceEvent {
                name: s.name.clone(),
                width: s.width,
                t: *t,
                value: v.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.t.cmp(&b.t).then_with(|| a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(name: &str, width: usize, trace: Vec<(u64, &str)>) -> WaveformSignal {
        WaveformSignal {
            name: name.into(),
            width,
            trace: trace.into_iter().map(|(t, v)| (t, v.to_string())).collect(),
        }
    }

    #[test]
    fn signals_to_vcd_roundtrip_via_parse() {
        // write → parse harus mengembalikan trace yang sama (nama/lebar/transisi)
        // — kebalikan dari parse_vcd, kunci format yang benar.
        let sigs = vec![
            wf("clk", 1, vec![(0, "0"), (5, "1"), (10, "0")]),
            wf("data", 4, vec![(0, "0000"), (7, "1010")]),
        ];
        let vcd = signals_to_vcd(&sigs);
        let back = parse_vcd(&vcd);
        assert_eq!(back.len(), 2, "dua signal kembali:\n{}", vcd);
        let clk = back.iter().find(|s| s.name == "clk").expect("clk");
        assert_eq!(clk.width, 1);
        assert_eq!(clk.trace, sigs[0].trace);
        let data = back.iter().find(|s| s.name == "data").expect("data");
        assert_eq!(data.width, 4);
        assert_eq!(data.trace, sigs[1].trace);
    }

    #[test]
    fn signals_to_vcd_has_header_and_single_scope() {
        let sigs = vec![wf("q", 1, Vec::new())];
        let vcd = signals_to_vcd(&sigs);
        assert!(vcd.contains("$timescale 1ns $end"));
        assert!(vcd.contains("$var wire 1 s0 q $end"));
        assert!(vcd.contains("$enddefinitions $end"));
        assert_eq!(vcd.matches("$scope").count(), 1, "satu scope top");
        assert!(vcd.contains("#0\n"));
    }

    #[test]
    fn signals_to_vcd_preserves_zero_time_value() {
        // Nilai t=0 yang BUKAN 0 harus dipertahankan (tidak boleh diganti
        // biner 0 implisit) — round-trip parse harus identik.
        let sigs = vec![wf("q", 1, vec![(0, "1"), (3, "0")])];
        let back = parse_vcd(&signals_to_vcd(&sigs));
        let q = back.iter().find(|s| s.name == "q").expect("q");
        assert_eq!(q.trace, sigs[0].trace);
    }

    // ── Resolver file assertion ──

    fn src(path: &str, text: &str) -> (PathBuf, String) {
        (PathBuf::from(path), text.to_string())
    }

    #[test]
    fn resolve_assert_finds_single_unambiguous_file() {
        let sources = vec![
            src("/proj/alu.sv", "module alu;\n  assert (a);\nendmodule\n"),
            src("/proj/cache.sv", "module cache;\n  logic q;\nendmodule\n"),
        ];
        assert_eq!(
            resolve_assert_in_sources(&sources, 2),
            Some(PathBuf::from("/proj/alu.sv")),
        );
    }

    #[test]
    fn resolve_assert_none_when_no_match() {
        let sources = vec![
            src("/proj/alu.sv", "module alu;\n  logic q;\nendmodule\n"),
            src("/proj/cache.sv", "module cache;\n  logic w;\nendmodule\n"),
        ];
        assert_eq!(resolve_assert_in_sources(&sources, 2), None);
    }

    #[test]
    fn resolve_assert_ambiguous_when_two_files_match() {
        // Dua file punya "assert" di baris yang sama → tak bisa dipastikan.
        let sources = vec![
            src("/proj/a.sv", "module a;\n  assert (x);\nendmodule\n"),
            src("/proj/b.sv", "module b;\n  assert (y);\nendmodule\n"),
        ];
        assert_eq!(resolve_assert_in_sources(&sources, 2), None);
    }

    #[test]
    fn resolve_assert_line_out_of_range_none() {
        let sources = vec![src("/proj/a.sv", "module a;\n  logic q;\nendmodule\n")];
        assert_eq!(resolve_assert_in_sources(&sources, 99), None);
    }

    // ── Flatten event trace ──

    #[test]
    fn flatten_events_skips_initial_value() {
        // Trace hanya berisi nilai inisial (t=0) → tidak ada event.
        let sigs = vec![wf("clk", 1, vec![(0, "0")])];
        assert!(flatten_events(&sigs).is_empty());
    }

    #[test]
    fn flatten_events_lists_changes_sorted_by_time() {
        let sigs = vec![
            wf("clk", 1, vec![(0, "0"), (5, "1"), (10, "0")]),
            wf("data", 4, vec![(0, "0000"), (7, "1010")]),
        ];
        let ev = flatten_events(&sigs);
        assert_eq!(ev.len(), 3);
        // Urut: t=5 clk, t=7 data, t=10 clk.
        assert_eq!(ev[0].t, 5);
        assert_eq!(ev[0].name, "clk");
        assert_eq!(ev[0].value, "1");
        assert_eq!(ev[1].t, 7);
        assert_eq!(ev[1].name, "data");
        assert_eq!(ev[1].width, 4);
        assert_eq!(ev[1].value, "1010");
        assert_eq!(ev[2].t, 10);
        assert_eq!(ev[2].name, "clk");
        assert_eq!(ev[2].value, "0");
    }

    #[test]
    fn flatten_events_empty_input_empty() {
        assert!(flatten_events(&[]).is_empty());
    }

    // ── scan_tree vs symlink-loop ──

    #[cfg(unix)]
    #[test]
    fn scan_tree_tolerates_symlink_loops() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("mivon_gui_scan_{}", std::process::id()));
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("sig.sv"), "module sig;\nendmodule\n").unwrap();
        symlink(&base, base.join("loop")).unwrap(); // loop → dir induk (siklus)
        symlink(&real, base.join("lib")).unwrap(); // aman → subdir

        let tree = scan_tree(&base);

        // Tidak stack-overflow; direktori nyata tetap eksplor.
        assert!(tree.iter().any(|n| n.name == "real" && n.is_dir));
        let real_node = tree.iter().find(|n| n.name == "real").unwrap();
        assert!(
            real_node.children.iter().any(|c| c.name == "sig.sv"),
            "file di dalam direktori nyata terlihat"
        );
        // Symlink-loop dipotong — anaknya tidak direkursi tak berujung.
        let loop_node = tree.iter().find(|n| n.name == "loop").unwrap();
        assert!(
            loop_node.children.is_empty(),
            "symlink-loop tidak boleh di-rekursi"
        );
        // Symlink aman ke subdir tetap dieksplor.
        let lib_node = tree.iter().find(|n| n.name == "lib").unwrap();
        assert!(lib_node.is_dir && !lib_node.children.is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }
}
