//! Seed corpus: loader direktori + fallback builtin minimal (hanya guard).
//!
//! Empat sumber seed:
//! - `fuzz/corpus/seeds/` — SystemVerilog RTL mandiri (`.sv/.svh/.v/.vh`)
//! - `fuzz/corpus/mv/` — Mivon HDL DSL `.mv` (di-transpile on-the-fly ke SV)
//! - Bila `fuzz/corpus/resources/` ada — project real yang di-copy
//! - Project NYATA langsung (tanpa copy): `cva6/`, `openc910/`,
//!   `opentitan/` — di-scan otomatis via filelist roda di corpus dir.
//!
//! Project real punya dependensi lintas file (`include "pkg.svh"`) — seed
//! di-resolve include-nya otomatis saat diambil (inline recursive), jadi
//! compile standalone tetap jalan tanpa copy manual.

use std::path::{Path, PathBuf};

/// Seed corpus. Load sekali per kampanye.
pub struct Corpus {
    seeds: Vec<PathBuf>,
    /// Index dependency (package/module/interface name → file) utk resolve
    /// import/submodule seed project real (area dv/UVM yang tadinya skip).
    deps_index: crate::deps::DepsIndex,
}

/// Source siap-evaluasi: sudah di-transpile bila `.mv`.
pub struct SeedSource {
    /// Source SV siap mutasi/eval (MV sudah di-transpile; include sudah
    /// di-resolve recursive untuk seed project real).
    pub text: String,
    /// Path asal.
    pub path: PathBuf,
    /// Apakah seed asli `.mv` (Mivon HDL).
    pub is_mv: bool,
    /// Apakah seed dari project real (include sudah di-inline).
    pub from_real_project: bool,
}

impl Corpus {
    /// Load semua seed dari dir. Urut stable (sorted).
    ///
    /// Tanpa `dir` (None): default = `fuzz/corpus/seeds` (SV) + `fuzz/corpus/mv`
    /// (Mivon HDL) + project real (cva6/openc910/opentitan bila ada). Dengan
    /// `dir` eksplisit: hanya dir itu.
    pub fn load(dir: Option<&Path>) -> Self {
        let mut seeds = Vec::new();

        match dir {
            // Eksplisit: hanya dir itu.
            Some(d) => {
                Self::push_dir(&mut seeds, d, 0);
            }
            // Default: seeds/ + mv/ + project real (no-copy).
            None => {
                let root = workspace_root();
                let base = root.join("crates/mivon-fuzz/fuzz/corpus");
                Self::push_dir(&mut seeds, &base.join("seeds"), 0);
                Self::push_dir(&mut seeds, &base.join("mv"), 0);
                Self::push_real_projects(&mut seeds);
            }
        }

        seeds.sort();
        seeds.dedup();
        // Guard: reject file terlalu besar.
        seeds.retain(|p| p.metadata().map(|m| m.len() <= 4_000_000).unwrap_or(false));

        // Batas total seed (hindari jutaan file) — naikkan utk area penuh.
        if seeds.len() > 100_000 {
            seeds.truncate(100_000);
        }

        // Index dependency (package/module/interface name → file) dibangun
        // SEKALI utk seluruh corpus — dipakai resolve deps di random_seed
        // (import/submodule seed real, terutama area dv/UVM opentitan).
        let deps_index = crate::deps::build_index(&seeds);

        Self { seeds, deps_index }
    }

    /// Project real RTL (tanpa copy): scan cva6/core, openc910/gen_rtl,
    /// opentitan/hw. Skip dv/tb/verif untuk kurangi noise dependensi.
    fn push_real_projects(seeds: &mut Vec<PathBuf>) {
        // Cari workspace root (dir dengan Cargo.toml yang punya [workspace]):
        // test cwd = package dir, CLI cwd = root — konsisten naik ke root.
        let root = workspace_root();
        let roots: &[(&str, &str)] = &[
            ("cva6", "core"),
            ("openc910", "C910_RTL_FACTORY/gen_rtl"),
            ("opentitan", "hw"),
        ];
        for (proj, sub) in roots {
            let dir = root.join(proj).join(sub);
            if dir.exists() {
                Self::walk_rtl(&dir, seeds, 0);
            }
        }
    }

    /// Walk rekursif, ambil `.sv/.v/.svh` — SEMUA area termasuk dv/tb/verif/tests
    /// (ingin ukur seberapa rusak mivon secara penuh).
    fn walk_rtl(dir: &Path, seeds: &mut Vec<PathBuf>, depth: usize) {
        if depth > 12 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                Self::walk_rtl(&p, seeds, depth + 1);
            } else if p.is_file() && Self::supported_ext(&p) {
                seeds.push(p);
            }
        }
    }

    fn push_dir(seeds: &mut Vec<PathBuf>, dir: &Path, _depth: usize) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && Self::supported_ext(&p) {
                    seeds.push(p);
                }
            }
        }
    }

    /// Apakah ekstensi file didukung sebagai seed.
    fn supported_ext(p: &Path) -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e, "sv" | "svh" | "v" | "vh" | "mv"))
            .unwrap_or(false)
    }

    pub fn is_empty(&self) -> bool {
        self.seeds.is_empty()
    }

    pub fn len(&self) -> usize {
        self.seeds.len()
    }

    /// Ambil seed random, sudah siap-evaluasi (MV di-transpile ke SV;
    /// project real include-nya di-resolve inline).
    pub fn random_seed(&self, rng: &mut crate::Rng) -> Option<SeedSource> {
        if self.seeds.is_empty() {
            return None;
        }
        let idx = rng.below(self.seeds.len());
        self.seed_at_idx(idx)
    }

    /// Ambil seed random KHUSUS `.mv` (subset filter ekstensi).
    ///
    /// Dipakai target MV: sampling acak dari corpus penuh (21/4664 seed
    /// `.mv`) membuat loop kampanye sering `break` di case pertama —
    /// kampanye MV dievaluasi cuma 0-2 dari 2000 case.
    pub fn random_mv_seed(&self, rng: &mut crate::Rng) -> Option<SeedSource> {
        let mv_idxs: Vec<usize> = self
            .seeds
            .iter()
            .enumerate()
            .filter(|(_, p)| p.extension().and_then(|e| e.to_str()) == Some("mv"))
            .map(|(i, _)| i)
            .collect();
        if mv_idxs.is_empty() {
            return None;
        }
        let pick = mv_idxs[rng.below(mv_idxs.len())];
        self.seed_at_idx(pick)
    }

    /// Load + resolve seed by index (baca file, inline include, resolve deps).
    fn seed_at_idx(&self, idx: usize) -> Option<SeedSource> {
        let p = self.seeds.get(idx)?;
        let is_mv = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e == "mv")
            .unwrap_or(false);
        let raw = std::fs::read_to_string(p).ok()?;
        let from_real = is_real_project(p);
        let text = if from_real && !is_mv {
            // Project real: inline recursive `` `include "x.svh" `` agar
            // compile standalone (dependency lintas file nyata) + resolve
            // import package/submodule + ensure top module (area dv/UVM
            // opentitan yang tadinya 70% skip "no top-level design").
            let inlined = resolve_includes(&raw, p, 0);
            crate::deps::resolve(&inlined, &self.deps_index, 0)
        } else {
            raw
        };
        // Return raw source — untuk `.mv` mutasi harus menyentuh DSL aslinya,
        // transpile dilakukan SETELAH mutasi (di run_single). Transpile di
        // sini membuat mutasi terjadi pada .sv hasil — bukan yang diminta.
        Some(SeedSource {
            text,
            path: p.clone(),
            is_mv,
            from_real_project: from_real,
        })
    }

    /// Ambil seed by index.
    pub fn seed_at(&self, idx: usize) -> Option<&Path> {
        self.seeds.get(idx).map(|v| &**v)
    }

    /// Gabung source dgn pasangan `tb_<stem>.sv` bila ada (arah design→tb).
    /// Dipakai verify iverilog: design tanpa stimulus → marker kosong →
    /// compare VAKUM (tidak bermakna). File tb (`tb_*`) diteruskan apa
    /// adanya; tanpa pasangan → source apa adanya.
    pub fn pair_with_tb(&self, path: &std::path::Path, src: &str) -> String {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return src.to_string();
        };
        if stem.starts_with("tb_") {
            return src.to_string();
        }
        let tb_name = format!("tb_{stem}.sv");
        for i in 0..self.len() {
            if let Some(p) = self.seed_at(i) {
                if p.file_name().and_then(|s| s.to_str()) == Some(tb_name.as_str()) {
                    if let Ok(tb) = std::fs::read_to_string(p) {
                        return format!("{src}\n{tb}");
                    }
                }
            }
        }
        src.to_string()
    }
}

/// Detect project real (bukan corpus mandiri).
fn is_real_project(p: &Path) -> bool {
    let s = p.to_string_lossy();
    is_real_seed_path(p)
        || s == "cva6"
        || s.starts_with("cva6/")
        || s == "openc910"
        || s.starts_with("openc910/")
        || s == "opentitan"
        || s.starts_with("opentitan/")
}

/// Pub wrapper untuk test/CLI: apakah path seed dari project real.
pub fn is_real_seed_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    // Absolut: segmen proyek setelah root mivon.
    for seg in ["/cva6/", "/openc910/", "/opentitan/"] {
        if s.contains(seg) {
            return true;
        }
    }
    false
}

/// Workspace root mivon: naik dari cwd sampai Cargo.toml dengan `[workspace]`.
/// Publik: dipakai main.rs (verify_mutated) agar corpus path tak
/// cwd-dependent (verify dari cwd luar workspace → corpus kosong →
/// 45/50 kasus skip karena corpus penuh tanpa marker).
pub fn workspace_root() -> PathBuf {
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let cargo = dir.join("Cargo.toml");
        if cargo.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo) {
                if content.contains("[workspace]") {
                    return dir;
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }
    // Fallback: pakai kasih dari env MIVON_ROOT atau cwd.
    std::env::var("MIVON_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Inline recursive `` `include "x.svh" `` dengan pencarian relatif terhadap
/// file + ancestor. Depth guard 8. Baris include digantikan isi file include.
fn resolve_includes(src: &str, file: &Path, depth: usize) -> String {
    if depth > 8 {
        return src.to_string();
    }
    let base_dir = file.parent().unwrap_or(Path::new("."));
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("`include") {
            if let Some(path) = extract_include_path(rest) {
                if let Some(resolved) = find_include_runtime(base_dir, &path) {
                    if let Ok(content) = std::fs::read_to_string(&resolved) {
                        let inlined = resolve_includes(&content, &resolved, depth + 1);
                        out.push_str(&inlined);
                        out.push('\n');
                        continue;
                    }
                }
                // Include tak ketemu — biarkan baris agar error terlihat.
                out.push_str(line);
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Extract nama file dari baris `` `include "x.svh" `` (atau `<x.svh>`).
fn extract_include_path(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if let Some(inner) = trimmed.strip_prefix('"').and_then(|r| r.split('"').next()) {
        return Some(inner.to_string());
    }
    if let Some(inner) = trimmed.strip_prefix('<').and_then(|r| r.split('>').next()) {
        return Some(inner.to_string());
    }
    // Tanpa kutip — kata pertama.
    trimmed.split_whitespace().next().map(|s| s.to_string())
}

/// Cari include: relative base dir, naik ancestor (depth 5), cari by basename
/// di dir yang sama.
fn find_include_runtime(base: &Path, name: &str) -> Option<PathBuf> {
    // Exak relative.
    let direct = base.join(name);
    if direct.exists() {
        return Some(direct);
    }
    // Ancestor walk.
    let mut anc = Some(base);
    let mut depth = 0;
    while let Some(d) = anc {
        let cand = d.join(name);
        if cand.exists() {
            return Some(cand);
        }
        if depth >= 5 {
            break;
        }
        anc = d.parent();
        depth += 1;
    }
    // By basename di base (file ditemukan di dir lain, nama cocok).
    let bn = Path::new(name).file_name()?;
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.file_name() == Some(bn) {
                // Hanya match bila nama sama & bukan dir — hati-hati jangan
                // ambil dir acak.
                return Some(p);
            }
        }
    }
    None
}

/// Transpile `.mv` → SV (svh + sv digabung, baris `` `include `` di-strip).
/// Mirip jalur `mivon x.mv` (F9 di main.rs). Pub agar dipakai combine_tb.
pub fn transpile_seed(p: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(p)?;
    let base = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("design")
        .to_string();
    transpile_mv_string(&raw, &base)
}

/// Transpile `.mv` string → SV (path bebas, dipakai setelah mutasi di MV).
pub fn transpile_mv_string(
    mv_src: &str,
    base_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let tr = mivon_api::mv::transpile(mv_src, base_name)?;
    let mut buf = tr.svh.clone();
    buf.push('\n');
    for line in tr.sv.lines() {
        if line.trim_start().starts_with("`include") {
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
    }
    Ok(buf)
}
