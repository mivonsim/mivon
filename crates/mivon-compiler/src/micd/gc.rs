//! gc.rs — garbage collection untuk MICD (Kritik 6 db.md).
//!
//! Cache persistent (ast.mdb, preproc.mdb, verify.mdb) tumbuh tanpa batas
//! lintas build: 200 compile → 20GB → 50GB → 120GB. `run_gc` membatasi
//! pertumbuhan dengan tiga mekanisme:
//!
//! * **LRU budget** — buang entry paling lama diakses sampai total bytes
//!   turun di bawah budget.
//! * **TTL** — buang entry yang tidak diakses selama `ttl_ns`.
//! * **Compaction** — buang entry yang tidak lagi tercapai dari metadata
//!   (file sudah tidak ada / tidak direferensikan): AST/preproc untuk path
//!   yang tidak terdaftar, verify untuk content hash yang tidak dipakai.
//!
//! Entry yang ter-evict dibangun ulang otomatis di compile berikutnya
//! (cost-nya sama dengan cold cache) — GC tidak pernah menghapus metadata,
//! graph, symbol, atau type index (inti incremental).
//!
//! GC berjalan best-effort saat `save()`; tidak ada jaminan hard limit.

use std::collections::HashSet;
use std::path::PathBuf;

use super::verify::now_ns;
use super::MicdDatabase;

/// Konfigurasi GC.
#[derive(Debug, Clone, PartialEq)]
pub struct GcConfig {
    /// Budget total bytes AST cache. 0 = tanpa batas LRU.
    pub ast_budget_bytes: u64,
    /// Budget total bytes preprocessed source. 0 = tanpa batas.
    pub preproc_budget_bytes: u64,
    /// Maksimum entry verify yang disimpan. 0 = tanpa batas.
    pub max_verify_entries: usize,
    /// Entry yang tidak diakses selama ini (ns) di-buang. 0 = nonaktif.
    pub ttl_ns: u64,
    /// Jalankan compaction (buang entry unreachable dari metadata).
    pub compact: bool,
}

impl Default for GcConfig {
    fn default() -> Self {
        GcConfig {
            // 256MB AST + 64MB preproc + 50k verify entry, TTL 7 hari.
            ast_budget_bytes: 256 * 1024 * 1024,
            preproc_budget_bytes: 64 * 1024 * 1024,
            max_verify_entries: 50_000,
            ttl_ns: 7 * 24 * 3600 * 1_000_000_000,
            compact: true,
        }
    }
}

/// Ringkasan hasil GC.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GcStats {
    pub evicted_ast: usize,
    pub evicted_preproc: usize,
    pub evicted_verify: usize,
    pub compacted_ast: usize,
    pub compacted_preproc: usize,
    pub compacted_verify: usize,
    /// Bytes yang dibebaskan dari cache (AST + preproc).
    pub freed_bytes: u64,
    /// Cache berubah → perlu save ulang.
    pub changed: bool,
}

impl GcStats {
    fn any(&self) -> bool {
        self.evicted_ast > 0
            || self.evicted_preproc > 0
            || self.evicted_verify > 0
            || self.compacted_ast > 0
            || self.compacted_preproc > 0
            || self.compacted_verify > 0
    }
}

/// Jalankan GC pada database sesuai konfigurasi. Mengembalikan ringkasan.
pub fn run_gc(db: &mut MicdDatabase, cfg: &GcConfig) -> GcStats {
    let mut st = GcStats::default();
    let now = now_ns();

    // ── TTL (Kritik 6: TTL). ──
    if cfg.ttl_ns > 0 {
        let cutoff = now.saturating_sub(cfg.ttl_ns);
        db.ast_accessed.retain(|path, at| {
            if *at < cutoff {
                if let Some((_, b)) = db.ast_cache.remove(path) {
                    st.evicted_ast += 1;
                    db.ast_bytes = db.ast_bytes.saturating_sub(b.len() as u64);
                    st.freed_bytes += b.len() as u64;
                }
                false
            } else {
                true
            }
        });
        db.preproc_accessed.retain(|path, at| {
            if *at < cutoff {
                if let Some(e) = db.preproc_cache.remove(path) {
                    st.evicted_preproc += 1;
                    db.preproc_bytes = db.preproc_bytes.saturating_sub(e.combined.len() as u64);
                    st.freed_bytes += e.combined.len() as u64;
                }
                false
            } else {
                true
            }
        });
        db.verify_accessed.retain(|hash, at| {
            if *at < cutoff {
                if let Some(v) = db.verify.remove(hash) {
                    if v.ast_hash != 0 {
                        db.verify_ast_index.remove(&v.ast_hash);
                    }
                    if v.semantic_hash != 0 {
                        db.verify_semantic_index.remove(&v.semantic_hash);
                    }
                    st.evicted_verify += 1;
                }
                false
            } else {
                true
            }
        });
    }

    // ── LRU budget (Kritik 6: LRU). ──
    if cfg.ast_budget_bytes > 0 && db.ast_bytes > cfg.ast_budget_bytes {
        evict_lru_ast(db, cfg.ast_budget_bytes, &mut st);
    }
    if cfg.preproc_budget_bytes > 0 && db.preproc_bytes > cfg.preproc_budget_bytes {
        evict_lru_preproc(db, cfg.preproc_budget_bytes, &mut st);
    }
    if cfg.max_verify_entries > 0 && db.verify.len() > cfg.max_verify_entries {
        evict_lru_verify(db, cfg.max_verify_entries, &mut st);
    }

    // ── Compaction (Kritik 6: compaction). ──
    if cfg.compact {
        compact_unreachable(db, &mut st);
    }

    if st.any() {
        st.changed = true;
        // Entry cache ter-evict → store terkait perlu ditulis ulang agar
        // versi disk tidak menyimpan entry yang sudah dibuang (sinkron).
        if st.evicted_ast > 0 || st.compacted_ast > 0 {
            db.dirty_ast = true;
        }
        if st.evicted_preproc > 0 || st.compacted_preproc > 0 {
            db.dirty_preproc = true;
        }
        if st.evicted_verify > 0 || st.compacted_verify > 0 {
            db.dirty_verify = true;
        }
        if st.evicted_ast > 0
            || st.evicted_preproc > 0
            || st.evicted_verify > 0
            || st.compacted_ast > 0
            || st.compacted_preproc > 0
            || st.compacted_verify > 0
        {
            db.dirty = true;
        }
    }
    st
}

/// LRU eviction AST: buang entry paling lama diakses sampai bytes ≤ budget.
fn evict_lru_ast(db: &mut MicdDatabase, budget: u64, st: &mut GcStats) {
    let mut order: Vec<(PathBuf, u64)> = db
        .ast_accessed
        .iter()
        .map(|(p, at)| (p.clone(), *at))
        .collect();
    order.sort_by_key(|(_, at)| *at);
    for (path, _) in order {
        if db.ast_bytes <= budget {
            break;
        }
        if let Some((_, b)) = db.ast_cache.remove(&path) {
            db.ast_accessed.remove(&path);
            db.ast_bytes = db.ast_bytes.saturating_sub(b.len() as u64);
            st.evicted_ast += 1;
            st.freed_bytes += b.len() as u64;
        }
    }
}

fn evict_lru_preproc(db: &mut MicdDatabase, budget: u64, st: &mut GcStats) {
    let mut order: Vec<(PathBuf, u64)> = db
        .preproc_accessed
        .iter()
        .map(|(p, at)| (p.clone(), *at))
        .collect();
    order.sort_by_key(|(_, at)| *at);
    for (path, _) in order {
        if db.preproc_bytes <= budget {
            break;
        }
        if let Some(e) = db.preproc_cache.remove(&path) {
            db.preproc_accessed.remove(&path);
            db.preproc_bytes = db.preproc_bytes.saturating_sub(e.combined.len() as u64);
            st.evicted_preproc += 1;
            st.freed_bytes += e.combined.len() as u64;
        }
    }
}

fn evict_lru_verify(db: &mut MicdDatabase, max: usize, st: &mut GcStats) {
    let mut order: Vec<(u64, u64)> = db.verify_accessed.iter().map(|(h, at)| (*h, *at)).collect();
    order.sort_by_key(|(_, at)| *at);
    for (hash, _) in order {
        if db.verify.len() <= max {
            break;
        }
        if let Some(v) = db.verify.remove(&hash) {
            db.verify_accessed.remove(&hash);
            if v.ast_hash != 0 {
                db.verify_ast_index.remove(&v.ast_hash);
            }
            if v.semantic_hash != 0 {
                db.verify_semantic_index.remove(&v.semantic_hash);
            }
            st.evicted_verify += 1;
        }
    }
}

/// Compaction: buang entry yang tidak tercapai dari metadata. File yang
/// tidak lagi terdaftar → AST/preproc dianggap sampah (tidak pernah akan
/// di-restore). Verify entry yang content hash-nya tidak dipakai file mana
/// pun → sampah.
fn compact_unreachable(db: &mut MicdDatabase, st: &mut GcStats) {
    let known: HashSet<&PathBuf> = db.files.keys().collect();
    let live_hashes: HashSet<u64> = db.files.values().map(|m| m.content_hash).collect();

    // AST & preproc: path tidak terdaftar → buang.
    let stale_ast: Vec<PathBuf> = db
        .ast_cache
        .keys()
        .filter(|p| !known.contains(p))
        .cloned()
        .collect();
    for p in stale_ast {
        if let Some((_, b)) = db.ast_cache.remove(&p) {
            db.ast_accessed.remove(&p);
            db.ast_bytes = db.ast_bytes.saturating_sub(b.len() as u64);
            st.compacted_ast += 1;
            st.freed_bytes += b.len() as u64;
        }
    }
    let stale_pre: Vec<PathBuf> = db
        .preproc_cache
        .keys()
        .filter(|p| !known.contains(p))
        .cloned()
        .collect();
    for p in stale_pre {
        if let Some(e) = db.preproc_cache.remove(&p) {
            db.preproc_accessed.remove(&p);
            db.preproc_bytes = db.preproc_bytes.saturating_sub(e.combined.len() as u64);
            st.compacted_preproc += 1;
            st.freed_bytes += e.combined.len() as u64;
        }
    }

    // Verify: content hash tidak dipakai file mana pun → buang. Kecuali
    // entry itu masih dirujuk sebagai AST-reuse (verify_ast_index menunjuk
    // padanya dari content hash lain yang hidup) — jaga agar tetap berfungsi.
    let reachable_verify: HashSet<u64> = live_hashes
        .iter()
        .copied()
        .flat_map(|h| {
            std::iter::once(h).chain(
                db.verify
                    .get(&h)
                    .map(|v| {
                        let mut out = Vec::new();
                        if v.ast_hash != 0 {
                            out.extend(db.verify_ast_index.get(&v.ast_hash).copied());
                        }
                        out
                    })
                    .unwrap_or_default(),
            )
        })
        .collect();
    let stale_verify: Vec<u64> = db
        .verify
        .keys()
        .filter(|h| !reachable_verify.contains(h))
        .copied()
        .collect();
    for h in stale_verify {
        if let Some(v) = db.verify.remove(&h) {
            db.verify_accessed.remove(&h);
            if v.ast_hash != 0 {
                db.verify_ast_index.remove(&v.ast_hash);
            }
            if v.semantic_hash != 0 {
                db.verify_semantic_index.remove(&v.semantic_hash);
            }
            st.compacted_verify += 1;
        }
    }
}

// ─── Fase 4 (Kritik F): GC terpadu — satu budget global, reklamasi pid ───
//
// Sebelumnya: 21 budget independen (`default_budget`: 256MB×3 + 64MB×8 +
// 8MB×10 ≈ 1,4GB per pid) + GC state terpisah + registry menampung pid
// tanpa batas. Kini: SATU budget global per pid (bobot per kategori hanya
// pembagi proporsional, bukan 21 anggaran mandiri) + penegakan global
// lintas-kategori (evict terlama di SELURUH layer) + reklamasi pid idle.

use super::cache::{CacheCategory, CacheLayer};
use std::path::Path;

/// Satu budget global untuk SELURUH 21 kategori cache per pid (Fase 4).
/// Menggantikan ±1,4GB anggaran independen dengan 512MB bersama.
pub const GLOBAL_CACHE_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

/// Bobot proporsional kategori (mewarisi ekspektasi ukuran lama, kini hanya
/// pembagi satu pool — bukan anggaran mandiri).
pub fn category_weight(cat: CacheCategory) -> u64 {
    match cat {
        CacheCategory::Preprocess | CacheCategory::Parser | CacheCategory::Elaborate => 256,
        CacheCategory::Lexer
        | CacheCategory::Semantic
        | CacheCategory::Optimize
        | CacheCategory::Dependency
        | CacheCategory::Hierarchy
        | CacheCategory::Simulation
        | CacheCategory::Waveform
        | CacheCategory::Coverage => 64,
        _ => 8,
    }
}

/// Bagi satu budget global ke 21 store proporsional bobot (Fase 4).
/// Dipanggil tiap open dan tiap save (sebelum GC) — satu titik kebijakan.
pub fn apply_global_budget(layer: &mut CacheLayer, global: u64) {
    let total: u64 = CacheCategory::ALL.iter().map(|c| category_weight(*c)).sum();
    if total == 0 {
        return;
    }
    for cat in CacheCategory::ALL {
        if let Some(st) = layer.store_mut(cat) {
            st.budget_bytes = global.saturating_mul(category_weight(cat)) / total;
        }
    }
}

/// Penegakan global ketat lintas-kategori (Fase 4): selama total bytes layer
/// melebihi `global`, buang entry TERLAMA di seluruh layer (bukan per-store).
/// Mengembalikan entry dibuang. Berhenti bila tidak ada kemajuan (aman).
pub fn enforce_global_budget(layer: &mut CacheLayer, global: u64) -> usize {
    let mut removed = 0usize;
    loop {
        let total: u64 = CacheCategory::ALL
            .iter()
            .filter_map(|c| layer.store(*c))
            .map(|s| s.bytes())
            .sum();
        if total <= global {
            break;
        }
        let oldest = CacheCategory::ALL
            .iter()
            .filter_map(|c| layer.store(*c).and_then(|s| s.oldest_key().map(|(k, at)| (*c, k, at))))
            .min_by_key(|(_, _, at)| *at);
        let Some((cat, key, _)) = oldest else {
            break;
        };
        let st = match layer.store_mut(cat) {
            Some(s) => s,
            None => break,
        };
        if st.remove(&key).is_none() {
            break;
        }
        removed += 1;
        st.dirty = true;
    }
    removed
}

/// Orkestrasi GC cache terpadu (Fase 4): budget proporsional → GC per-store
/// (TTL+LRU+sweep) → penegakan global ketat. Satu entry point kebijakan.
pub fn run_cache_gc(layer: &mut CacheLayer, global: u64) -> usize {
    apply_global_budget(layer, global);
    let mut n = layer.run_gc();
    n += enforce_global_budget(layer, global);
    n
}

/// Konfigurasi reklamasi pid idle (Fase 4).
#[derive(Debug, Clone, PartialEq)]
pub struct ReclaimConfig {
    /// Pertahankan maksimal N pid terbaru. 0 = tanpa batas jumlah.
    pub max_pids: usize,
    /// Pid yang tidak dibangun selama ini dihapus. 0 = nonaktif.
    pub idle_ttl_ns: u64,
}

impl Default for ReclaimConfig {
    fn default() -> Self {
        ReclaimConfig {
            // 16 pid + idle 30 hari: konservatif (cache rebuild otomatis).
            max_pids: 16,
            idle_ttl_ns: 30 * 24 * 3600 * 1_000_000_000,
        }
    }
}

/// Reklamasi pid idle (Fase 4): hapus direktori state/objects/cache/
// precompiled/locks pid yang (a) melebihi `max_pids` (terlama dulu) atau
/// (b) idle lebih dari `idle_ttl_ns`. `self_pid` (pemanggil) TIDAK pernah
/// dihapus; pid yang lock writer-nya aktif dilewati (K1: jangan merusak
/// eksklusi writer antar-proses). Registry diperbarui. Mengembalikan pid
/// yang dihapus. Best-effort (kegagalan satu pid tidak menghentikan lain).
/// Cache = data turunan (dibangun ulang otomatis) — penghapusan aman.
pub fn reclaim_idle_projects(
    db_root: &Path,
    self_pid: &str,
    cfg: &ReclaimConfig,
) -> Vec<String> {
    let reg_path = db_root.join(super::FILE_REGISTRY);
    let mut map: std::collections::HashMap<String, super::ProjectInfo> =
        std::fs::read(&reg_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
    if map.is_empty() {
        return Vec::new();
    }
    let now = super::verify::now_ns();
    let mut by_age: Vec<(String, u64)> =
        map.iter().map(|(p, i)| (p.clone(), i.last_built_ns)).collect();
    by_age.sort_by_key(|(_, at)| *at);
    let mut doomed: Vec<String> = Vec::new();
    // (b) idle melampaui TTL — dari yang terlama.
    if cfg.idle_ttl_ns > 0 {
        let cutoff = now.saturating_sub(cfg.idle_ttl_ns);
        for (pid, at) in &by_age {
            if *at < cutoff && !doomed.contains(pid) {
                doomed.push(pid.clone());
            }
        }
    }
    // (a) melebihi max_pids — buang terlama hingga sisa max_pids.
    if cfg.max_pids > 0 {
        let live = by_age.len().saturating_sub(doomed.len());
        if live > cfg.max_pids {
            let mut need = live - cfg.max_pids;
            for (pid, _) in &by_age {
                if need == 0 {
                    break;
                }
                if !doomed.contains(pid) {
                    doomed.push(pid.clone());
                    need -= 1;
                }
            }
        }
    }
    let mut gone = Vec::new();
    for pid in doomed {
        // S1: jangan hapus pid pemanggil sendiri. K1: lewati pid yang
        // writer-nya aktif (lock ada = sedang save di proses lain).
        if pid == self_pid {
            continue;
        }
        let lock = super::lock_path(db_root, &pid);
        if super::lock::is_writer_locked(&lock) {
            continue;
        }
        let st = super::state_dir(db_root, &pid);
        let objs = super::objects_dir(db_root, &pid);
        let cache = db_root.join(super::cache::DIR_CACHE).join(&pid);
        let pre = db_root.join(super::DIR_PRECOMPILED).join(&pid);
        let _ = std::fs::remove_dir_all(&st);
        let _ = std::fs::remove_dir_all(&objs);
        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&pre);
        let _ = std::fs::remove_file(&lock);
        // Verifikasi: direktori pid benar-benar hilang sebelum drop registry.
        if !st.exists() && !objs.exists() && !cache.exists() && !pre.exists() {
            map.remove(&pid);
            gone.push(pid);
        }
    }
    if !gone.is_empty() {
        let data = serde_json::to_vec_pretty(&map).unwrap_or_default();
        if !data.is_empty() {
            let _ = super::format::write_tmp(&reg_path, &data);
            let _ = super::format::commit_tmp(&reg_path);
        }
    }
    gone
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::micd::verify::VerifyResult;

    fn root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mivon_micd_gc_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_lru_ast_budget_evicts_oldest() {
        let root = root("lru");
        let mut db = MicdDatabase::open(&root);
        // 2 AST @ 100B, budget 150B → 1 ter-evict (paling lama diakses).
        db.ast_accessed.insert("a.sv".into(), 1);
        db.ast_cache.insert("a.sv".into(), (1, vec![0u8; 100].into()));
        db.ast_bytes += 100;
        db.ast_accessed.insert("b.sv".into(), 2);
        db.ast_cache.insert("b.sv".into(), (2, vec![1u8; 100].into()));
        db.ast_bytes += 100;

        let cfg = GcConfig {
            ast_budget_bytes: 150,
            preproc_budget_bytes: 0,
            max_verify_entries: 0,
            ttl_ns: 0,
            compact: false,
        };
        let st = run_gc(&mut db, &cfg);
        assert_eq!(st.evicted_ast, 1);
        assert!(!db.ast_cache.contains_key(&PathBuf::from("a.sv")));
        assert!(db.ast_cache.contains_key(&PathBuf::from("b.sv")));
        assert!(db.ast_bytes <= 150);
        assert!(st.changed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_ttl_evicts_stale() {
        let root = root("ttl");
        let mut db = MicdDatabase::open(&root);
        let now = now_ns();
        // a.sv diakses 10 detik lalu; b.sv baru saja.
        db.ast_accessed.insert("a.sv".into(), now - 10_000_000_000);
        db.ast_cache.insert("a.sv".into(), (1, vec![0u8; 10].into()));
        db.ast_bytes += 10;
        db.ast_accessed.insert("b.sv".into(), now);
        db.ast_cache.insert("b.sv".into(), (2, vec![1u8; 10].into()));
        db.ast_bytes += 10;

        let cfg = GcConfig {
            ast_budget_bytes: 0,
            preproc_budget_bytes: 0,
            max_verify_entries: 0,
            ttl_ns: 5_000_000_000, // 5 detik
            compact: false,
        };
        let st = run_gc(&mut db, &cfg);
        assert_eq!(st.evicted_ast, 1);
        assert!(!db.ast_cache.contains_key(&PathBuf::from("a.sv")));
        assert!(db.ast_cache.contains_key(&PathBuf::from("b.sv")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_compaction_removes_unreachable() {
        let root = root("compact");
        let mut db = MicdDatabase::open(&root);
        // File terdaftar: a.sv (content hash 100).
        db.files.insert(
            "a.sv".into(),
            crate::micd::metadata::FileMeta {
                path: "a.sv".into(),
                content_hash: 100,
                mtime_ns: 0,
                size: 10,
                status: crate::micd::metadata::FileStatus::Unchanged,
                flags_hash: 0,
                deps: vec![],
                include_hashes: vec![],
                compiled_at_ns: 0,
                ast_format_version: 1,
            },
        );
        // AST untuk a.sv (valid) dan ghost.sv (sampah).
        db.ast_accessed.insert("a.sv".into(), 1);
        db.ast_cache.insert("a.sv".into(), (100, vec![0u8; 5].into()));
        db.ast_bytes += 5;
        db.ast_accessed.insert("ghost.sv".into(), 1);
        db.ast_cache.insert("ghost.sv".into(), (999, vec![1u8; 5].into()));
        db.ast_bytes += 5;
        // Verify untuk hash 100 (hidup) dan 999 (sampah).
        db.set_verify(VerifyResult::fresh(100));
        db.set_verify(VerifyResult::fresh(999));

        let cfg = GcConfig {
            ast_budget_bytes: 0,
            preproc_budget_bytes: 0,
            max_verify_entries: 0,
            ttl_ns: 0,
            compact: true,
        };
        let st = run_gc(&mut db, &cfg);
        assert!(db.ast_cache.contains_key(&PathBuf::from("a.sv")));
        assert!(!db.ast_cache.contains_key(&PathBuf::from("ghost.sv")));
        assert_eq!(st.compacted_ast, 1);
        assert!(db.verify.contains_key(&100));
        assert!(!db.verify.contains_key(&999));
        assert_eq!(st.compacted_verify, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_verify_budget_lru() {
        let root = root("vbudget");
        let mut db = MicdDatabase::open(&root);
        db.set_verify(VerifyResult::fresh(1));
        db.set_verify(VerifyResult::fresh(2));
        db.set_verify(VerifyResult::fresh(3));
        // Paksa order akses: 1 lama, 3 baru.
        db.verify_accessed.insert(1, 1);
        db.verify_accessed.insert(2, 2);
        db.verify_accessed.insert(3, 3);

        let cfg = GcConfig {
            ast_budget_bytes: 0,
            preproc_budget_bytes: 0,
            max_verify_entries: 2,
            ttl_ns: 0,
            compact: false,
        };
        let st = run_gc(&mut db, &cfg);
        assert_eq!(st.evicted_verify, 1);
        assert!(!db.verify.contains_key(&1));
        assert!(db.verify.contains_key(&2));
        assert!(db.verify.contains_key(&3));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Fase 4 (Kritik F): satu budget global + reklamasi pid ──

    #[test]
    fn test_fase4_budget_global_terbagi_proporsional() {
        use crate::micd::cache::{CacheCategory, CacheLayer};
        let root = root("weights");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
        // Jumlah budget ≈ global (rugi pembulatan < jumlah kategori).
        let total: u64 = CacheCategory::ALL
            .iter()
            .filter_map(|c| layer.store(*c))
            .map(|s| s.budget_bytes)
            .sum();
        assert!(total <= GLOBAL_CACHE_BUDGET_BYTES);
        assert!(total + CacheCategory::ALL.len() as u64 > GLOBAL_CACHE_BUDGET_BYTES);
        // Proporsi benar: elaborate (bobot 256) > lint (bobot 8).
        let total_w: u64 = CacheCategory::ALL.iter().map(|c| crate::micd::gc::category_weight(*c)).sum();
        let big = layer.store(CacheCategory::Elaborate).unwrap().budget_bytes;
        let small = layer.store(CacheCategory::Lint).unwrap().budget_bytes;
        assert_eq!(big, GLOBAL_CACHE_BUDGET_BYTES * 256 / total_w);
        assert_eq!(small, GLOBAL_CACHE_BUDGET_BYTES * 8 / total_w);
        assert!(big > small * 10);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase4_enforce_global_evict_terlama_lintas_kategori() {
        use crate::micd::cache::{CacheCategory, CacheLayer};
        use crate::micd::cache::index::CacheIndexEntry;
        let root = root("global");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
        // Matikan budget per-store agar hanya penegakan global yang bekerja.
        for cat in CacheCategory::ALL {
            layer.store_mut(cat).unwrap().budget_bytes = 0;
            layer.store_mut(cat).unwrap().ttl_ns = 0;
        }
        // Parser: entry TUA 100B; Lexer: entry BARU 100B. Global 150B.
        layer.put(CacheCategory::Parser, "tua", &[1u8; 100]).unwrap();
        layer.put(CacheCategory::Lexer, "baru", &[2u8; 100]).unwrap();
        // Atur accessed_ns manual (tua=1, baru=2).
        set_accessed(&mut layer, CacheCategory::Parser, "tua", 1);
        set_accessed(&mut layer, CacheCategory::Lexer, "baru", 2);
        let removed = enforce_global_budget(&mut layer, 150);
        assert_eq!(removed, 1, "satu entry dibuang hingga <= 150B");
        assert!(
            !layer.contains(CacheCategory::Parser, "tua"),
            "yang terlama (lintas kategori) dibuang duluan"
        );
        assert!(layer.contains(CacheCategory::Lexer, "baru"));
        let total: u64 = CacheCategory::ALL
            .iter()
            .filter_map(|c| layer.store(*c))
            .map(|s| s.bytes())
            .sum();
        assert!(total <= 150);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase4_reclaim_pid_idle() {
        use crate::micd::cache::DIR_CACHE;
        let db = root("reclaim");
        std::fs::create_dir_all(&db).unwrap();
        // 3 pid palsu: 2 idle (last_built 0/1), 1 aktif (now).
        let mut map: std::collections::HashMap<String, crate::micd::ProjectInfo> =
            std::collections::HashMap::new();
        for (pid, at) in [("pid_tua", 1u64), ("pid_tua2", 2u64)] {
            map.insert(
                pid.to_string(),
                crate::micd::ProjectInfo {
                    root: "/x".into(),
                    source_count: 1,
                    sources: vec![],
                    created_ns: 0,
                    last_built_ns: at,
                    compiler_version: String::new(),
                    binary_fingerprint: 0,
                },
            );
            std::fs::create_dir_all(db.join("state").join(pid)).unwrap();
            std::fs::create_dir_all(db.join("objects").join(pid)).unwrap();
            std::fs::create_dir_all(db.join(DIR_CACHE).join(pid)).unwrap();
        }
        let now = crate::micd::verify::now_ns();
        map.insert(
            "pid_aktif".to_string(),
            crate::micd::ProjectInfo {
                root: "/x".into(),
                source_count: 1,
                sources: vec![],
                created_ns: now,
                last_built_ns: now,
                compiler_version: String::new(),
                binary_fingerprint: 0,
            },
        );
        std::fs::create_dir_all(db.join("state").join("pid_aktif")).unwrap();
        let reg = db.join(crate::micd::FILE_REGISTRY);
        std::fs::write(&reg, serde_json::to_vec_pretty(&map).unwrap()).unwrap();
        // Reclaim: max 2 pid + idle TTL (cutoff di antara at=2 dan now).
        let cfg = ReclaimConfig {
            max_pids: 2,
            idle_ttl_ns: now.saturating_sub(10),
        };
        let gone = reclaim_idle_projects(&db, "pid_aktif", &cfg);
        assert_eq!(gone.len(), 2);
        assert!(!db.join("state").join("pid_tua").exists());
        assert!(!db.join("objects").join("pid_tua2").exists());
        assert!(!db.join(DIR_CACHE).join("pid_tua").exists());
        assert!(db.join("state").join("pid_aktif").exists(), "pid aktif selamat");
        let map2: std::collections::HashMap<String, crate::micd::ProjectInfo> =
            serde_json::from_slice(&std::fs::read(&reg).unwrap()).unwrap();
        assert!(!map2.contains_key("pid_tua"));
        assert!(map2.contains_key("pid_aktif"));
        let _ = std::fs::remove_dir_all(&db);
    }

    #[test]
    fn test_fase4_reclaim_lewati_terkunci_dan_diri_sendiri() {
        // K1/S1: pid terkunci (writer aktif) + pid sendiri tidak dihapus.
        let db = root("reclaim_skip");
        std::fs::create_dir_all(&db).unwrap();
        let mut map: std::collections::HashMap<String, crate::micd::ProjectInfo> =
            std::collections::HashMap::new();
        for pid in ["pid_lock", "pid_self"] {
            map.insert(
                pid.to_string(),
                crate::micd::ProjectInfo {
                    root: "/x".into(),
                    source_count: 1,
                    sources: vec![],
                    created_ns: 0,
                    last_built_ns: 1,
                    compiler_version: String::new(),
                    binary_fingerprint: 0,
                },
            );
            std::fs::create_dir_all(db.join("state").join(pid)).unwrap();
        }
        // Simulasikan writer aktif di pid_lock (file lock ada).
        let locks = db.join(crate::micd::DIR_LOCKS);
        std::fs::create_dir_all(&locks).unwrap();
        std::fs::write(locks.join("pid_lock.lock"), b"x").unwrap();
        let reg = db.join(crate::micd::FILE_REGISTRY);
        std::fs::write(&reg, serde_json::to_vec_pretty(&map).unwrap()).unwrap();
        let cfg = ReclaimConfig {
            max_pids: 0,
            idle_ttl_ns: 1000,
        };
        let gone = reclaim_idle_projects(&db, "pid_self", &cfg);
        assert!(gone.is_empty(), "terkunci + diri sendiri harus selamat: {:?}", gone);
        assert!(db.join("state").join("pid_lock").exists());
        assert!(db.join("state").join("pid_self").exists());
        let _ = std::fs::remove_dir_all(&db);
    }

    #[test]
    fn test_fase4_reclaim_tidak_hapus_bila_di_bawah_batas() {
        let db = root("reclaim_noop");
        std::fs::create_dir_all(&db).unwrap();
        let now = crate::micd::verify::now_ns();
        let mut map: std::collections::HashMap<String, crate::micd::ProjectInfo> =
            std::collections::HashMap::new();
        map.insert(
            "pid_a".to_string(),
            crate::micd::ProjectInfo {
                root: "/x".into(),
                source_count: 1,
                sources: vec![],
                created_ns: now,
                last_built_ns: now,
                compiler_version: String::new(),
                binary_fingerprint: 0,
            },
        );
        std::fs::create_dir_all(db.join("state").join("pid_a")).unwrap();
        let reg = db.join(crate::micd::FILE_REGISTRY);
        std::fs::write(&reg, serde_json::to_vec_pretty(&map).unwrap()).unwrap();
        let gone = reclaim_idle_projects(&db, "pid_a", &ReclaimConfig::default());
        assert!(gone.is_empty(), "di bawah batas → tidak ada yang dihapus");
        assert!(db.join("state").join("pid_a").exists());
        let _ = std::fs::remove_dir_all(&db);
    }

    /// Helper test: paksa accessed_ns satu key (indeks diuji via perilaku).
    fn set_accessed(
        layer: &mut crate::micd::cache::CacheLayer,
        cat: crate::micd::cache::CacheCategory,
        key: &str,
        at: u64,
    ) {
        let st = layer.store_mut(cat).unwrap();
        st.set_accessed_for_test(key, at);
    }
}
