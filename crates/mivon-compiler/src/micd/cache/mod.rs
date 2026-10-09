//! cache/ — lapisan cache pipeline per kategori (db.md "Saya akan mendesain
//! cache/ seperti ini", baris 1141-1605).
//!
//! Di bawah `database/cache/<pid>/` hidup satu store seragam untuk tiap tahap
//! kompilasi (lexer, parser, semantic, …, profile). [`CacheLayer`] adalah
//! aggregator: membuka 21 kategori, menyediakan API seragam put/get, dan
//! menyimpan store yang berubah dalam satu save.
//!
//! ```text
//! database/
//!     cache/<pid>/
//!         preprocess/   … seragam: manifest.mdb + objects/index/blobs/temp/
//!         lexer/          journal/stats/lock  (lihat [`super::store`])
//!         …
//! ```

pub mod category;
pub mod index;
pub mod manifest;
pub mod pipeline;
pub mod stage;
pub mod stats;
pub mod store;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

pub use category::CacheCategory;
pub use manifest::{CacheManifest, CACHE_SCHEMA_VERSION};
pub use stage::{stage_schema_version, StageKey, StageStore, StageValue};
pub use stats::{CacheLayerStats, CategoryStats};
pub use store::CategoryStore;

use store::{DIR_BLOBS, DIR_OBJECTS, DIR_TEMP};

/// Nama direktori lapisan cache di database root (`database/cache/`).
pub const DIR_CACHE: &str = "cache";

/// Budget default per kategori (GC LRU). Kategori data besar diberi budget
/// lebih besar; record KV lebih kecil.
pub fn default_budget(cat: CacheCategory) -> u64 {
    match cat {
        CacheCategory::Preprocess | CacheCategory::Parser | CacheCategory::Elaborate => {
            256 * 1024 * 1024
        }
        CacheCategory::Lexer
        | CacheCategory::Semantic
        | CacheCategory::Optimize
        | CacheCategory::Dependency
        | CacheCategory::Hierarchy
        | CacheCategory::Simulation
        | CacheCategory::Waveform
        | CacheCategory::Coverage => 64 * 1024 * 1024,
        _ => 8 * 1024 * 1024,
    }
}

/// TTL default (7 hari — Kritik 6 db.md).
pub const DEFAULT_TTL_NS: u64 = 7 * 24 * 3600 * 1_000_000_000;

/// Aggregator lapisan `cache/` per project.
pub struct CacheLayer {
    /// `<db_root>/cache/<pid>/`.
    pub root: PathBuf,
    stores: HashMap<CacheCategory, CategoryStore>,
    /// Jalankan GC (TTL+LRU) sebelum save.
    pub gc_on_save: bool,
}

impl std::fmt::Debug for CacheLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheLayer")
            .field("root", &self.root)
            .field("categories", &self.stores.len())
            .field("stats", &self.stats().summary())
            .finish()
    }
}

impl CacheLayer {
    /// Buka lapisan cache untuk sebuah project. Membuat 21 store (struktur
    /// seragam) di `<db_root>/cache/<pid>/`. Gagal (root tak bisa ditulis) →
    /// `Err` (pemanggil bisa memakai `None` — cache best-effort).
    pub fn open(db_root: &Path, pid: &str, config_hash: u64) -> io::Result<CacheLayer> {
        let root = db_root.join(DIR_CACHE).join(pid);
        std::fs::create_dir_all(&root)?;
        let mut stores = HashMap::with_capacity(CacheCategory::ALL.len());
        for cat in CacheCategory::ALL {
            let mut st = CategoryStore::open(&root.join(cat.name()), cat, config_hash);
            st.budget_bytes = default_budget(cat);
            st.ttl_ns = DEFAULT_TTL_NS;
            stores.insert(cat, st);
        }
        Ok(CacheLayer {
            root,
            stores,
            gc_on_save: true,
        })
    }

    /// Direktori satu kategori.
    pub fn category_dir(&self, cat: CacheCategory) -> PathBuf {
        self.root.join(cat.name())
    }

    pub fn store(&self, cat: CacheCategory) -> Option<&CategoryStore> {
        self.stores.get(&cat)
    }

    pub fn store_mut(&mut self, cat: CacheCategory) -> Option<&mut CategoryStore> {
        self.stores.get_mut(&cat)
    }

    /// Simpan payload untuk `key` di kategori `cat`. Kunci dapat berupa path
    /// file, nama module, hash hex, dsb. Mengembalikan content hash (CAS).
    pub fn put(&mut self, cat: CacheCategory, key: &str, bytes: &[u8]) -> Option<u64> {
        self.stores.get_mut(&cat)?.put(key, bytes)
    }

    /// Ambil payload `key` dari kategori `cat`.
    pub fn get(&mut self, cat: CacheCategory, key: &str) -> Option<Vec<u8>> {
        self.stores.get_mut(&cat)?.get(key)
    }

    pub fn contains(&self, cat: CacheCategory, key: &str) -> bool {
        self.stores
            .get(&cat)
            .map(|s| s.contains(key))
            .unwrap_or(false)
    }

    pub fn remove(&mut self, cat: CacheCategory, key: &str) -> bool {
        self.stores
            .get_mut(&cat)
            .map(|s| s.remove(key).is_some())
            .unwrap_or(false)
    }

    // ── Fase 1: API bertipe via StageStore (satu engine, facade generik) ──
    //
    // Jalur baru untuk kode bertipe; jalur mentah put/get di atas tetap ada
    // (kompatibel). Byte di disk identik — hanya encode/decode di tepi.

    /// Simpan nilai bertipe di kategori `cat` (Fase 1: `StageStore`).
    pub fn put_typed<K: StageKey, V: StageValue>(
        &mut self,
        cat: CacheCategory,
        key: &K,
        value: &V,
    ) -> Option<u64> {
        let st = self.stores.get_mut(&cat)?;
        let k = key.encode_key();
        let bytes = value.encode_value();
        st.put(&k, &bytes)
    }

    /// Ambil nilai bertipe dari kategori `cat` (lazy dari disk).
    pub fn get_typed<K: StageKey, V: StageValue>(
        &mut self,
        cat: CacheCategory,
        key: &K,
    ) -> Option<V> {
        let st = self.stores.get_mut(&cat)?;
        let bytes = st.get(&key.encode_key())?;
        V::decode_value(&bytes)
    }

    /// Cek keberadaan kunci bertipe (tanpa membaca payload).
    pub fn contains_typed<K: StageKey>(&self, cat: CacheCategory, key: &K) -> bool {
        self.stores
            .get(&cat)
            .map(|s| s.contains(&key.encode_key()))
            .unwrap_or(false)
    }

    /// Hapus kunci bertipe. `false` bila tak ada.
    pub fn remove_typed<K: StageKey>(&mut self, cat: CacheCategory, key: &K) -> bool {
        self.stores
            .get_mut(&cat)
            .map(|s| s.remove(&key.encode_key()).is_some())
            .unwrap_or(false)
    }

    /// Versi skema stage untuk `cat` (fondasi invalidasi per-stage Fase 3).
    pub fn stage_schema(&self, cat: CacheCategory) -> u64 {
        stage_schema_version(cat)
    }

    /// Jumlah entry satu kategori.
    pub fn entry_count(&self, cat: CacheCategory) -> usize {
        self.store(cat).map(|s| s.len()).unwrap_or(0)
    }

    /// Apakah ada store yang berubah (belum disimpan).
    pub fn is_dirty(&self) -> bool {
        self.stores.values().any(|s| s.dirty)
    }

    /// Statistik lintas kategori.
    pub fn stats(&self) -> CacheLayerStats {
        let mut out = CacheLayerStats::default();
        for s in self.stores.values() {
            let cs = s.stats();
            out.total_entries += cs.entries;
            out.total_bytes += cs.bytes;
            out.total_hits += cs.hits;
            out.total_misses += cs.misses;
            if cs.rebuilt {
                out.rebuilt += 1;
            }
            out.per_category.push(cs);
        }
        out.stores = self.stores.len();
        out
    }

    /// Simpan seluruh store yang berubah. Best-effort: error satu kategori
    /// tidak menggagalkan kategori lain.
    pub fn save(&mut self) -> io::Result<()> {
        if self.gc_on_save {
            self.run_gc();
        }
        let mut first_err = None;
        for st in self.stores.values_mut() {
            if let Err(e) = st.save() {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        if let Some(e) = first_err {
            Err(e)
        } else {
            Ok(())
        }
    }

    /// GC seluruh kategori: buang entry lewat budget/TTL + sapu objek yatim.
    /// Mengembalikan total entry yang dibuang.
    pub fn run_gc(&mut self) -> usize {
        self.stores.values_mut().map(|s| s.gc()).sum()
    }

    /// Bersihkan seluruh lapisan (index + objek semua kategori).
    pub fn clear(&mut self) -> io::Result<()> {
        let mut first_err = None;
        for st in self.stores.values_mut() {
            if let Err(e) = st.clear() {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        if let Some(e) = first_err {
            Err(e)
        } else {
            Ok(())
        }
    }

    /// Hapus seluruh direktori lapisan cache dari disk (untuk `--cache-clear`).
    pub fn remove_all(db_root: &Path, pid: &str) -> io::Result<()> {
        let dir = db_root.join(DIR_CACHE).join(pid);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        // Parent `cache/` kosong → buang juga.
        let parent = db_root.join(DIR_CACHE);
        if parent.is_dir()
            && parent
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&parent);
        }
        Ok(())
    }

    /// Path direktorat objek satu kategori (untuk tool / debug).
    pub fn objects_dir(&self, cat: CacheCategory) -> PathBuf {
        self.category_dir(cat).join(DIR_OBJECTS)
    }
    pub fn blobs_dir(&self, cat: CacheCategory) -> PathBuf {
        self.category_dir(cat).join(DIR_BLOBS)
    }

    /// Bersihkan sisa `.tmp` di temp/ (crash sebelum commit).
    pub fn clean_temp(&self) {
        for st in self.stores.values() {
            let dir = st.root.join(DIR_TEMP);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mivon_cache_layer_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_open_creates_all_categories() {
        let root = test_root("open");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let layer = CacheLayer::open(&db, "pid1", 0).unwrap();
        assert_eq!(layer.stores.len(), CacheCategory::ALL.len());
        for cat in CacheCategory::ALL {
            assert!(layer.category_dir(cat).is_dir(), "{}", cat.name());
        }
        // Struktur seragam per kategori.
        for cat in CacheCategory::ALL {
            let d = layer.category_dir(cat);
            for sub in [
                "objects", "blobs", "temp", "journal", "stats", "lock", "index",
            ] {
                assert!(d.join(sub).is_dir(), "{}/{}", cat.name(), sub);
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_put_get_across_categories() {
        let root = test_root("across");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
        layer
            .put(CacheCategory::Parser, "a.sv", b"ast-bytes")
            .unwrap();
        layer
            .put(CacheCategory::Preprocess, "a.sv", b"module a; endmodule")
            .unwrap();
        assert_eq!(
            layer.get(CacheCategory::Parser, "a.sv").unwrap(),
            b"ast-bytes"
        );
        assert_eq!(
            layer.get(CacheCategory::Preprocess, "a.sv").unwrap(),
            b"module a; endmodule"
        );
        assert!(layer.contains(CacheCategory::Parser, "a.sv"));
        assert!(!layer.contains(CacheCategory::Lexer, "a.sv"));
        assert_eq!(layer.entry_count(CacheCategory::Parser), 1);
        layer.save().unwrap();
        assert!(!layer.is_dirty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_persist_across_layer_reopen() {
        let root = test_root("reopen");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        {
            let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
            layer
                .put(CacheCategory::Semantic, "top", b"semantic")
                .unwrap();
            layer.save().unwrap();
        }
        {
            let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
            assert_eq!(
                layer.get(CacheCategory::Semantic, "top").unwrap(),
                b"semantic"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_layer_isolated_by_pid() {
        let root = test_root("pid_iso");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        {
            let mut layer = CacheLayer::open(&db, "proj_a", 0).unwrap();
            layer.put(CacheCategory::Type, "m", b"sig").unwrap();
            layer.save().unwrap();
        }
        let layer_b = CacheLayer::open(&db, "proj_b", 0).unwrap();
        assert!(
            !layer_b.contains(CacheCategory::Type, "m"),
            "project lain terisolasi"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_remove_all_and_clear() {
        let root = test_root("clear");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
        layer.put(CacheCategory::Parser, "a.sv", b"x").unwrap();
        layer.save().unwrap();
        layer.clear().unwrap();
        assert_eq!(layer.stats().total_entries, 0);
        assert!(!layer.contains(CacheCategory::Parser, "a.sv"));
        // remove_all menghapus direktori.
        CacheLayer::remove_all(&db, "pid1").unwrap();
        assert!(!db.join("cache").join("pid1").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_empty_shell_save_does_not_clobber_other_instance() {
        // Skenario env: dua instance membuka lapisan yang sama. Instance A
        // mengisi + menyimpan; instance B (shell kosong, hanya dibuka) lalu
        // ikut menyimpan — B TIDAK boleh menimpa entry A (fix global: rebuild
        // tidak menandai dirty).
        let root = test_root("shell");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        {
            // Instance A: isi + simpan.
            let mut a = CacheLayer::open(&db, "pid1", 0).unwrap();
            a.put(CacheCategory::Parser, "a.sv", b"ast-bytes").unwrap();
            a.save().unwrap();
        }
        {
            // Instance B: shell kosong (tanpa put) — save harus no-op.
            let mut b = CacheLayer::open(&db, "pid1", 0).unwrap();
            assert!(!b.is_dirty(), "shell kosong tidak dirty setelah open");
            b.save().unwrap();
        }
        {
            // Entry A masih utuh setelah save B.
            let mut c = CacheLayer::open(&db, "pid1", 0).unwrap();
            assert_eq!(c.get(CacheCategory::Parser, "a.sv").unwrap(), b"ast-bytes");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_gc_runs_across_categories() {
        let root = test_root("gc");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
        // Isi melebihi budget kecil → GC buang.
        layer.store_mut(CacheCategory::Parser).unwrap().budget_bytes = 50;
        layer.put(CacheCategory::Parser, "a", &[1u8; 40]).unwrap();
        layer.put(CacheCategory::Parser, "b", &[2u8; 40]).unwrap();
        let removed = layer.run_gc();
        assert!(removed >= 1);
        assert!(layer.entry_count(CacheCategory::Parser) <= 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Fase 1: 21 kategori lewat facade bertipe, perilaku identik ──

    #[test]
    fn test_fase1_typed_across_all_21_categories() {
        // Semua stage lewat SATU facade generik; byte di disk identik dengan
        // jalur mentah (kompatibel silang put ↔ put_typed).
        let root = test_root("fase1_all21");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        {
            let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
            assert_eq!(layer.stats().stores, CacheCategory::ALL.len());
            for cat in CacheCategory::ALL {
                let key = format!("key-{}", cat.name());
                let val = format!("val-{}", cat.name());
                layer.put_typed(cat, &key, &val).unwrap();
                assert!(layer.contains_typed(cat, &key));
                assert_eq!(layer.stage_schema(cat), 1);
            }
            layer.save().unwrap();
        }
        {
            let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
            for cat in CacheCategory::ALL {
                let key = format!("key-{}", cat.name());
                let expect = format!("val-{}", cat.name());
                // Baca bertipe…
                assert_eq!(layer.get_typed::<String, String>(cat, &key).unwrap(), expect);
                // …dan mentah (kompatibel silang, byte identik).
                assert_eq!(layer.get(cat, &key).unwrap(), expect.as_bytes());
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase1_typed_pathbuf_keys_across_reopen() {
        let root = test_root("fase1_path");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        {
            let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
            let k = PathBuf::from("rtl/top.sv");
            layer.put_typed(CacheCategory::Preprocess, &k, &12345u64).unwrap();
            layer.save().unwrap();
        }
        {
            let mut layer = CacheLayer::open(&db, "pid1", 0).unwrap();
            let k = PathBuf::from("rtl/top.sv");
            assert_eq!(
                layer.get_typed::<PathBuf, u64>(CacheCategory::Preprocess, &k).unwrap(),
                12345u64
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
