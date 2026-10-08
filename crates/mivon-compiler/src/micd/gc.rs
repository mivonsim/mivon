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
}
