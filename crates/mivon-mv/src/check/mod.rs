//! Mivon HDL (.mv) — Type-check & semantic analysis (Fase 4).
//! Validasi SEBELUM emisi (MIVON-HDL.md §9, prinsip desain #4).
//! 1 file = 1 tanggung jawab — struktur (berkembang):
//! - `mod.rs`   — entry `check`/`check_many`, konteks (Ctx/Scope/Env/BlockKind),
//!   indeks global (collect_ctx), error helper
//! - `defs.rs`  — typedef & interface check
//! - `module.rs`— module/param/port/item/generate/inst check
//! - `class.rs` — function/task/class/constraint check
//! - `stmt.rs`  — statement check (E2002/E2003/E2004 dll)
//! - `expr.rs`  — expression check + lebar bit/const-fold
//! - `tests.rs` — unit test

use crate::ast::*;
use crate::MvError;
use std::collections::{HashMap, HashSet};

pub mod class;
pub mod defs;
pub mod expr;
pub mod module;
pub mod stmt;

use class::{check_class, check_func, check_task};
use defs::{check_interface, check_typedef};
use module::check_module;

#[cfg(test)]
mod tests;

/// Indeks global satu file `.mv`.
pub(crate) struct Ctx<'a> {
    /// nama tipe (file level + semua package) → Typedef
    pub(crate) types: HashMap<&'a str, &'a Typedef>,
    /// nama package → Package
    pub(crate) packages: HashMap<&'a str, &'a Package>,
    /// nama member enum → lebar enum asal (bit)
    pub(crate) enum_members: HashMap<&'a str, i64>,
    /// nama konstanta package (terlihat di lebar/nilai enum & ekspresi)
    pub(crate) consts: HashSet<&'a str>,
    /// nama function/task level file
    pub(crate) funcs: HashSet<&'a str>,
    pub(crate) tasks: HashSet<&'a str>,
    /// nama function/task → (jumlah argumen WAJIB, jumlah argumen total).
    /// Dipakai E2011: jumlah argumen saat pemanggilan harus antara keduanya
    /// (argumen ber-default boleh dilewati dari belakang). Tanpa ini,
    /// `f()` untuk `func f(a, b)` lolos type-check lalu gagal di simulator.
    ///
    /// Entri dengan value `None` = nama AMBIGU (dideklarasikan di lebih dari
    /// satu file konteks gabungan) → E2011 dilewati agar tidak jadi false
    /// positive. Duplikat lintas file sendiri sudah ditolak E2007.
    pub(crate) func_arity: HashMap<&'a str, Option<(usize, usize)>>,
    /// nama method class → (jumlah argumen WAJIB, jumlah argumen total).
    pub(crate) method_arity: HashMap<&'a str, Option<(usize, usize)>>,
    /// nama class — sah sebagai tipe user-defined (lebar tidak diketahui,
    /// lihat type_width): `var it : item = item::new()` (F12).
    pub(crate) classes: HashSet<&'a str>,
    /// nama interface (F26) — sah sebagai tipe port module (`in if : axi_lite`).
    pub(crate) interfaces: HashSet<&'a str>,
    /// nama module + program di file ini/gabungan (F29) — target instansiasi
    /// yang dikenal; nama lain dianggap eksternal (file .mv lain) → dilewati
    /// (pola konservatif E2001/E2005).
    pub(crate) modules: HashSet<&'a str>,
    /// nama target (module/interface/program) → daftar port (utk validasi
    /// koneksi `.port(expr)` dan jumlah positional).
    pub(crate) module_ports: HashMap<&'a str, Vec<String>>,
    /// nama target module/program → daftar parameter (F31 — validasi
    /// override `inst foo u #(.W(4))`).
    pub(crate) module_params: HashMap<&'a str, Vec<String>>,
    /// F32: nama target module/program → daftar nama TYPE parameter.
    pub(crate) module_type_params: HashMap<&'a str, Vec<String>>,
}

/// Nilai parameter yang berhasil di-fold ke konstanta integer.
pub(crate) type Params<'a> = HashMap<&'a str, i64>;

/// Lingkungan module untuk pesan error + aturan E2003.
#[derive(Clone)]
pub(crate) struct Env<'a> {
    pub(crate) mname: &'a str,
    /// arah port (nama port → Dir)
    pub(crate) ports: HashMap<&'a str, Dir>,
}

/// Scope deklarasi yang terlihat pada titik check.
#[derive(Clone)]
pub(crate) struct Scope<'a> {
    /// nama sinyal/port/reg/const/genvar/loop-var/local/instance
    pub(crate) sigs: HashSet<&'a str>,
    /// nama function/task (module + file level)
    pub(crate) funcs: HashSet<&'a str>,
    /// arity function/task yang terlihat di scope (lihat `Ctx::func_arity`)
    pub(crate) func_arity: HashMap<&'a str, Option<(usize, usize)>>,
    /// parameter module + nilai konstannya (untuk const-fold)
    pub(crate) params: Params<'a>,
    /// F32: type parameter module (`T : type = logic[7:0]`) — nama → default
    /// tipe (None bila tanpa default).
    pub(crate) type_params: HashMap<&'a str, Option<&'a MvType>>,
    /// tipe deklarasi sinyal/port (untuk lebar bit)
    pub(crate) types: HashMap<&'a str, &'a MvType>,
    /// typedef LOKAL module (`type X = ...` di badan module) — nama tipe →
    /// definisi; sah sbg tipe di dalam module (bukan ctx global).
    pub(crate) local_types: HashMap<&'a str, &'a Typedef>,
    pub(crate) enum_members: &'a HashMap<&'a str, i64>,
    /// konstanta package (terlihat sebagai ident di ekspresi)
    pub(crate) consts: &'a HashSet<&'a str>,
    /// konstanta LOCAL module (`const C = 4` di badan module) — dipakai
    /// aturan lvalue (E2010): `const`/`localparam` tak boleh di-assign.
    pub(crate) local_consts: HashSet<&'a str>,
    pub(crate) env: Env<'a>,
    /// kedalaman loop (untuk validasi break/continue)
    pub(crate) loop_depth: usize,
    /// apakah sedang di dalam task (return expr di task = error)
    pub(crate) in_task: bool,
}

impl Scope<'_> {
    pub(crate) fn known(&self, name: &str) -> bool {
        self.sigs.contains(name)
            || self.funcs.contains(name)
            || self.params.contains_key(name)
            || self.enum_members.contains_key(name)
            || self.consts.contains(name)
    }
}

/// Konteks blok statement untuk aturan assignment (E2004).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum BlockKind {
    /// `seq` — hanya non-blocking `<=`
    Seq,
    /// comb/always/latch/func/task — hanya blocking `=`; E2003 aktif
    Always,
    /// `initial`/`final` (testbench) — blocking `=`; boleh drive port input
    Tb,
}

/// Error type-check BERPOSISI (F11): `err_at(line, col, code, msg)`.
pub(crate) fn err_at(line: usize, col: usize, code: &str, msg: impl Into<String>) -> MvError {
    MvError::new(line, col, format!("[{code}] {}", msg.into()))
}

/// Posisi (line, col) nama typedef — dipakai error E2007/E2005 (F11).
pub(crate) fn td_pos(td: &Typedef) -> (usize, usize) {
    match td {
        Typedef::Alias { line, col, .. }
        | Typedef::Struct { line, col, .. }
        | Typedef::Union { line, col, .. }
        | Typedef::Enum { line, col, .. } => (*line, *col),
    }
}

// ── Entrypoint ──

/// Type-check satu file `.mv` (konteks hanya dari file ini). Error pertama
/// di-return (fail-fast). Untuk batch lintas-file pakai `check_many`.
pub fn check(file: &MvFile) -> Result<(), MvError> {
    let ctx = build_ctx(file);
    check_with_ctx(file, &ctx)
}

/// Type-check beberapa file `.mv` dengan KONTEKS GABUNGAN (F9 — multi-file).
/// Error pertama di-return bersama indeks file asalnya.
pub fn check_many(files: &[&MvFile]) -> Result<(), (usize, MvError)> {
    let ctx = build_ctx_many(files);

    // ── duplikat LINTAS-FILE (E2007) ──
    let mut pkg_owner: HashMap<&str, usize> = HashMap::new();
    let mut type_owner: HashMap<&str, usize> = HashMap::new();
    let mut cls_owner: HashMap<&str, usize> = HashMap::new();
    let mut mod_owner: HashMap<&str, usize> = HashMap::new();
    // F26: interface berbagi namespace dengan package (definisi bersama di
    // .svh) — duplikat lintas-file juga error (E2007).
    let mut ifc_owner: HashMap<&str, usize> = HashMap::new();
    // Function/task level file juga bentrok lintas-file: keduanya di-emit
    // ke scope FILE SV yang sama, jadi verilator/iverilog menolak
    // "Duplicate declaration of function".
    let mut func_owner: HashMap<&str, usize> = HashMap::new();
    let mut task_owner: HashMap<&str, usize> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        for f in &f.funcs {
            if let Some(prev) = func_owner.insert(f.name.as_str(), i) {
                if prev != i {
                    return Err((
                        i,
                        err_at(
                            f.line,
                            f.col,
                            "E2007",
                            format!(
                                "function '{}' dideklarasikan di file #{} dan #{} — \
                                 keduanya jadi scope file SV yang sama",
                                f.name,
                                prev + 1,
                                i + 1
                            ),
                        ),
                    ));
                }
            }
        }
        for t in &f.tasks {
            if let Some(prev) = task_owner.insert(t.name.as_str(), i) {
                if prev != i {
                    return Err((
                        i,
                        err_at(
                            t.line,
                            t.col,
                            "E2007",
                            format!(
                                "task '{}' dideklarasikan di file #{} dan #{} — \
                                 keduanya jadi scope file SV yang sama",
                                t.name,
                                prev + 1,
                                i + 1
                            ),
                        ),
                    ));
                }
            }
        }
    }
    for (i, f) in files.iter().enumerate() {
        for p in &f.packages {
            if let Some(prev) = pkg_owner.insert(p.name.as_str(), i) {
                if prev != i {
                    return Err((
                        i,
                        err_at(
                            p.line,
                            p.col,
                            "E2007",
                            format!(
                                "package '{}' dideklarasikan di file #{} dan #{}",
                                p.name,
                                prev + 1,
                                i + 1
                            ),
                        ),
                    ));
                }
            }
        }
        for ifc in &f.interfaces {
            if let Some(prev) = ifc_owner.insert(ifc.name.as_str(), i) {
                if prev != i {
                    return Err((
                        i,
                        err_at(
                            ifc.line,
                            ifc.col,
                            "E2007",
                            format!(
                                "interface '{}' dideklarasikan di file #{} dan #{}",
                                ifc.name,
                                prev + 1,
                                i + 1
                            ),
                        ),
                    ));
                }
            }
        }
        for td in &f.typedefs {
            let n = td_name(td);
            if let Some(prev) = type_owner.insert(n, i) {
                if prev != i {
                    let (l, c) = td_pos(td);
                    return Err((
                        i,
                        err_at(
                            l,
                            c,
                            "E2007",
                            format!(
                                "tipe '{n}' dideklarasikan di file #{} dan #{}",
                                prev + 1,
                                i + 1
                            ),
                        ),
                    ));
                }
            }
        }
        for c in &f.classes {
            if let Some(prev) = cls_owner.insert(c.name.as_str(), i) {
                if prev != i {
                    return Err((
                        i,
                        err_at(
                            c.line,
                            c.col,
                            "E2007",
                            format!(
                                "class '{}' dideklarasikan di file #{} dan #{}",
                                c.name,
                                prev + 1,
                                i + 1
                            ),
                        ),
                    ));
                }
            }
        }
        // Module & program berbagi namespace SV — duplikat lintas-file juga
        // error (E2007).
        for m in f.modules.iter().chain(f.programs.iter()) {
            if let Some(prev) = mod_owner.insert(m.name.as_str(), i) {
                if prev != i {
                    return Err((
                        i,
                        err_at(
                            m.line,
                            m.col,
                            "E2007",
                            format!(
                                "module '{}' dideklarasikan di file #{} dan #{}",
                                m.name,
                                prev + 1,
                                i + 1
                            ),
                        ),
                    ));
                }
            }
        }
        // F29 fix review: module/interface berbagi namespace tipe di SV.
        for ifc in &f.interfaces {
            if let Some(prev) = mod_owner.get(ifc.name.as_str()) {
                if *prev != i {
                    return Err((
                        i,
                        err_at(
                            ifc.line,
                            ifc.col,
                            "E2007",
                            format!(
                                "interface '{}' bentrok dengan module di file #{}",
                                ifc.name,
                                prev + 1
                            ),
                        ),
                    ));
                }
            }
        }
    }

    for (i, f) in files.iter().enumerate() {
        if let Err(e) = check_with_ctx(f, &ctx) {
            return Err((i, e));
        }
    }
    Ok(())
}

/// Validasi satu file terhadap konteks yang diberikan (bisa gabungan).
pub(crate) fn check_with_ctx<'a>(file: &'a MvFile, ctx: &'a Ctx<'a>) -> Result<(), MvError> {
    // ── level file: duplikasi tipe + validasi isi ──
    let mut seen = HashSet::new();
    for td in &file.typedefs {
        let n = td_name(td);
        if !seen.insert(n) {
            let (l, c) = td_pos(td);
            return Err(err_at(
                l,
                c,
                "E2007",
                format!("tipe '{n}' dideklarasikan dua kali di level file"),
            ));
        }
        check_typedef(td, ctx)?;
    }
    // duplikat nama function/task level file (E2007)
    let mut fnames = HashSet::new();
    for f in &file.funcs {
        if !fnames.insert(f.name.as_str()) {
            return Err(err_at(
                f.line,
                f.col,
                "E2007",
                format!(
                    "function '{}' dideklarasikan dua kali di level file",
                    f.name
                ),
            ));
        }
    }
    let mut tnames = HashSet::new();
    for t in &file.tasks {
        if !tnames.insert(t.name.as_str()) {
            return Err(err_at(
                t.line,
                t.col,
                "E2007",
                format!("task '{}' dideklarasikan dua kali di level file", t.name),
            ));
        }
    }

    // ── interface (F26) — namespace sama dgn module (F29) ──
    let mut mod_names: HashSet<&str> = HashSet::new();
    for m in &file.modules {
        mod_names.insert(m.name.as_str());
    }
    for p in &file.programs {
        mod_names.insert(p.name.as_str());
    }
    let mut ifc_seen = HashSet::new();
    for i in &file.interfaces {
        if !ifc_seen.insert(i.name.as_str()) {
            return Err(err_at(
                i.line,
                i.col,
                "E2007",
                format!("interface '{}' dideklarasikan dua kali", i.name),
            ));
        }
        if mod_names.contains(i.name.as_str()) {
            return Err(err_at(
                i.line,
                i.col,
                "E2007",
                format!(
                    "interface '{}' bentrok dengan module/program bernama sama",
                    i.name
                ),
            ));
        }
        check_interface(i, ctx)?;
    }

    let mut pkg_seen = HashSet::new();
    for p in &file.packages {
        if !pkg_seen.insert(p.name.as_str()) {
            return Err(err_at(
                p.line,
                p.col,
                "E2007",
                format!("package '{}' dideklarasikan dua kali", p.name),
            ));
        }
        let mut td_seen = HashSet::new();
        let mut c_seen = HashSet::new();
        for td in &p.typedefs {
            let n = td_name(td);
            if !td_seen.insert(n) {
                let (l, c) = td_pos(td);
                return Err(err_at(
                    l,
                    c,
                    "E2007",
                    format!("tipe '{n}' dideklarasikan dua kali di package '{}'", p.name),
                ));
            }
            check_typedef(td, ctx)?;
        }
        for (cn, _, _) in &p.consts {
            if !c_seen.insert(cn.as_str()) {
                return Err(err_at(
                    p.line,
                    p.col,
                    "E2007",
                    format!(
                        "konstanta '{cn}' dideklarasikan dua kali di package '{}'",
                        p.name
                    ),
                ));
            }
        }
    }

    // ── module ──
    let mut mod_seen = HashSet::new();
    for m in &file.modules {
        if !mod_seen.insert(m.name.as_str()) {
            return Err(err_at(
                m.line,
                m.col,
                "E2007",
                format!("module '{}' dideklarasikan dua kali", m.name),
            ));
        }
        check_module(m, ctx)?;
    }
    // `program` (MIVON-HDL.md §7.3) — body testbench tetap di-type-check.
    for p in &file.programs {
        if !mod_seen.insert(p.name.as_str()) {
            return Err(err_at(
                p.line,
                p.col,
                "E2007",
                format!("module/program '{}' dideklarasikan dua kali", p.name),
            ));
        }
        check_module(p, ctx)?;
    }

    // ── class (MIVON-HDL.md §8) ──
    let mut cls_seen = HashSet::new();
    for c in &file.classes {
        if !cls_seen.insert(c.name.as_str()) {
            return Err(err_at(
                c.line,
                c.col,
                "E2007",
                format!("class '{}' dideklarasikan dua kali", c.name),
            ));
        }
        check_class(c, ctx)?;
    }

    // ── function/task level file ──
    for f in &file.funcs {
        let mut scope = new_scope(ctx, &f.name);
        check_func(f, ctx, &mut scope)?;
    }
    for t in &file.tasks {
        let mut scope = new_scope(ctx, &t.name);
        check_task(t, ctx, &mut scope)?;
    }
    Ok(())
}

// ── Indeks ──

/// Kumpulkan indeks global dari satu atau banyak file. `or_insert` → nama
/// yang duplikat antar-file memakai definisi pertama (urutan deterministik).
fn collect_ctx<'a>(files: impl IntoIterator<Item = &'a MvFile>) -> Ctx<'a> {
    let mut types = HashMap::new();
    let mut packages = HashMap::new();
    let mut enum_members = HashMap::new();
    let mut funcs = HashSet::new();
    let mut func_arity: HashMap<&str, Option<(usize, usize)>> = HashMap::new();
    let mut method_arity: HashMap<&str, Option<(usize, usize)>> = HashMap::new();
    let mut tasks = HashSet::new();
    let mut consts = HashSet::new();
    let mut classes = HashSet::new();
    let mut interfaces = HashSet::new();
    let mut modules = HashSet::new();
    let mut module_ports = HashMap::new();
    let mut module_params = HashMap::new();
    let mut module_type_params = HashMap::new();
    for file in files {
        for td in &file.typedefs {
            types.entry(td_name(td)).or_insert(td);
            collect_enum_members(td, &mut enum_members);
        }
        for i in &file.interfaces {
            interfaces.insert(i.name.as_str());
            module_ports.insert(
                i.name.as_str(),
                i.ports.iter().flat_map(|p| p.names.clone()).collect(),
            );
        }
        // module & program berbagi namespace SV — indeks ports/params/type-params
        // identik (reuse helper; program tak punya param, tapi konsisten).
        for m in file.modules.iter().chain(file.programs.iter()) {
            modules.insert(m.name.as_str());
            module_ports.insert(
                m.name.as_str(),
                m.items
                    .iter()
                    .filter_map(|it| match it {
                        MItem::Port(p) => Some(p.names.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect(),
            );
            module_params.insert(
                m.name.as_str(),
                m.params.iter().map(|p| p.name.clone()).collect(),
            );
            module_type_params.insert(
                m.name.as_str(),
                m.params
                    .iter()
                    .filter(|p| {
                        p.type_default.is_some()
                            || matches!(&p.ty, Some(MvType::Named(s, ..)) if s == "type")
                    })
                    .map(|p| p.name.clone())
                    .collect(),
            );
        }
        for p in &file.packages {
            packages.entry(p.name.as_str()).or_insert(p);
            for td in &p.typedefs {
                types.entry(td_name(td)).or_insert(td);
                collect_enum_members(td, &mut enum_members);
            }
        }
        for p in &file.packages {
            for (cn, _, _) in &p.consts {
                consts.insert(cn.as_str());
            }
        }
        for f in &file.funcs {
            funcs.insert(f.name.as_str());
            insert_arity(&mut func_arity, f.name.as_str(), arity_of(&f.args));
        }
        for t in &file.tasks {
            tasks.insert(t.name.as_str());
            insert_arity(&mut func_arity, t.name.as_str(), arity_of(&t.args));
        }
        for c in &file.classes {
            classes.insert(c.name.as_str());
            for f in &c.funcs {
                insert_arity(
                    &mut method_arity,
                    f.name.as_str(),
                    arity_of(&f.args),
                );
            }
            for t in &c.tasks {
                insert_arity(
                    &mut method_arity,
                    t.name.as_str(),
                    arity_of(&t.args),
                );
            }
        }
    }
    Ctx {
        types,
        packages,
        enum_members,
        consts,
        funcs,
        tasks,
        func_arity,
        method_arity,
        classes,
        interfaces,
        modules,
        module_ports,
        module_params,
        module_type_params,
    }
}

pub(crate) fn build_ctx<'a>(file: &'a MvFile) -> Ctx<'a> {
    collect_ctx(std::iter::once(file))
}

/// Sisipkan arity function/method. Nama yang muncul lebih dari sekali ditandai
/// ambigu (`None`) supaya E2011 tidak salah paham tandatangannya.
fn insert_arity<'a>(
    map: &mut HashMap<&'a str, Option<(usize, usize)>>,
    name: &'a str,
    arity: (usize, usize),
) {
    match map.get(name) {
        // Sudah ada dengan tanda tangan berbeda → ambigu.
        Some(Some(prev)) if *prev != arity => {
            map.insert(name, None);
        }
        Some(None) => {}
        _ => {
            map.insert(name, Some(arity));
        }
    }
}

/// `(jumlah argumen WAJIB, jumlah argumen total)` dari daftar argumen
/// func/task. Argumen ber-`default` boleh dilewati dari belakang, jadi hanya
/// argumen tanpa default yang wajib diberikan.
pub(crate) fn arity_of(args: &[(String, MvType, Option<Dir>, Option<Expr>)]) -> (usize, usize) {
    let total = args.len();
    let required = args.iter().filter(|(_, _, _, d)| d.is_none()).count();
    (required, total)
}

/// [`arity_of`] untuk sibling module crate.
pub(crate) fn arity_of_pub(args: &[(String, MvType, Option<Dir>, Option<Expr>)]) -> (usize, usize) {
    arity_of(args)
}

/// [`insert_arity`] untuk sibling module crate.
pub(crate) fn insert_arity_pub<'a>(
    map: &mut HashMap<&'a str, Option<(usize, usize)>>,
    name: &'a str,
    arity: (usize, usize),
) {
    insert_arity(map, name, arity)
}

pub(crate) fn build_ctx_many<'a>(files: &'a [&'a MvFile]) -> Ctx<'a> {
    collect_ctx(files.iter().copied())
}

pub(crate) fn td_name(td: &Typedef) -> &str {
    match td {
        Typedef::Alias { name, .. } => name,
        Typedef::Struct { name, .. } => name,
        Typedef::Union { name, .. } => name,
        Typedef::Enum { name, .. } => name,
    }
}

pub(crate) fn collect_enum_members<'a>(td: &'a Typedef, out: &mut HashMap<&'a str, i64>) {
    if let Typedef::Enum { width, members, .. } = td {
        // Lebar TOTAL bit — sinkron dengan `codegen/defs.rs`
        // (`logic [enum_bits:0]`) via `enum_width`. `enum_bits` = MSB saja.
        let w = match width {
            Some(Expr::Int(n)) => *n,
            _ => crate::enum_width(members.len()),
        };
        for m in members {
            out.entry(m.name.as_str()).or_insert(w);
        }
    }
}

pub(crate) fn resolve_typedef<'a>(name: &str, ctx: &'a Ctx<'a>) -> Option<&'a Typedef> {
    if let Some((pkg, item)) = name.split_once("::") {
        let p = ctx.packages.get(pkg)?;
        p.typedefs.iter().find(|td| td_name(td) == item)
    } else {
        ctx.types.get(name).copied()
    }
}

/// Scope module default baru: funcs global + env kosong.
pub(crate) fn new_scope<'a>(ctx: &'a Ctx<'a>, mname: &'a str) -> Scope<'a> {
    let mut funcs: HashSet<&'a str> = ctx.funcs.clone();
    funcs.extend(ctx.tasks.iter().copied());
    let func_arity = ctx.func_arity.clone();
    Scope {
        sigs: HashSet::new(),
        funcs,
        func_arity,
        params: HashMap::new(),
        type_params: HashMap::new(),
        types: HashMap::new(),
        local_types: HashMap::new(),
        enum_members: &ctx.enum_members,
        consts: &ctx.consts,
        local_consts: HashSet::new(),
        env: Env {
            mname,
            ports: HashMap::new(),
        },
        loop_depth: 0,
        in_task: false,
    }
}
