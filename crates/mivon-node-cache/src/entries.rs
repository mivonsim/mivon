//! Tipe entri cache yang dipakai pipeline: metadata file, hasil verifikasi,
//! index simbol, graph file, dan profil build.
//!
//! Semua tipe di sini adalah PAYLOAD dari node — serialisasi/penyimpanannya
//! ditangani `project.rs` (object content-addressed + manifest), bukan struct
//! ini.

use crate::dependency::file_checksum;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Nama toolchain/fingerprint implementasi (invalidasi saat versi berubah).
pub const TOOLCHAIN: &str = concat!("mivon-node-cache-", env!("CARGO_PKG_VERSION"));

/// Skema payload (naikkan bila struct payload berubah).
pub const FILE_SCHEMA: u32 = 2;
pub const PREPROC_SCHEMA: u32 = 2;
pub const VERIFY_SCHEMA: u32 = 2;
pub const DIAG_SCHEMA: u32 = 2;
pub const INDEX_SCHEMA: u32 = 2;

/// Status kompilasi sebuah file pada build terakhir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    New,
    Unchanged,
    Recompiled,
    Error,
}

/// Metadata satu file sumber.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: PathBuf,
    /// Hash isi (xxh3-64) — dasar invalidasi, bukan timestamp.
    pub content_hash: u64,
    pub mtime_ns: u64,
    pub size: u64,
    pub status: FileStatus,
    /// Hash flags compiler (defines/incdirs) saat build.
    pub flags_hash: u64,
    /// Dependency file-level (instansiasi/import/include).
    pub deps: Vec<PathBuf>,
    /// File include + hash isinya saat build.
    pub include_hashes: Vec<(PathBuf, u64)>,
    pub compiled_at_ns: u64,
    pub ast_format_version: u64,
}

/// Hasil preprocess satu file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreprocEntry {
    pub content_hash: u64,
    pub combined: String,
    pub timescale: Option<(String, String)>,
    /// Segmen `` `timescale `` dalam koordinat source gabungan (LRM §19.8).
    pub timescale_segments: Vec<(usize, (String, String))>,
}

/// Kategori analisis verifikasi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VerifyCheckKind {
    Parse,
    Elaborate,
    Lint,
    Width,
    Race,
    Xprop,
    Cdc,
    Fsm,
    Dataflow,
    Timing,
    Coverage,
    Assertion,
}

impl VerifyCheckKind {
    pub const ALL: [VerifyCheckKind; 12] = [
        VerifyCheckKind::Parse,
        VerifyCheckKind::Elaborate,
        VerifyCheckKind::Lint,
        VerifyCheckKind::Width,
        VerifyCheckKind::Race,
        VerifyCheckKind::Xprop,
        VerifyCheckKind::Cdc,
        VerifyCheckKind::Fsm,
        VerifyCheckKind::Dataflow,
        VerifyCheckKind::Timing,
        VerifyCheckKind::Coverage,
        VerifyCheckKind::Assertion,
    ];
}

/// Hasil satu kategori check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub ok: bool,
    pub err_count: usize,
    pub warn_count: usize,
    pub result_hash: u64,
}

impl CheckResult {
    pub fn fresh() -> Self {
        CheckResult {
            ok: false,
            err_count: 0,
            warn_count: 0,
            result_hash: 0,
        }
    }

    pub fn pass(result_hash: u64) -> Self {
        CheckResult {
            ok: true,
            err_count: 0,
            warn_count: 0,
            result_hash,
        }
    }
}

/// Hasil verifikasi satu file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyResult {
    pub content_hash: u64,
    pub ast_hash: u64,
    pub semantic_hash: u64,
    pub ir_hash: u64,
    pub parse_ok: bool,
    pub elab_ok: bool,
    pub err_count: usize,
    pub warn_count: usize,
    pub info_count: usize,
    pub parse_ms: u64,
    pub elab_ms: u64,
    pub result_hash: u64,
    pub verified_at_ns: u64,
    pub checks: HashMap<VerifyCheckKind, CheckResult>,
}

impl VerifyResult {
    pub fn fresh(content_hash: u64) -> Self {
        VerifyResult {
            content_hash,
            ast_hash: 0,
            semantic_hash: 0,
            ir_hash: 0,
            parse_ok: false,
            elab_ok: false,
            err_count: 0,
            warn_count: 0,
            info_count: 0,
            parse_ms: 0,
            elab_ms: 0,
            result_hash: 0,
            verified_at_ns: now_ns(),
            checks: HashMap::new(),
        }
    }

    pub fn ok(&self) -> bool {
        self.parse_ok && self.elab_ok && self.err_count == 0
    }

    pub fn matches_ast(&self, ast_hash: u64) -> bool {
        self.ast_hash != 0 && self.ast_hash == ast_hash
    }

    pub fn matches_semantic(&self, semantic_hash: u64) -> bool {
        self.semantic_hash != 0 && self.semantic_hash == semantic_hash
    }

    pub fn check(&self, kind: VerifyCheckKind) -> Option<&CheckResult> {
        self.checks.get(&kind)
    }

    pub fn check_reusable(&self, kind: VerifyCheckKind, result_hash: u64) -> bool {
        self.checks
            .get(&kind)
            .map(|c| c.ok && c.result_hash == result_hash)
            .unwrap_or(false)
    }

    pub fn set_check(&mut self, kind: VerifyCheckKind, r: CheckResult) {
        self.checks.insert(kind, r);
    }
}

/// Waktu unix dalam nanoseconds.
pub fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Hash kombinasi flags compiler (defines + incdirs).
pub fn flags_hash(defines: &[(String, String)], incdirs: &[PathBuf]) -> u64 {
    let mut h = 0u64;
    for (k, v) in defines {
        h = h.wrapping_mul(31).wrapping_add(crate::dependency::checksum64(k.as_bytes()));
        h = h.wrapping_mul(31).wrapping_add(crate::dependency::checksum64(v.as_bytes()));
    }
    for d in incdirs {
        h = h.wrapping_mul(31).wrapping_add(crate::dependency::checksum64(
            d.to_string_lossy().as_bytes(),
        ));
    }
    h
}

/// Hash canonical sebuah path.
pub fn path_hash(path: &std::path::Path) -> u64 {
    crate::dependency::checksum64(path.to_string_lossy().as_bytes())
}

/// Index simbol: nama → (jenis, file definisi) + penggunaan per file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SymbolIndex {
    entries: Vec<(String, String, PathBuf)>,
    defs: HashMap<String, PathBuf>,
    uses: HashMap<PathBuf, Vec<String>>,
}

impl SymbolIndex {
    pub fn new() -> Self {
        SymbolIndex::default()
    }

    /// Daftarkan simbol `name` berjenis `kind` di `file`.
    pub fn add(&mut self, name: String, kind: String, file: PathBuf) {
        if self
            .entries
            .iter()
            .any(|(n, k, f)| *n == name && *k == kind && *f == file)
        {
            return;
        }
        self.entries.push((name, kind, file));
    }

    /// Catat definisi simbol (dependency level simbol).
    pub fn set_def(&mut self, name: String, file: PathBuf) {
        self.defs.insert(name, file);
    }

    /// Catat pemakaian simbol oleh sebuah file.
    pub fn add_use(&mut self, file: PathBuf, name: String) {
        let list = self.uses.entry(file).or_default();
        if !list.iter().any(|n| *n == name) {
            list.push(name);
        }
    }

    /// File tempat `name` (dengan `kind`) didefinisikan.
    pub fn locate(&self, name: &str, kind: &str) -> Option<&PathBuf> {
        self.entries
            .iter()
            .find(|(n, k, _)| n == name && k == kind)
            .map(|(_, _, f)| f)
    }

    /// Semua nama simbol unik.
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = self.entries.iter().map(|(n, _, _)| n.clone()).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Banyak simbol unik.
    pub fn len(&self) -> usize {
        self.names().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Pemakaian simbol oleh file.
    pub fn uses_of(&self, file: &std::path::Path) -> &[String] {
        self.uses.get(file).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Graph file-level: edge dependency + definisi simbol.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileGraph {
    forward: HashMap<PathBuf, Vec<PathBuf>>,
    defs: HashMap<String, PathBuf>,
}

impl FileGraph {
    pub fn new() -> Self {
        FileGraph::default()
    }

    pub fn set_file_deps(&mut self, file: PathBuf, deps: Vec<PathBuf>) {
        if deps.is_empty() {
            self.forward.remove(&file);
        } else {
            self.forward.insert(file, deps);
        }
    }

    pub fn deps_of(&self, file: &std::path::Path) -> Vec<PathBuf> {
        self.forward.get(file).cloned().unwrap_or_default()
    }

    pub fn set_def(&mut self, symbol: String, file: PathBuf) {
        self.defs.insert(symbol, file);
    }

    pub fn def_of(&self, symbol: &str) -> Option<&PathBuf> {
        self.defs.get(symbol)
    }

    /// Jumlah edge dependency.
    pub fn len(&self) -> usize {
        self.forward.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    pub fn files(&self) -> Vec<PathBuf> {
        self.forward.keys().cloned().collect()
    }

    /// Transitive closure file yang TERDAMPAK bila `changed` berubah
    /// (menelusuri edge kebalikan: A bergantung B ⇒ B berubah ⇒ A terdampak).
    pub fn affected(&self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let mut reverse: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for (file, deps) in &self.forward {
            for dep in deps {
                reverse
                    .entry(dep.clone())
                    .or_default()
                    .push(file.clone());
            }
        }
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut stack: Vec<PathBuf> = changed.to_vec();
        while let Some(cur) = stack.pop() {
            if let Some(dependents) = reverse.get(&cur) {
                for f in dependents {
                    if seen.insert(f.clone()) {
                        stack.push(f.clone());
                    }
                }
            }
        }
        let mut out: Vec<PathBuf> = seen.into_iter().collect();
        out.sort();
        out
    }
}

/// Profil satu build.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildProfile {
    pub build_id: u64,
    pub total_ms: u64,
    pub preprocess_ms: u64,
    pub lex_ms: u64,
    pub parse_ms: u64,
    pub elaborate_ms: u64,
    pub optimize_ms: u64,
    pub verify_ms: u64,
    pub save_ms: u64,
    pub files: usize,
    pub changed_files: usize,
    pub dirty_nodes: usize,
    pub restored_designs: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub peak_mem_kb: u64,
    pub snapshot_id: u64,
    pub verified_at_ns: u64,
}

/// Kumpulan profil build (append terbatas).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsDb {
    pub profiles: Vec<BuildProfile>,
    pub max_profiles: usize,
}

impl Default for StatsDb {
    fn default() -> Self {
        StatsDb {
            profiles: Vec::new(),
            max_profiles: 256,
        }
    }
}

impl StatsDb {
    pub fn new() -> Self {
        StatsDb::default()
    }

    pub fn record(&mut self, p: BuildProfile) {
        self.profiles.push(p);
        while self.profiles.len() > self.max_profiles {
            self.profiles.remove(0);
        }
    }

    pub fn total_builds(&self) -> usize {
        self.profiles.len()
    }

    pub fn last(&self) -> Option<&BuildProfile> {
        self.profiles.last()
    }

    pub fn avg_total_ms(&self) -> u64 {
        if self.profiles.is_empty() {
            return 0;
        }
        let sum: u64 = self.profiles.iter().map(|p| p.total_ms).sum();
        sum / self.profiles.len() as u64
    }

    pub fn hit_rate_pct(&self) -> u8 {
        let hits: usize = self.profiles.iter().map(|p| p.cache_hits).sum();
        let misses: usize = self.profiles.iter().map(|p| p.cache_misses).sum();
        let total = hits + misses;
        if total == 0 {
            return 0;
        }
        ((hits as f64 / total as f64) * 100.0) as u8
    }

    pub fn next_profile(&self) -> BuildProfile {
        BuildProfile {
            build_id: self.profiles.last().map(|l| l.build_id + 1).unwrap_or(1),
            verified_at_ns: now_ns(),
            ..Default::default()
        }
    }
}

/// Peak RSS (VmHWM) dalam KiB dari `/proc/self/status`. Non-Linux → 0.
pub fn peak_rss_kb() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Statistik setelah `ProjectCache::save()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub files: usize,
    pub restored_designs: usize,
    pub changed_files: usize,
    pub verify_hits: usize,
    pub verify_misses: usize,
    pub snapshot_id: u64,
    pub bytes_written: u64,
}

/// Metadata file saat capture (hash isi + ukuran).
pub fn capture_file_meta(
    path: &std::path::Path,
    flags: u64,
    status: FileStatus,
    deps: Vec<PathBuf>,
    include_hashes: Vec<(PathBuf, u64)>,
) -> Option<FileMeta> {
    let bytes = std::fs::read(path).ok()?;
    let content_hash = crate::dependency::checksum64(&bytes);
    let mtime_ns = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some(FileMeta {
        path: path.to_path_buf(),
        content_hash,
        mtime_ns,
        size: bytes.len() as u64,
        status,
        flags_hash: flags,
        deps,
        include_hashes,
        compiled_at_ns: now_ns(),
        ast_format_version: crate::ast::AST_FORMAT_VERSION,
    })
}

/// Apakah isi file berubah dibanding metadata tersimpan.
pub fn content_changed(meta: Option<&FileMeta>, path: &std::path::Path) -> bool {
    let Some(meta) = meta else { return true };
    file_checksum(path) != Some(meta.content_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_index_queries() {
        let mut s = SymbolIndex::new();
        s.add("uart".into(), "module".into(), PathBuf::from("uart.sv"));
        s.add("uart".into(), "package".into(), PathBuf::from("p.sv"));
        assert_eq!(s.locate("uart", "module"), Some(&PathBuf::from("uart.sv")));
        assert_eq!(s.locate("uart", "package"), Some(&PathBuf::from("p.sv")));
        assert!(s.locate("nope", "module").is_none());
        assert_eq!(s.len(), 1);
        s.add_use(PathBuf::from("top.sv"), "uart".into());
        assert_eq!(s.uses_of(&PathBuf::from("top.sv")), &["uart".to_string()]);
    }

    #[test]
    fn file_graph_edges() {
        let mut g = FileGraph::new();
        g.set_file_deps(PathBuf::from("a.sv"), vec![PathBuf::from("b.sv")]);
        g.set_def("b".into(), PathBuf::from("b.sv"));
        assert_eq!(g.deps_of(&std::path::Path::new("a.sv")), vec![PathBuf::from("b.sv")]);
        assert_eq!(g.def_of("b"), Some(&PathBuf::from("b.sv")));
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn stats_db_bounds() {
        let mut db = StatsDb::new();
        db.max_profiles = 2;
        for i in 0..4u64 {
            let mut p = BuildProfile::default();
            p.build_id = i + 1;
            db.record(p);
        }
        assert_eq!(db.total_builds(), 2);
        assert_eq!(db.last().unwrap().build_id, 4);
    }

    #[test]
    fn verify_checks_are_independent() {
        let mut v = VerifyResult::fresh(1);
        v.set_check(VerifyCheckKind::Width, CheckResult::pass(9));
        assert!(v.check_reusable(VerifyCheckKind::Width, 9));
        assert!(!v.check_reusable(VerifyCheckKind::Width, 8));
        assert!(!v.check_reusable(VerifyCheckKind::Lint, 9));
    }
}
