//! Mivon HDL (.mv) — bahasa baru milik Mivon untuk menulis RTL yang lebih bersih,
//! di-transpile ke SystemVerilog `.sv`/`.svh` oleh tool `mivon mgen`.
//!
//! Pipeline: `.mv` → lexer → parser → AST → check → codegen → `.sv` + `.svh`
//! (lihat MIVON-HDL.md untuk spesifikasi bahasa).
//!
//! 1 file = 1 tanggung jawab:
//! - `lexer.rs`  — tokenizer
//! - `parser.rs` — recursive descent → `ast::MvFile`
//! - `check.rs`  — type-check & semantic (E2001–E2007, MIVON-HDL.md §9)
//! - `codegen.rs`— emitter SystemVerilog (`.sv`/`.svh`)

// Allow complex Result/function types (intentional API signatures)
#![allow(clippy::type_complexity)]

pub mod ast;
pub mod check;
pub mod codegen;
pub mod lexer;
pub mod parser;
pub mod print;

use crate::ast::MvFile;
use std::fmt;

/// Lebar **index MSB** enum SV untuk `n` anggota: `clog2(n)`, minimal 1.
///
/// `enum_bits` adalah MSB, BUKAN jumlah bit. Emitter menulis
/// `typedef enum logic [enum_bits(n):0]` → total `enum_bits(n) + 1` bit
/// (3 anggota → `logic [1:0]`, cukup untuk 0..3). Pemanggil yang butuh
/// JUMLAH bit harus memakai `enum_width`.
pub(crate) fn enum_bits(n: usize) -> i64 {
    if n <= 2 {
        1
    } else {
        ((n - 1) as f64).log2().ceil() as i64
    }
}

/// Lebar total (jumlah bit) enum SV untuk `n` anggota — selalu sinkron
/// dengan emisi `codegen/defs.rs`. Pakai ini di semua_places yang butuh
/// "berapa bit sinyal enum ini", bukan `enum_bits`.
pub(crate) fn enum_width(n: usize) -> i64 {
    enum_bits(n) + 1
}

/// Error parse/lex Mivon HDL dengan posisi (line, col).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MvError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl MvError {
    pub fn new(line: usize, col: usize, msg: String) -> Self {
        MvError { line, col, msg }
    }

    /// Format ringkas `line:col: pesan`. Sejak F11 semua error (lexer, parser,
    /// DAN type-check E2001–E2007) membawa posisi — format ini selalu
    /// menampilkan `line:col:`.
    pub fn format(&self) -> String {
        if self.line == 0 && self.col == 0 {
            self.msg.clone()
        } else {
            format!("{}:{}: {}", self.line, self.col, self.msg)
        }
    }
}

impl fmt::Display for MvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format())
    }
}

impl std::error::Error for MvError {}

/// Format error dengan snippet source (F11): ringkasan `path: line:col: msg`
///     + baris sumber untuk konteks, caret menunjuk posisi persis (gaya rustc).
///     `line`/`col` 1-based; 0 berarti tanpa posisi (tidak tampil snippet).
///     Dipakai `mgen` (src/tools/gen.rs) dan `run` (src/main.rs) untuk UX error
///     yang konsisten.
///
/// Batasan (sama seperti `raw_slice` di parser): `col` lexer dihitung per
/// CHAR sedangkan padding caret memakai spasi — baris yang memuat TAB atau
/// karakter multi-byte (non-ASCII) SEBELUM titik error membuat caret
/// meleset. Praktis tidak terjadi di .mv (ASCII + spasi); dibiarkan agar
/// sederhana.
pub fn format_error(path: &str, src: &str, e: &MvError) -> String {
    let mut out = format!("{path}: {}", e.format());
    if e.line > 0 && e.col > 0 {
        if let Some(line) = src.lines().nth(e.line - 1) {
            out.push_str(&format!("\n  --> {path}:{}:{}", e.line, e.col));
            out.push_str("\n   |");
            out.push_str(&format!("\n{:>4} | {}", e.line, line));
            // caret: col 1-based → (col-1) spasi; tab tidak diganti (jarang
            // dipakai di .mv; offset tetap 1 kolom per karakter source).
            let pad = " ".repeat(e.col.saturating_sub(1));
            out.push_str(&format!("\n     | {pad}^"));
        }
    }
    out
}

/// Hasil transpile satu file `.mv`/`.mvh`.
#[derive(Debug, Clone)]
pub struct TranspileResult {
    /// Konten `.sv` (module/program/class/function/task) — kosong untuk
    /// sumber `.mvh` (header-only, F43).
    pub sv: String,
    /// Konten `.svh` (package/typedef/interface + include guard)
    pub svh: String,
}

/// Satu item input batch transpile (F43): sumber + nama base + flag header.
/// `header = true` untuk sumber `.mvh` — wajib hanya berisi definisi bersama,
/// output-nya `.svh` saja.
#[derive(Debug, Clone)]
pub struct MvItem {
    pub src: String,
    pub base: String,
    pub header: bool,
    /// `mgen --package <nama>`: bungkus typedef level file dalam package
    /// bernama tsb (MIVON-HDL.md §11). `None` = emisi apa adanya ($unit).
    pub package: Option<String>,
}

impl MvItem {
    /// Item sumber `.mv` biasa (module/testbench → `.sv` + `.svh`).
    pub fn new(src: impl Into<String>, base: impl Into<String>) -> Self {
        MvItem {
            src: src.into(),
            base: base.into(),
            header: false,
            package: None,
        }
    }

    /// Item sumber `.mvh` (header Mivon HDL → `.svh` saja).
    pub fn header(src: impl Into<String>, base: impl Into<String>) -> Self {
        MvItem {
            src: src.into(),
            base: base.into(),
            header: true,
            package: None,
        }
    }

    /// Set flag header (F43) pada item yang sudah dibangun.
    pub fn with_header(mut self, header: bool) -> Self {
        self.header = header;
        self
    }
}

/// Transpile source `.mv` → `.sv` + `.svh`.
/// `base` = nama file tanpa ekstensi (mis. `counter` dari `counter.mv`).
/// Type-check dijalankan SEBELUM emisi — error E2001–E2007 muncul di level
/// `.mv`, bukan di SV hasil generate (MIVON-HDL.md §9, prinsip desain #4).
pub fn transpile(src: &str, base: &str) -> Result<TranspileResult, MvError> {
    let file = parser::parse(src)?;
    check::check(&file)?;
    generate_from(&file, base, &[], "mv", None, &[])
}

/// Transpile source `.mvh` (header Mivon HDL, F43) → `.svh` saja.
/// Kontrak: sumber `.mvh` HANYA boleh berisi definisi bersama (typedef/
/// package/interface). Konten fungsional (module/program/class/func/task)
/// → error **E2008** berposisi di nama pelanggar (MIVON-HDL.md §9).
pub fn transpile_header(src: &str, base: &str) -> Result<TranspileResult, MvError> {
    let file = parser::parse(src)?;
    check::check(&file)?;
    validate_header_only(&file)?;
    generate_from(&file, base, &[], "mvh", None, &[])
}

/// Transpile TANPA type-check (escape hatch `mgen --no-check` — untuk kode
/// yang memakai konstruk eksternal yang belum dipahami checker).
/// Validasi header-only `.mvh` (E2008) TETAP dijalankan — kontrak ekstensi,
/// bukan type-check.
pub fn transpile_no_check(src: &str, base: &str) -> Result<TranspileResult, MvError> {
    let file = parser::parse(src)?;
    generate_from(&file, base, &[], "mv", None, &[])
}

/// `transpile_no_check` untuk sumber `.mvh` — validate E2008 tetap jalan.
pub fn transpile_header_no_check(src: &str, base: &str) -> Result<TranspileResult, MvError> {
    let file = parser::parse(src)?;
    validate_header_only(&file)?;
    generate_from(&file, base, &[], "mvh", None, &[])
}

/// Transpile BEBERAPA file `.mv`/`.mvh` sekaligus dengan KONTEKS GABUNGAN
/// (F9): tipe/package/konstanta dari semua file terlihat oleh semua file,
/// sehingga `use pkg::*` antar-file (`types.mvh` → `counter.mv`) lolos
/// type-check.
///
/// Hasil sejajar dengan `items`. Error pertama di-return bersama indeks item
/// asalnya — pemanggil menyertakan path-nya dalam pesan error.
pub fn transpile_many_items(
    items: &[MvItem],
) -> Result<Vec<TranspileResult>, (usize, MvError)> {
    let files = parse_all(items)?;
    let refs: Vec<&ast::MvFile> = files.iter().collect();
    check::check_many(&refs)?;
    for (i, it) in items.iter().enumerate() {
        if it.header {
            validate_header_only(&files[i]).map_err(|e| (i, e))?;
        }
    }
    generate_all(items, &files)
}

/// `transpile_many_items` tanpa type-check (padanan `--no-check` untuk
/// batch). Validasi header-only `.mvh` (E2008) tetap dijalankan.
pub fn transpile_many_items_no_check(
    items: &[MvItem],
) -> Result<Vec<TranspileResult>, (usize, MvError)> {
    let files = parse_all(items)?;
    for (i, it) in items.iter().enumerate() {
        if it.header {
            validate_header_only(&files[i]).map_err(|e| (i, e))?;
        }
    }
    generate_all(items, &files)
}

/// Transpile batch gaya lama — pasangan (sumber, base), semua `.mv`.
/// Lihat `transpile_many_items` bila campur `.mvh`.
pub fn transpile_many(
    items: &[(String, String)],
) -> Result<Vec<TranspileResult>, (usize, MvError)> {
    let owned: Vec<MvItem> = items
        .iter()
        .map(|(src, base)| MvItem::new(src.clone(), base.clone()))
        .collect();
    transpile_many_items(&owned)
}

/// `transpile_many` tanpa type-check (padanan `--no-check` untuk batch).
pub fn transpile_many_no_check(
    items: &[(String, String)],
) -> Result<Vec<TranspileResult>, (usize, MvError)> {
    let owned: Vec<MvItem> = items
        .iter()
        .map(|(src, base)| MvItem::new(src.clone(), base.clone()))
        .collect();
    transpile_many_items_no_check(&owned)
}

/// Validasi kontrak header `.mvh` (F43, E2008): sumber header hanya boleh
/// berisi definisi bersama (typedef/package/interface). Konten fungsional
/// pertama → error berposisi (line, col) di nama pelanggarnya.
fn validate_header_only(file: &MvFile) -> Result<(), MvError> {
    let offender = file
        .modules
        .first()
        .map(|m| ("module", m.name.as_str(), m.line, m.col))
        .or_else(|| {
            file.programs
                .first()
                .map(|m| ("program", m.name.as_str(), m.line, m.col))
        })
        .or_else(|| {
            file.classes
                .first()
                .map(|c| ("class", c.name.as_str(), c.line, c.col))
        })
        .or_else(|| {
            file.funcs
                .first()
                .map(|f| ("function", f.name.as_str(), f.line, f.col))
        })
        .or_else(|| {
            file.tasks
                .first()
                .map(|t| ("task", t.name.as_str(), t.line, t.col))
        });
    if let Some((kind, name, line, col)) = offender {
        return Err(MvError::new(
            line,
            col,
            format!(
                "[E2008] file .mvh hanya berisi definisi bersama (typedef/package/interface) — ditemukan {kind} '{name}'"
            ),
        ));
    }
    Ok(())
}

fn parse_all(items: &[MvItem]) -> Result<Vec<ast::MvFile>, (usize, MvError)> {
    let mut files = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let f = parser::parse(&it.src).map_err(|e| (i, e))?;
        files.push(f);
    }
    Ok(files)
}

fn generate_all(
    items: &[MvItem],
    files: &[ast::MvFile],
) -> Result<Vec<TranspileResult>, (usize, MvError)> {
    // F26 fix review: nama interface dari SEMUA file (konteks gabungan) —
    // port module bertipe interface yang didefinisikan di file lain tetap
    // di-emit tanpa arah (konsisten dgn check_many). Definisi interface
    // sendiri tetap keluar hanya di .svh file asalnya.
    let all_ifaces: Vec<&str> = files
        .iter()
        .flat_map(|f| f.interfaces.iter().map(|i| i.name.as_str()))
        .collect();
    // F77: owner definisi bersama — file yang me-refer definisi file LAIN
    // mendapat `include "<base>.svh"` supaya output mandiri di tool EDA.
    // Resolusi meniru prioritas check: definisi file sendiri menang atas
    // nama luar; anggota package via `use` ditutup oleh include package-nya.
    let owners = build_owners(files);
    // Base per file untuk emisi include (sejajar dgn items/files).
    let bases: Vec<&str> = items.iter().map(|it| it.base.as_str()).collect();
    let mut out = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let src_ext = if it.header { "mvh" } else { "mv" };
        let includes = cross_file_includes(&files[i], i, &owners, &bases);
        let inc_refs: Vec<&str> = includes.iter().map(|s| s.as_str()).collect();
        let r = generate_from(
            &files[i],
            &it.base,
            &all_ifaces,
            src_ext,
            it.package.as_deref(),
            &inc_refs,
        )
        .map_err(|e| (i, e))?;
        out.push(r);
    }
    Ok(out)
}

/// F77: indeks owner definisi bersama lintas-file.
/// - `pkg_owner`: nama package → file pemilik.
/// - `pkg_members`: nama package → nama typedef anggotanya (untukmembedakan
///   `State` via `use traffic_pkg::*` dari typedef file-level bernama sama).
/// - `filedef_owner`: typedef level file + interface → file pemilik (referensi
///   bare tanpa `use`). Anggota package SENGAJA tidak masuk sini.
/// First-wins bila nama ganda (check_many menolak E2007 lintas-file; jalur
/// no-check konservatif).
struct Owners<'a> {
    pkg_owner: std::collections::HashMap<&'a str, usize>,
    pkg_members: std::collections::HashMap<&'a str, std::collections::HashSet<&'a str>>,
    filedef_owner: std::collections::HashMap<&'a str, usize>,
}

fn build_owners(files: &[ast::MvFile]) -> Owners<'_> {
    let mut o = Owners {
        pkg_owner: std::collections::HashMap::new(),
        pkg_members: std::collections::HashMap::new(),
        filedef_owner: std::collections::HashMap::new(),
    };
    for (i, f) in files.iter().enumerate() {
        for td in &f.typedefs {
            o.filedef_owner.entry(td_name(td)).or_insert(i);
        }
        for p in &f.packages {
            o.pkg_owner.entry(p.name.as_str()).or_insert(i);
            let members = o.pkg_members.entry(p.name.as_str()).or_default();
            for td in &p.typedefs {
                members.insert(td_name(td));
            }
        }
        for ifc in &f.interfaces {
            o.filedef_owner.entry(ifc.name.as_str()).or_insert(i);
        }
    }
    o
}

fn td_name(td: &crate::ast::Typedef) -> &str {
    match td {
        crate::ast::Typedef::Alias { name, .. }
        | crate::ast::Typedef::Struct { name, .. }
        | crate::ast::Typedef::Union { name, .. }
        | crate::ast::Typedef::Enum { name, .. } => name.as_str(),
    }
}

/// F77: base file LAIN yang definisi bersamanya di-refer file ini.
/// Urutan = urutan batch (deterministik), tanpa self, tanpa duplikat.
fn cross_file_includes(
    file: &ast::MvFile,
    self_idx: usize,
    owners: &Owners,
    bases: &[&str],
) -> Vec<String> {
    use std::collections::HashSet;
    // Definisi file sendiri selalu menang (prioritas check).
    let mut own_pkgs: HashSet<&str> = HashSet::new();
    let mut own_filedefs: HashSet<&str> = HashSet::new();
    for p in &file.packages {
        own_pkgs.insert(p.name.as_str());
    }
    for td in &file.typedefs {
        own_filedefs.insert(td_name(td));
    }
    for ifc in &file.interfaces {
        own_filedefs.insert(ifc.name.as_str());
    }
    // Package yang di-`use` file ini (termasuk di dalam generate).
    let mut used_pkgs: HashSet<&str> = HashSet::new();
    for m in file.modules.iter().chain(file.programs.iter()) {
        collect_used_pkgs_in_items(&m.items, &mut used_pkgs);
    }
    let mut type_refs = Vec::new();
    collect_file_refs(file, &mut type_refs);

    let mut seen = HashSet::new();
    let mut found: Vec<(usize, String)> = Vec::new();
    let add = |owner: usize, seen: &mut HashSet<usize>, found: &mut Vec<(usize, String)>| {
        if owner != self_idx && seen.insert(owner) {
            found.push((owner, bases[owner].to_string()));
        }
    };
    // `use pkg::*` → include pemilik package (kecuali milik sendiri).
    for pkg in &used_pkgs {
        if own_pkgs.contains(pkg) {
            continue;
        }
        if let Some(&owner) = owners.pkg_owner.get(pkg) {
            add(owner, &mut seen, &mut found);
        }
    }
    for r in &type_refs {
        if let Some((pkg, _)) = r.split_once("::") {
            // `pkg::item` → pemilik package (kecuali milik sendiri).
            if own_pkgs.contains(pkg) {
                continue;
            }
            if let Some(&owner) = owners.pkg_owner.get(pkg) {
                add(owner, &mut seen, &mut found);
            }
            continue;
        }
        // Nama bare: milik sendiri → skip; anggota package yang di-`use`
        // (milik sendiri maupun luar) → ditutup include package-nya.
        if own_filedefs.contains(r.as_str()) {
            continue;
        }
        let via_use = used_pkgs.iter().any(|pkg| {
            owners
                .pkg_members
                .get(*pkg)
                .is_some_and(|m| m.contains(r.as_str()))
        });
        if via_use {
            continue;
        }
        if let Some(&owner) = owners.filedef_owner.get(r.as_str()) {
            add(owner, &mut seen, &mut found);
        }
    }
    // Urutan batch (indeks owner) — deterministik mengikuti urutan input.
    found.sort_by_key(|(idx, _)| *idx);
    found.into_iter().map(|(_, b)| b).collect()
}

/// F77: kumpulkan package yang di-`use` (`use pkg::*`), termasuk di dalam
/// blok generate.
fn collect_used_pkgs_in_items<'a>(items: &'a [crate::ast::MItem], out: &mut std::collections::HashSet<&'a str>) {
    use crate::ast::MItem;
    for it in items {
        match it {
            MItem::Use { pkg, .. } => {
                out.insert(pkg.as_str());
            }
            MItem::GenFor { body, .. } => collect_used_pkgs_in_items(body, out),
            MItem::GenIf { then, els, .. } => {
                collect_used_pkgs_in_items(then, out);
                collect_used_pkgs_in_items(els, out);
            }
            _ => {}
        }
    }
}

/// F77: kumpulkan semua referensi nama tipe/package dalam satu file:
/// `use pkg::*` + setiap `MvType::Named` di port/sig/reg/wire/const/typedef/
/// func/task/class/interface.
fn collect_file_refs(file: &ast::MvFile, out: &mut Vec<String>) {
    use crate::ast::{MItem, MvType};
    for td in &file.typedefs {
        collect_typedef_refs(td, out);
    }
    for p in &file.packages {
        for td in &p.typedefs {
            collect_typedef_refs(td, out);
        }
        for (_, ty, _) in &p.consts {
            if let Some(t) = ty {
                collect_type_refs(t, out);
            }
        }
    }
    for ifc in &file.interfaces {
        for port in &ifc.ports {
            collect_type_refs(&port.ty, out);
        }
        for (_, ty, _, _) in &ifc.sigs {
            collect_type_refs(ty, out);
        }
    }
    for m in file.modules.iter().chain(file.programs.iter()) {
        for param in &m.params {
            if let Some(t) = &param.ty {
                // Marker `Named("type")` bukan referensi.
                if !matches!(t, MvType::Named(s, ..) if s == "type") {
                    collect_type_refs(t, out);
                }
            }
            if let Some(t) = &param.type_default {
                collect_type_refs(t, out);
            }
        }
        for item in &m.items {
            match item {
                MItem::Port(p) => collect_type_refs(&p.ty, out),
                MItem::Typedef(td) => collect_typedef_refs(td, out),
                MItem::Sig { ty, .. } | MItem::Reg { ty, .. } | MItem::Wire { ty, .. } => {
                    collect_type_refs(ty, out)
                }
                MItem::Const { ty, .. } => {
                    if let Some(t) = ty {
                        collect_type_refs(t, out);
                    }
                }
                MItem::Use { pkg, .. } => out.push(pkg.clone()),
                MItem::Seq(_, body)
                | MItem::Comb(body)
                | MItem::Always(body)
                | MItem::Latch(body)
                | MItem::Initial(body)
                | MItem::Final(body) => collect_stmt_type_refs(body, out),
                MItem::GenFor { body, .. } => {
                    for it in body {
                        collect_mitem_refs(it, out);
                    }
                }
                MItem::GenIf { then, els, .. } => {
                    for it in then.iter().chain(els.iter()) {
                        collect_mitem_refs(it, out);
                    }
                }
                MItem::Func(f) => {
                    for (_, ty, _, _) in &f.args {
                        collect_type_refs(ty, out);
                    }
                    if let Some(t) = &f.ret {
                        collect_type_refs(t, out);
                    }
                    for s in &f.body {
                        collect_stmt_type_refs(s, out);
                    }
                }
                MItem::Task(t) => {
                    for (_, ty, _, _) in &t.args {
                        collect_type_refs(ty, out);
                    }
                    for s in &t.body {
                        collect_stmt_type_refs(s, out);
                    }
                }
                MItem::Inst { .. }
                | MItem::Assign { .. }
                | MItem::AssertProperty(_)
                | MItem::AssumeProperty(_)
                | MItem::CoverProperty(_) => {}
            }
        }
    }
    for c in &file.classes {
        for (_, ty, _) in &c.fields {
            collect_type_refs(ty, out);
        }
        for f in &c.funcs {
            for (_, ty, _, _) in &f.args {
                collect_type_refs(ty, out);
            }
            if let Some(t) = &f.ret {
                collect_type_refs(t, out);
            }
        }
        for t in &c.tasks {
            for (_, ty, _, _) in &t.args {
                collect_type_refs(ty, out);
            }
        }
    }
    for f in &file.funcs {
        for (_, ty, _, _) in &f.args {
            collect_type_refs(ty, out);
        }
        if let Some(t) = &f.ret {
            collect_type_refs(t, out);
        }
    }
    for t in &file.tasks {
        for (_, ty, _, _) in &t.args {
            collect_type_refs(ty, out);
        }
    }
}

fn collect_mitem_refs(item: &crate::ast::MItem, out: &mut Vec<String>) {
    use crate::ast::MItem;
    match item {
        MItem::Port(p) => collect_type_refs(&p.ty, out),
        MItem::Typedef(td) => collect_typedef_refs(td, out),
        MItem::Sig { ty, .. } | MItem::Reg { ty, .. } | MItem::Wire { ty, .. } => {
            collect_type_refs(ty, out)
        }
        MItem::Const { ty, .. } => {
            if let Some(t) = ty {
                collect_type_refs(t, out);
            }
        }
        MItem::Use { pkg, .. } => out.push(pkg.clone()),
        MItem::Seq(_, body)
        | MItem::Comb(body)
        | MItem::Always(body)
        | MItem::Latch(body)
        | MItem::Initial(body)
        | MItem::Final(body) => collect_stmt_type_refs(body, out),
        MItem::GenFor { body, .. } => {
            for it in body {
                collect_mitem_refs(it, out);
            }
        }
        MItem::GenIf { then, els, .. } => {
            for it in then.iter().chain(els.iter()) {
                collect_mitem_refs(it, out);
            }
        }
        MItem::Func(f) => {
            for (_, ty, _, _) in &f.args {
                collect_type_refs(ty, out);
            }
            if let Some(t) = &f.ret {
                collect_type_refs(t, out);
            }
        }
        MItem::Task(t) => {
            for (_, ty, _, _) in &t.args {
                collect_type_refs(ty, out);
            }
        }
        MItem::Inst { .. }
        | MItem::Assign { .. }
        | MItem::AssertProperty(_)
        | MItem::AssumeProperty(_)
        | MItem::CoverProperty(_) => {}
    }
}

fn collect_typedef_refs(td: &crate::ast::Typedef, out: &mut Vec<String>) {
    use crate::ast::Typedef;
    match td {
        Typedef::Alias { ty, .. } => collect_type_refs(ty, out),
        Typedef::Struct { fields, .. } | Typedef::Union { fields, .. } => {
            for f in fields {
                collect_type_refs(&f.ty, out);
            }
        }
        Typedef::Enum { .. } => {}
    }
}

fn collect_type_refs(ty: &crate::ast::MvType, out: &mut Vec<String>) {
    use crate::ast::MvType;
    match ty {
        MvType::Named(n, ..) => {
            out.push(n.clone());
        }
        MvType::Signed(inner) => collect_type_refs(inner, out),
        MvType::Array(inner, _) => collect_type_refs(inner, out),
        MvType::Queue(inner) => collect_type_refs(inner, out),
        _ => {}
    }
}

fn collect_stmt_type_refs(s: &crate::ast::Stmt, out: &mut Vec<String>) {
    use crate::ast::Stmt;
    match s {
        Stmt::Block(stmts) | Stmt::Fork { branches: stmts, .. } => {
            for x in stmts {
                collect_stmt_type_refs(x, out);
            }
        }
        Stmt::NamedBlock { stmts, .. } => {
            for x in stmts {
                collect_stmt_type_refs(x, out);
            }
        }
        Stmt::VarDecl { ty, .. } => collect_type_refs(ty, out),
        Stmt::If { then, els, .. } => {
            collect_stmt_type_refs(then, out);
            if let Some(e) = els {
                collect_stmt_type_refs(e, out);
            }
        }
        Stmt::Case { items, default, .. } => {
            for (_, b) in items {
                collect_stmt_type_refs(b, out);
            }
            if let Some(d) = default {
                collect_stmt_type_refs(d, out);
            }
        }
        Stmt::CaseInside { items, default, .. } => {
            for (_, b) in items {
                collect_stmt_type_refs(b, out);
            }
            if let Some(d) = default {
                collect_stmt_type_refs(d, out);
            }
        }
        Stmt::For { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::Forever(body)
        | Stmt::Wait { body, .. }
        | Stmt::Event { body: Some(body), .. }
        | Stmt::Delay { body, .. }
        | Stmt::Foreach { body, .. } => collect_stmt_type_refs(body, out),
        _ => {}
    }
}

fn generate_from(
    file: &MvFile,
    base: &str,
    iface_names: &[&str],
    src_ext: &str,
    package: Option<&str>,
    includes: &[&str],
) -> Result<TranspileResult, MvError> {
    let opts = codegen::GenOpts {
        package,
        includes: includes.to_vec(),
    };
    let out = codegen::generate_src_ext_opts(file, base, iface_names, src_ext, &opts);
    Ok(TranspileResult {
        sv: out.sv,
        svh: out.svh,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transpile_counter_roundtrip() {
        let src = r#"
module counter #(WIDTH = 8) {
    in  clk, rst_n : bit
    in  enable     : bit
    out count      : logic[WIDTH-1:0]

    seq(clk, rst_n) {
        if (!rst_n) {
            count <= '0
        } else if (enable) {
            count <= count + 1
        }
    }
}
"#;
        let r = transpile(src, "counter").expect("transpile");
        assert!(r.sv.contains("module counter"));
        // Tanpa package/typedef → `.svh` kosong & `.sv` berdiri sendiri
        // (tidak ada baris `include) — fix: svh hanya digenerate jika perlu.
        assert!(r.svh.is_empty(), "svh harus kosong: {}", r.svh);
        assert!(!r.sv.contains("`include"), "sv tidak boleh include svh");
    }

    #[test]
    fn transpile_error_position() {

        let err = transpile("module {", "m").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.msg.contains("identifier"));
    }

    #[test]
    fn f11_format_error_shows_snippet_and_caret() {
        // F11: error type-check kini berposisi; format_error menampilkan
        // baris sumber + caret menunjuk kolom persis (gaya rustc).
        let src = "module m {\n    in clk : bit\n    out y : bit\n    comb { y = foo }\n}\n";
        let err = transpile(src, "m").unwrap_err();
        assert_eq!(err.line, 4);
        let rendered = format_error("counter.mv", src, &err);
        assert!(
            rendered.contains("counter.mv: 4:16: [E2001]"),
            "got: {rendered}"
        );
        assert!(rendered.contains("comb { y = foo }"), "got: {rendered}");
        assert!(rendered.contains("^"), "caret harus ada: {rendered}");
        // Caret menunjuk kolom `foo` (16): baris caret = `     | ` (7 char)
        // + (col-1)=15 spasi + '^' → index 7+15=22.
        let caret_line = rendered.lines().find(|l| l.contains('^')).unwrap();
        assert_eq!(caret_line.find('^').unwrap(), 22, "caret col: {rendered}");
    }

    #[test]
    fn transpile_error_unclosed() {
        let err = transpile("module m {\n    in clk : bit\n", "m").unwrap_err();
        assert!(err.msg.contains("tidak ditutup"));
    }

    #[test]
    fn atsv_escape_hatch_verbatim() {
        // `@sv { ... }` — teks SV mentah di-emit verbatim; body isolasi dari
        // lexer .mv (karakter `|->`, `"`, `$`, `;` bebas DI DALAM body).
        // CATATAN: statement .mv di luar `@sv` TIDAK memakai `;` (mirip Python);
        // hanya isi `@sv` yang bebas menulis `;` SV.
        let src = r#"
module m {
    in clk : bit
    out y  : logic[3:0]
    initial {
        @sv {
            // SV mentah — bisa apa pun
            $monitor("t=%0t y=%0d", $time, y);
            assert property (@(posedge clk) y == $past(y) + 1);
        }
        y = 4'h0
    }
}
"#;
        let r = transpile(src, "m").expect("transpile @sv");
        assert!(
            r.sv.contains("$monitor(\"t=%0t y=%0d\", $time, y);"),
            "SV mentah harus di-emit verbatim: {}",
            r.sv
        );
        assert!(
            r.sv.contains("assert property (@(posedge clk) y == $past(y) + 1);"),
            "operator SVA dalam @sv harus lolos: {}",
            r.sv
        );
        // statement setelah @sv tetap di-emit
        assert!(r.sv.contains("y = 4'h0;"), "stmts setelah @sv: {}", r.sv);
    }

    #[test]
    fn atsv_brace_balance_and_string() {
        // Brace di dalam string `@sv` tidak menghitung kedalaman; brace
        // bersarang di luar string dihitung.
        let src = r#"
module m {
    out y : logic[7:0]
    comb {
        @sv {
            // string berisi { tak memengaruhi
            `SV_MACRO("{a}") 
            y = 8'hFF;
        }
    }
}
"#;
        let r = transpile(src, "m").expect("transpile @sv brace balance");
        assert!(
            r.sv.contains("`SV_MACRO(\"{a}\")"),
            "string berisi brace harus lolos verbatim: {}",
            r.sv
        );
        assert!(r.sv.contains("y = 8'hFF;"), "body @sv: {}", r.sv);
    }

    #[test]
    fn array_lit_unpacked_init() {
        // F42: `'{e0, e1, ...}` — array literal (assignment pattern unpacked)
        // di-emit `'{...}` (bukan concat `{...}`), untuk deklarasi ROM/LUT.
        let src = r#"
module m {
    in clk : bit
    out rom0 : logic[7:0]
    out rom3 : logic[7:0]
    sig rom : logic[8][4] = '{1, 2, 3, 4}
    comb {
        rom0 = rom[0]
        rom3 = rom[3]
    }
}
"#;
        let r = transpile(src, "m").expect("transpile array literal");
        assert!(
            r.sv.contains("[0:3] = '{1, 2, 3, 4};"),
            "emisi array init: {}",
            r.sv
        );
        // concat biasa tetap `{...}` tanpa quote
        let r2 = transpile(
            "module m2 {\n out y : logic[15:0]\n comb { y = {8'h01, 8'h02} } }",
            "m2",
        )
        .expect("concat tetap");
        assert!(r2.sv.contains("y = {8'h01, 8'h02};"), "concat: {}", r2.sv);
    }

    #[test]
    fn for_step_generate_and_behavioral() {
        // `for i in 0..N step 2` — increment `i = i + 2` di generate &
        // behavioral; tanpa step tetap `i = i + 1`.
        let src = r#"
module m {
    in clk : bit
    out acc : logic[7:0]
    sig a : logic[7:0]
    // generate dengan step
    for i in 0..8 step 2 {
        sig a_int : logic[7:0]
        comb { a = 1 }
    }
    // behavioral dengan step (hanya genap)
    comb {
        for j in 0..8 step 2 {
            a[j] = 1
        }
    }
    // behavioral tanpa step
    comb {
        for k in 0..4 {
            a[k] = 0
        }
    }
}
"#;
        let r = transpile(src, "m").expect("transpile for step");
        assert!(
            r.sv.contains("for (genvar i = 0; i < 8; i = i + 2) begin : gen_i"),
            "generate step: {}",
            r.sv
        );
        assert!(
            r.sv.contains("for (int j = 0; j < 8; j = j + 2) begin"),
            "behavioral step: {}",
            r.sv
        );
        assert!(
            r.sv.contains("for (int k = 0; k < 4; k = k + 1) begin"),
            "tanpa step default +1: {}",
            r.sv
        );
    }

    #[test]
    fn transpile_many_cross_file() {
        // F9: `types.mv` + `counter.mv` di-transpile bersama — package dari
        // file pertama terlihat oleh file kedua (konteks gabungan).
        let items = vec![
            (
                "package types_pkg {\n type Addr = logic[15:0]\n enum State { IDLE, RUN }\n}\nmodule types_dummy {\n in clk : bit\n}\n".to_string(),
                "types".to_string(),
            ),
            (
                "module counter {\n use types_pkg::*\n in clk, rst_n : bit\n out addr : Addr\n out st : State\n seq(clk, rst_n) {\n if (!rst_n) {\n addr <= '0\n st <= IDLE\n } else {\n addr <= addr + 1\n st <= RUN\n }\n }\n}\n".to_string(),
                "counter".to_string(),
            ),
        ];
        let results = transpile_many(&items).expect("transpile batch lintas-file");
        // counter.sv memakai tipe dari package (bukan typedef lokal)
        assert!(results[1].sv.contains("module counter"));
        assert!(results[1].sv.contains("import types_pkg::*;"));
        // types.svh berisi package types_pkg
        assert!(results[0].svh.contains("package types_pkg;"));
        assert!(results[0].svh.contains("typedef logic [15:0] Addr;"));
    }

    #[test]
    fn transpile_many_solo_still_errors() {
        // Satu file yang memakai tipe dari file lain (tanpa file definisinya)
        // tetap error E2005 — konsisten dengan perilaku per-file.
        let items = vec![(
            "module counter {\n use types_pkg::*\n in clk : bit\n out addr : Addr\n comb { addr = 1 }\n}\n".to_string(),
            "counter".to_string(),
        )];
        let (idx, e) = transpile_many(&items).unwrap_err();
        assert_eq!(idx, 0);
        assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    }

    #[test]
    fn transpile_package_svh() {
        let src = r#"
package pkt {
    type Addr = logic[15:0]
    enum State { IDLE, RUN }
}
module top {
    use pkt::*
    in clk : bit
    out a  : Addr
    out s  : State
}
"#;
        let r = transpile(src, "top").unwrap();
        assert!(r.svh.contains("package pkt;"));
        assert!(r.svh.contains("typedef logic [15:0] Addr;"));
        assert!(r.sv.contains("`include \"top.svh\""));
        assert!(r.sv.contains("import pkt::*;"));
    }

    // ── F43: `.mvh` (header Mivon HDL → `.svh` saja) ──

    #[test]
    fn mvh_header_transpiles_to_svh_only() {
        // `.mvh` valid (typedef + package + interface) → `.svh` lengkap
        // dgn include guard, `.sv` kosong, header komentar menyebut `.mvh`.
        let src = r#"
type Addr = logic[15:0]
package hdr_pkg {
    enum State { IDLE, RUN }
}
interface bus_if {
    in clk : bit
}
"#;
        let r = transpile_header(src, "defs").expect("transpile .mvh");
        assert!(r.svh.contains("`ifndef DEFS_SVH"), "guard: {}", r.svh);
        assert!(r.svh.contains("typedef logic [15:0] Addr;"));
        assert!(r.svh.contains("package hdr_pkg;"));
        assert!(r.svh.contains("interface bus_if;"));
        assert!(r.svh.contains("Sumber    : defs.mvh"), "header: {}", r.svh);
        assert!(r.svh.contains("mivon mgen defs.mvh"));
        assert!(r.sv.is_empty(), ".sv harus kosong: {}", r.sv);
    }

    #[test]
    fn mvh_rejects_module_with_position() {
        // E2008: konten fungsional di `.mvh` ditolak, berposisi line:col.
        let src = "type Addr = logic[7:0]\nmodule bad {\n in clk : bit\n}\n";
        let e = transpile_header(src, "defs").unwrap_err();
        assert!(e.msg.contains("E2008"), "msg: {}", e.msg);
        assert!(e.msg.contains("module 'bad'"), "msg: {}", e.msg);
        assert_eq!(e.line, 2, "posisi line nama module");
        assert!(e.col > 0);
    }

    #[test]
    fn mvh_rejects_func_class_task() {
        for (src, want) in [
            ("func f() -> int {\n    return 1\n}\n", "function 'f'"),
            ("task t() {\n    #1\n}\n", "task 't'"),
            ("class c {\n    field x : int\n}\n", "class 'c'"),
            ("program p {\n    in clk : bit\n}\n", "program 'p'"),
        ] {
            let e = transpile_header(src, "h").unwrap_err();
            assert!(e.msg.contains("E2008"), "src {src:?}: {}", e.msg);
            assert!(e.msg.contains(want), "src {src:?}: {}", e.msg);
        }
    }

    #[test]
    fn mvh_no_check_still_validates_header() {
        // `--no-check` melewatkan type-check TAPI kontrak header tetap (E2008).
        let src = "module m {\n in clk : bit\n}\n";
        let e = transpile_header_no_check(src, "h").unwrap_err();
        assert!(e.msg.contains("E2008"), "msg: {}", e.msg);
        // Type-check memang dilewati: sumber dgn tipe tak dikenal lolos.
        let r = transpile_header_no_check("type A = Nope[3]\n", "h");
        assert!(r.is_ok(), "no-check harus lewati E2005: {:?}", r.err());
    }

    #[test]
    fn mvh_batch_mixed_with_mv() {
        // Batch gabungan: `defs.mvh` (header) + `counter.mv` (desain) —
        // package dari file header terlihat oleh desain (konteks F9),
        // flag `header` menghasilkan output sejajar per item.
        let items = vec![
            MvItem::header(
                "package hdr_pkg {\n type Addr = logic[15:0]\n}\n",
                "defs",
            ),
            MvItem::new(
                "module counter {\n use hdr_pkg::*\n in clk : bit\n out a : Addr\n comb { a = 0 }\n}\n",
                "counter",
            ),
        ];
        let results = transpile_many_items(&items).expect("batch .mvh+.mv");
        assert!(results[0].svh.contains("package hdr_pkg;"));
        assert!(results[0].sv.is_empty(), ".mvh tanpa .sv");
        assert!(results[0].svh.contains("defs.mvh"));
        assert!(results[1].sv.contains("module counter"));
        assert!(results[1].sv.contains("import hdr_pkg::*;"));
        assert!(results[1].sv.contains("Sumber    : counter.mv"));
    }

    #[test]
    fn mvh_batch_error_carries_index() {
        // E2008 di batch → indeks item asal + posisi.
        let items = vec![
            MvItem::new("module ok {\n in clk : bit\n}\n", "ok"),
            MvItem::header("module bad {\n in clk : bit\n}\n", "bad"),
        ];
        let (idx, e) = transpile_many_items(&items).unwrap_err();
        assert_eq!(idx, 1);
        assert!(e.msg.contains("E2008"), "msg: {}", e.msg);
    }

    #[test]
    fn mv_legacy_batch_api_still_works() {
        // API lama `transpile_many((src, base))` tetap berfungsi (semua .mv).
        let items = vec![(
            "module m {\n in clk : bit\n out y : bit\n comb { y = 1 }\n}\n".to_string(),
            "m".to_string(),
        )];
        let results = transpile_many(&items).expect("legacy batch");
        assert!(results[0].sv.contains("module m"));
        assert!(results[0].sv.contains("Sumber    : m.mv"));
    }

    #[test]
    fn f77_cross_file_include_in_sv() {
        // F77: file yang memakai package file LAIN mendapat
        // `` `include "<base>.svh" `` supaya output mandiri di tool EDA.
        let items = vec![
            MvItem::new(
                "package types_pkg {\n type Addr = logic[15:0]\n}\nmodule types_dummy {\n in clk : bit\n}\n",
                "types",
            ),
            MvItem::new(
                "module counter {\n use types_pkg::*\n in clk : bit\n out addr : Addr\n comb { addr = 0 }\n}\n",
                "counter",
            ),
        ];
        let results = transpile_many_items(&items).expect("batch F77");
        assert!(
            results[1].sv.contains("`include \"types.svh\""),
            "counter.sv harus include types.svh: {}",
            results[1].sv
        );
        // Pemilik definisi hanya include .svh-nya sendiri (perilaku lama),
        // bukan file lain.
        assert!(
            !results[0].sv.contains("`include \"counter.svh\""),
            "types.sv tak boleh include counter.svh: {}",
            results[0].sv
        );
    }

    #[test]
    fn f77_no_include_without_cross_ref() {
        // Tanpa referensi lintas-file → tidak ada include tambahan
        // (output lama tak berubah).
        let items = vec![
            MvItem::new("module a {\n in clk : bit\n out y : bit\n comb { y = 1 }\n}\n", "a"),
            MvItem::new("module b {\n in clk : bit\n out y : bit\n comb { y = 0 }\n}\n", "b"),
        ];
        let results = transpile_many_items(&items).expect("batch tanpa cross-ref");
        assert!(!results[0].sv.contains("`include"), "a.sv: {}", results[0].sv);
        assert!(!results[1].sv.contains("`include"), "b.sv: {}", results[1].sv);
    }

    #[test]
    fn f77_interface_typedef_cross_ref() {

        // Referensi via tipe port interface & typedef scoped `pkg::T`
        // juga memicu include.
        let items = vec![
            MvItem::new(
                "package p {\n type W = logic[7:0]\n}\ninterface bus_if {\n in clk : bit\n}\nmodule d1 {\n in clk : bit\n}\n",
                "defs",
            ),
            MvItem::new(
                "module dut {\n use p::*\n in b : bus_if\n out y : p::W\n comb { y = 0 }\n}\n",
                "dut",
            ),
        ];
        let results = transpile_many_items(&items).expect("batch iface/typedef");
        assert!(
            results[1].sv.contains("`include \"defs.svh\""),
            "dut.sv: {}",
            results[1].sv
        );
    }
}
