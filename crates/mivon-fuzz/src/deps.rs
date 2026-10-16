//! Dependency resolver untuk seed project real (area dv/UVM opentitan dll).
//!
//! Seed tunggal real RTL gagal compile standalone karena:
//! 1. `import pkg::*` — package tidak ter-definisi (file lain di project)
//! 2. `Module u_inst(...)` — submodule berada di file lain
//! 3. Tidak ada top-level module (file berisi package/interface saja) →
//!    "Unable to determine top-level design" (EL3001)
//!
//! Index nama (package/module/interface) dibangun SEKALI saat Corpus::load,
//! lalu `resolve` menyambung dependensi secara rekursif (depth-guard):
//! - import → prepend source package (recursive deps)
//! - instance → append source module (recursive deps)
//! - tanpa module sama sekali → append `module fz_top; endmodule`
//!
//! Ini menghapus mayoritas skip (census: 70% EL3001 no-top + 15% E3001
//! instance unresolved pada corpus penuh 4690 seeds).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Index nama deklarasi → file sumber (built sekali per Corpus).
#[derive(Debug, Default, Clone)]
pub struct DepsIndex {
    /// `package <name>;` → file.
    pub packages: HashMap<String, PathBuf>,
    /// `module <name>` → file (bisa lebih dari satu — ambil pertama).
    pub modules: HashMap<String, PathBuf>,
    /// `interface <name>` → file.
    pub interfaces: HashMap<String, PathBuf>,
}

/// Baca hanya awalan file untuk index (deklarasi top-level ada di kepala;
/// file project real besar tak perlu dibaca penuh → hemat I/O + memori).
const INDEX_HEAD_BYTES: usize = 64 * 1024;

fn read_head(path: &Path, max: usize) -> Option<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Ekstrak semua nama deklarasi top-level dari satu file (line-scan ringan:
/// `package X;` / `module X` / `interface X` pada awal baris). Bukan parser
/// penuh — cukup akurat utk index dependency.
pub fn index_file(path: &Path, idx: &mut DepsIndex) {
    let Some(content) = read_head(path, INDEX_HEAD_BYTES) else {
        return;
    };
    let mut in_comment = false;
    for line in content.lines() {
        let t = line.trim_start();
        // Skip komentar blok sederhana (/* */ single-line).
        if t.starts_with("/*") {
            in_comment = !t.contains("*/");
            continue;
        }
        if in_comment {
            if t.contains("*/") {
                in_comment = false;
            }
            continue;
        }
        if t.starts_with("//") {
            continue;
        }
        let rest = t;
        if let Some(r) = rest.strip_prefix("package ") {
            // `package X;` or `package X;` (tidak ada `package automatic`).
            let name = r
                .split(|c: char| c.is_whitespace() || c == ';' || c == '(')
                .next()
                .unwrap_or("");
            if !name.is_empty() && !name.starts_with("//") {
                idx.packages
                    .entry(name.to_string())
                    .or_insert_with(|| path.to_path_buf());
            }
            continue;
        }
        if let Some(r) = rest.strip_prefix("module ") {
            // `module X` / `module X #(...)` / `module X (`
            let name = r
                .split(|c: char| c.is_whitespace() || c == '(' || c == '#')
                .next()
                .unwrap_or("");
            if !name.is_empty() && !name.starts_with("//") {
                idx.modules
                    .entry(name.to_string())
                    .or_insert_with(|| path.to_path_buf());
            }
            continue;
        }
        if let Some(r) = rest.strip_prefix("interface ") {
            let name = r
                .split(|c: char| c.is_whitespace() || c == '(' || c == '#')
                .next()
                .unwrap_or("");
            if !name.is_empty() && !name.starts_with("//") {
                idx.interfaces
                    .entry(name.to_string())
                    .or_insert_with(|| path.to_path_buf());
            }
            continue;
        }
        // `export "DPI-C" ...` dan deklarasi lain tidak di-index.
    }
}

/// Bangun index dari daftar seed paths.
pub fn build_index(paths: &[PathBuf]) -> DepsIndex {
    let mut idx = DepsIndex::default();
    for p in paths {
        index_file(p, &mut idx);
    }
    idx
}

/// Kumpulkan import names: `import pkg::*` / `import pkg::item`.
fn collect_imports(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if let Some(r) = t.strip_prefix("import ") {
            if let Some(name) = r
                .split("::")
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "uvm_pkg")
            {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
    out
}

/// Kumpulkan instance module names: `Name u_inst(...)` / `Name #(...) u_inst(...)`.
/// False positive dimitigasi: hanya ambil nama yang ADA di module_index
/// (nama modul nyata di project) → bukan function call / tipe.
fn collect_instances(src: &str, idx: &DepsIndex) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        // Pola: `<Name> <u_inst>(` atau `<Name> #(...) <u_inst>(`
        // Cari byte-offset awal identifier pertama — char-boundary SAFE
        // (jangan slice per-byte di unicode `─` — panic ditemukan census).
        let start = t
            .char_indices()
            .find(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
            .map(|(i, _)| i)
            .unwrap_or(t.len());
        let first = t[start..]
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .next()
            .unwrap_or("");
        if first.is_empty() {
            continue;
        }
        if idx.modules.contains_key(first) && !out.iter().any(|s| s == first) {
            out.push(first.to_string());
        }
    }
    out
}

/// Apakah source memiliki deklarasi module/interface top-level (untuk
/// ensure-top: hindari append module kosong pada file yang sudah punya).
fn has_module_or_interface(src: &str) -> bool {
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("module ") || t.starts_with("interface ") {
            return true;
        }
    }
    false
}

/// Batas hasil perakitan dependensi: di atas ini kembalikan source mentah
/// (rantai include/instance project besar bisa meledak → swap-thrash).
pub const MAX_RESOLVED_BYTES: usize = 8 * 1024 * 1024;

/// Resolve dependensi seed secara rekursif (import prepend + instance append
/// + ensure top module). Depth guard mencegah siklus.
pub fn resolve(src: &str, idx: &DepsIndex, depth: usize) -> String {
    if depth > 8 || src.len() > MAX_RESOLVED_BYTES {
        return src.to_string();
    }

    let mut prefix = String::new();
    let mut suffix = String::new();

    // 1. Imports → prepend package source (recursive).
    for pkg in collect_imports(src) {
        if let Some(path) = idx.packages.get(&pkg) {
            if let Ok(content) = std::fs::read_to_string(path) {
                let resolved = resolve(&content, idx, depth + 1);
                // Dedup: jangan prepend package yang sudah ada di prefix/src.
                let marker = format!("package {pkg};");
                if !prefix.contains(&marker) && !src.contains(&marker) {
                    prefix.push_str(&resolved);
                    prefix.push('\n');
                }
            }
        }
    }

    // 2. Instances → append module source (recursive).
    for m in collect_instances(src, idx) {
        if let Some(path) = idx.modules.get(&m) {
            if let Ok(content) = std::fs::read_to_string(path) {
                let resolved = resolve(&content, idx, depth + 1);
                let marker = format!("module {m}");
                if !suffix.contains(&marker) && !src.contains(&marker) {
                    suffix.push_str(&resolved);
                    suffix.push('\n');
                }
            }
        }
    }

    let mut out = String::new();
    if prefix.len() + src.len() + suffix.len() > MAX_RESOLVED_BYTES {
        // Rakitan melebihi batas → pakai source asli (tanpa resolve) —
        // hindari blowup memori dari fan-out deps project besar.
        return src.to_string();
    }
    if !prefix.is_empty() {
        out.push_str(&prefix);
        out.push('\n');
    }
    out.push_str(src);
    if !suffix.is_empty() {
        out.push('\n');
        out.push_str(&suffix);
    }
    // 3. Ensure top: file tanpa module/interface (package-only, mis. area dv)
    //    gagal "Unable to determine top-level design" → tambah module kosong.
    if !has_module_or_interface(&out) {
        out.push_str("\nmodule fz_top_probe; endmodule\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_resolve_import() {
        let dir = std::env::temp_dir().join(format!("fz_deps_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let pkg = dir.join("my_pkg.sv");
        let top = dir.join("top.sv");
        std::fs::write(
            &pkg,
            "package my_pkg;\n  parameter int W = 8;\nendpackage\n",
        )
        .unwrap();
        std::fs::write(
            &top,
            "import my_pkg::*;\nmodule top;\n  initial $finish;\nendmodule\n",
        )
        .unwrap();
        let mut idx = DepsIndex::default();
        index_file(&pkg, &mut idx);
        index_file(&top, &mut idx);
        assert!(idx.packages.contains_key("my_pkg"));
        let src = std::fs::read_to_string(&top).unwrap();
        let out = resolve(&src, &idx, 0);
        assert!(out.contains("package my_pkg;"));
        assert!(out.contains("module top;"));
        let _ = std::fs::remove_file(&pkg);
        let _ = std::fs::remove_file(&top);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_top_for_package_only() {
        let src = "package only_pkg;\n  parameter int X = 1;\nendpackage\n";
        let idx = DepsIndex::default();
        let out = resolve(src, &idx, 0);
        assert!(out.contains("module fz_top_probe"));
        assert!(out.contains("package only_pkg;"));
    }

    #[test]
    fn append_instance_module() {
        let dir = std::env::temp_dir().join(format!("fz_depsi_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sub = dir.join("sub.sv");
        let top = dir.join("t.sv");
        std::fs::write(
            &sub,
            "module sub(input a, output b);\n  assign b = a;\nendmodule\n",
        )
        .unwrap();
        std::fs::write(
            &top,
            "module t(input i, output o);\n  sub u_sub(.a(i), .b(o));\nendmodule\n",
        )
        .unwrap();
        let mut idx = DepsIndex::default();
        index_file(&sub, &mut idx);
        index_file(&top, &mut idx);
        let src = std::fs::read_to_string(&top).unwrap();
        let out = resolve(&src, &idx, 0);
        assert!(out.contains("module sub("), "out:\n{out}");
        let _ = std::fs::remove_file(&sub);
        let _ = std::fs::remove_file(&top);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
