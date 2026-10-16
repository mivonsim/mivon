//! Seed corpus: loader direktori + fallback builtin minimal (hanya guard).
//!
//! Empat sumber seed:
//! - `fuzz/corpus/seeds/` — SystemVerilog RTL mandiri (`.sv/.svh/.v/.vh`)
//! - `fuzz/corpus/mv/` — Mivon HDL DSL `.mv` (di-transpile on-the-fly ke SV)
//! - Bila `fuzz/corpus/resources/` ada — project real yang di-copy
//! - Project NYATA langsung (tanpa copy): `cva6/`, `openc910/`,
//!   `opentitan/`, `rocket-chip/` — di-scan otomatis via filelist roda di
//!   corpus dir.
//!
//! Project real punya dependensi lintas file (`include "pkg.svh"`) — seed
//! di-resolve include-nya otomatis saat diambil (inline recursive), jadi
//! compile standalone tetap jalan tanpa copy manual.

use std::path::{Path, PathBuf};

/// Batas aman memuat seed ke memori. Seed di atas ini dicatat sebagai
/// `skipped_large` (eksplisit, bukan dibuang senyap) — kampanye tak lagi
/// "lupa" seed raksasa (RTL real cva6/opentitan sering >4MB).
pub const MAX_SEED_BYTES: u64 = 32 * 1024 * 1024;

/// Seed corpus. Load sekali per kampanye.
pub struct Corpus {
    seeds: Vec<PathBuf>,
    /// Index dependency (package/module/interface name → file) utk resolve
    /// import/submodule seed project real (area dv/UVM yang tadinya skip).
    deps_index: crate::deps::DepsIndex,
    /// Seed > `MAX_SEED_BYTES` — di-HITUNG, bukan dibuang senyap.
    pub skipped_large: usize,
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

        // Batas total seed (hindari jutaan file) — naikkan utk area penuh.
        if seeds.len() > 100_000 {
            seeds.truncate(100_000);
        }

        // Akun seed raksasa: JANGAN buang senyap. Seed <= MAX_SEED_BYTES
        // dipertahankan; sisanya di-HITUNG sebagai `skipped_large` (terlihat
        // di laporan) sebelum dibuang agar load tak meledak OOM. Cap ini
        // praktis tak pernah kena (file RTL real < 2MB) — seed > SIZE_CAP
        // yang muncul dari inlining include ditangani jalur large-aware di
        // `run_single` (mutate_light + timeout 4×), bukan dibuang.
        let mut skipped_large = 0usize;
        seeds.retain(|p| {
            let ok = p
                .metadata()
                .map(|m| m.len() <= MAX_SEED_BYTES)
                .unwrap_or(false);
            if !ok {
                skipped_large += 1;
            }
            ok
        });

        // Index dependency (package/module/interface name → file) dibangun
        // SEKALI utk seluruh corpus — dipakai resolve deps di random_seed
        // (import/submodule seed real, terutama area dv/UVM opentitan).
        let deps_index = crate::deps::build_index(&seeds);

        Self {
            seeds,
            deps_index,
            skipped_large,
        }
    }

    /// Project real RTL (tanpa copy): scan cva6/core, openc910/gen_rtl,
    /// opentitan/hw, rocket-chip. Skip dv/tb/verif untuk kurangi noise
    /// dependensi.
    fn push_real_projects(seeds: &mut Vec<PathBuf>) {
        // Cari workspace root (dir dengan Cargo.toml yang punya [workspace]):
        // test cwd = package dir, CLI cwd = root — konsisten naik ke root.
        let root = workspace_root();
        let roots: &[(&str, &str)] = &[
            ("cva6", "core"),
            ("openc910", "C910_RTL_FACTORY/gen_rtl"),
            ("opentitan", "hw"),
            // rocket-chip: RTL hasil-generate Chisel ada di `out/**/compile.dest`
            // (502 `.sv`), harness/TB Verilog di `src/main/resources/vsrc`.
            // `out/` juga memuat artefak `.class` — `walk_rtl` memfilter
            // ekstensi (.sv/.v/.svh/.vh) jadi hanya RTL yang masuk.
            ("rocket-chip", "out"),
            ("rocket-chip", "src/main/resources/vsrc"),
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
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if name.starts_with('.') || name == "database" || name == "cache" {
                        continue;
                    }
                }
                Self::walk_rtl(&p, seeds, depth + 1);
            } else if p.is_file() && Self::supported_ext(&p) {
                seeds.push(p);
            }
        }
    }

    fn push_dir(seeds: &mut Vec<PathBuf>, dir: &Path, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                // Jangan seret cache/MICD ke corpus: `.mivon/`, dotdir,
                // `database/`, `cache/` = artefak, bukan RTL.
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if name.starts_with('.') || name == "database" || name == "cache" {
                        continue;
                    }
                }
                Self::push_dir(seeds, &p, depth + 1);
            } else if p.is_file() && Self::supported_ext(&p) {
                seeds.push(p);
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

    /// Load + resolve seed by index (sama seperti `random_seed` tapi indeks
    /// eksplisit). Dipakai seed-coverage plan agar tiap seed dijamah >=1x.
    pub fn seed_by_index(&self, idx: usize) -> Option<SeedSource> {
        self.seed_at_idx(idx)
    }

    /// Gabung source dgn pasangan `tb_<stem>.sv` bila ada (arah design→tb).
    /// Dipakai verify iverilog: design tanpa stimulus → marker kosong →
    /// compare VAKUM (tidak bermakna). Tanpa pasangan → source apa adanya.
    ///
    /// Arah balik (tb→design): seed `tb_*` standalone tak memuat DUT →
    /// iverilog gagal compile → RefUnavailable selamanya (separuh corpus
    /// tak terverifikasi). Strip `tb_` → cari `<stem>.sv`/`.v` → prepend.
    pub fn pair_with_tb(&self, path: &std::path::Path, src: &str) -> String {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return src.to_string();
        };
        if let Some(mod_stem) = stem.strip_prefix("tb_") {
            for ext in ["sv", "v"] {
                let mod_name = format!("{mod_stem}.{ext}");
                for i in 0..self.len() {
                    if let Some(p) = self.seed_at(i) {
                        if p.file_name().and_then(|s| s.to_str()) == Some(mod_name.as_str()) {
                            if let Ok(mod_src) = std::fs::read_to_string(p) {
                                return format!("{mod_src}\n{src}");
                            }
                        }
                    }
                }
            }
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

/// Cap hasil inlining include: di atas ini berhenti me-resolve (baris
/// berikutnya disalin apa adanya) — cegah blowup rantai include project.
const INLINE_CAP_BYTES: usize = 8 * 1024 * 1024;

/// Inline recursive `` `include "x.svh" `` dengan pencarian relatif terhadap
/// file + ancestor. Depth guard 8. Baris include digantikan isi file include.
fn resolve_includes(src: &str, file: &Path, depth: usize) -> String {
    if depth > 8 {
        return src.to_string();
    }
    let base_dir = file.parent().unwrap_or(Path::new("."));
    let mut out = String::with_capacity(src.len().min(4 * 1024 * 1024));
    for line in src.lines() {
        if out.len() > INLINE_CAP_BYTES {
            out.push_str(line);
            out.push('\n');
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_counts_large_seeds_instead_of_dropping() {
        let dir = std::env::temp_dir().join(format!("fzcorp_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let small = dir.join("s.sv");
        std::fs::write(&small, "module m; endmodule").unwrap();
        // Seed raksasa (> MAX_SEED_BYTES) — dulu dibuang senyap oleh `retain`.
        let large = dir.join("big.sv");
        let _ = std::fs::File::create(&large).and_then(|mut f| {
            f.write_all(&vec![b' '; MAX_SEED_BYTES as usize + 1024])
        });
        let corpus = Corpus::load(Some(&dir));
        assert_eq!(corpus.len(), 1, "seed kecil tetap dipertahankan");
        assert_eq!(
            corpus.skipped_large, 1,
            "seed raksasa di-HITUNG, bukan dibuang tanpa jejak"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_no_large_when_all_small() {
        let dir = std::env::temp_dir().join(format!("fzcorp2_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("a.sv"), "module a; endmodule").unwrap();
        std::fs::write(dir.join("b.sv"), "module b; endmodule").unwrap();
        let corpus = Corpus::load(Some(&dir));
        assert_eq!(corpus.len(), 2);
        assert_eq!(corpus.skipped_large, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pairing dua arah: design→tb DAN tb→design (dulu tb_* diteruskan
    /// mentah tanpa DUT → verify iverilog RefUnavailable selamanya).
    #[test]
    fn pair_with_tb_bidirectional() {
        let dir = std::env::temp_dir().join(format!("fzpair_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("01_counter.sv"), "module counter; endmodule").unwrap();
        std::fs::write(dir.join("tb_01_counter.sv"), "module tb; endmodule").unwrap();
        let corpus = Corpus::load(Some(&dir));
        // design → tb ditempel di belakang.
        let fwd = corpus.pair_with_tb(
            &dir.join("01_counter.sv"),
            "module counter; endmodule",
        );
        assert!(fwd.contains("module tb;"), "design harus ditempeli tb");
        // tb → design di-prepend di depan.
        let rev = corpus.pair_with_tb(&dir.join("tb_01_counter.sv"), "module tb; endmodule");
        assert!(rev.contains("module counter;"), "tb harus ditempeli DUT");
        assert!(rev.find("module counter;").unwrap() < rev.find("module tb;").unwrap());
        // Tanpa pasangan → apa adanya.
        let lone = corpus.pair_with_tb(&dir.join("tb_lone.sv"), "module tb_lone; endmodule");
        assert_eq!(lone, "module tb_lone; endmodule");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
