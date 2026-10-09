//! store — store disk seragam satu kategori cache/ (db.md "Saran arsitektur
//! cache": tiap cache memiliki struktur internal yang seragam).
//!
//! ```text
//! <root>/<category>/
//!     manifest.mdb     — metadata cache, versi skema, checksum, konfigurasi
//!     objects/         — objek cache berdasarkan content hash (payload kecil)
//!     index/
//!         index.mdb    — FileID/NodeID → objek cache
//!     blobs/           — data besar (AST/elaborasi) — content hash
//!     temp/            — cache yang sedang dibangun sebelum commit
//!     journal/
//!         journal.mdb  — log transaksi (recovery proses terhenti)
//!     stats/
//!         stats.mdb    — statistik hit/miss, ukuran, umur cache
//!     lock/
//!         writer.lock  — koordinasi writer (reader tanpa lock: rename atomik)
//! ```
//!
//! Payload objek content-addressed (CAS): dua key dengan isi sama berbagi satu
//! file. Objek ditulis atomik (temp + rename) saat `put`; index/manifest/stats
//! ditulis dalam satu transaksi journal (Kritik 4 & 5 db.md). GC membuang
//! entry LRU (budget + TTL) dan menyapu objek yang tidak lagi dirujuk.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use super::super::format::{MdbReader, MdbWriter, KIND_MANIFEST, KIND_STATS};
use super::super::lock::acquire_write_lock;
use super::super::txn;
use super::super::verify::now_ns;
use super::category::CacheCategory;
use super::index::{CacheIndex, CacheIndexEntry};
use super::manifest::CacheManifest;
use super::stats::CategoryStats;
use crate::cache::compute_checksum;

/// Key singleton manifest & stats di file MDB masing-masing.
const KEY_SINGLETON: u64 = 0x0000_0000_0000_0001;
/// Payload lebih besar dari ini masuk `blobs/`, sisanya `objects/`.
pub const BLOB_THRESHOLD: usize = 64 * 1024;
/// Ekstensi objek kecil/besar.
pub const OBJ_EXT: &str = "obj";
pub const BLOB_EXT: &str = "blob";

/// Nama subdirektori seragam.
pub const DIR_OBJECTS: &str = "objects";
pub const DIR_INDEX: &str = "index";
pub const DIR_BLOBS: &str = "blobs";
pub const DIR_TEMP: &str = "temp";
pub const DIR_JOURNAL: &str = "journal";
pub const DIR_STATS: &str = "stats";
pub const DIR_LOCK: &str = "lock";

/// Store disk satu kategori cache.
pub struct CategoryStore {
    pub category: CacheCategory,
    /// Root kategori: `<db>/cache/<pid>/<category>/`.
    pub root: PathBuf,
    manifest: CacheManifest,
    index: CacheIndex,
    /// Hit/miss runtime (persisten di stats/stats.mdb).
    hits: u64,
    misses: u64,
    /// Budget bytes payload (LRU GC). 0 = tanpa batas.
    pub budget_bytes: u64,
    /// TTL (ns): entry tak diakses selama ini di-buang. 0 = nonaktif.
    pub ttl_ns: u64,
    /// Ada perubahan belum disimpan.
    pub dirty: bool,
    /// Di-rebuild (schema mismatch / corrupt) pada open sesi ini.
    pub rebuilt: bool,
}

impl std::fmt::Debug for CategoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CategoryStore")
            .field("category", &self.category.name())
            .field("entries", &self.index.len())
            .field("bytes", &self.index.bytes())
            .field("dirty", &self.dirty)
            .finish()
    }
}

impl CategoryStore {
    /// Buka store kategori: buat struktur seragam, crash recovery, muat
    /// manifest + index + stats. Schema mismatch → rebuild dari kosong
    /// (Kritik 3 db.md).
    pub fn open(root: &Path, category: CacheCategory, config_hash: u64) -> CategoryStore {
        let _ = std::fs::create_dir_all(root);
        for d in [DIR_OBJECTS, DIR_BLOBS, DIR_TEMP, DIR_LOCK] {
            let _ = std::fs::create_dir_all(root.join(d));
        }
        let _ = std::fs::create_dir_all(root.join(DIR_INDEX));
        let _ = std::fs::create_dir_all(root.join(DIR_JOURNAL));
        let _ = std::fs::create_dir_all(root.join(DIR_STATS));

        // Crash recovery (Kritik 5): journal tersisa → validasi store.
        txn::recover(root, &root.join(DIR_JOURNAL).join("journal.mdb"));

        let mut st = CategoryStore {
            category,
            root: root.to_path_buf(),
            manifest: CacheManifest::fresh(config_hash),
            index: CacheIndex::new(),
            hits: 0,
            misses: 0,
            budget_bytes: 0,
            ttl_ns: 0,
            dirty: false,
            rebuilt: false,
        };

        // manifest.mdb — pintu skema. Bila tidak cocok, seluruh store
        // dianggap tidak kompatibel → dibangun ulang dari kosong.
        if let Ok(r) = MdbReader::open(&st.manifest_path()) {
            if let Some(b) = r.get(KEY_SINGLETON) {
                if let Ok(m) = bincode::deserialize::<CacheManifest>(&b) {
                    if m.valid() {
                        st.manifest = m;
                        st.rebuilt = false;
                        st.index = CacheIndex::load(&st.index_path());
                        st.load_stats();
                        return st;
                    }
                }
            }
        }
        // Rebuild: manifest/index/stats lama dibuang deterministik.
        let _ = std::fs::remove_file(st.manifest_path());
        let _ = std::fs::remove_file(st.index_path());
        let _ = std::fs::remove_file(st.stats_path());
        st.manifest = CacheManifest::fresh(config_hash);
        st.rebuilt = true;
        // PENTING (global): rebuild TIDAK menandai dirty. Store kosong yang
        // hanya dibuka ulang (shell) tidak boleh disimpan oleh `save()` —
        // mis. DatabaseContext env membuka DB di startup lalu shutdown
        // memanggil save: shell kosong itu akan MENIMPA cache berisi yang
        // ditulis instance lain (run/run_fast). Manifest/index/stats baru
        // di-persist secara lazy oleh put/get/clear pertama yang benar-benar
        // memodifikasi store.
        st
    }

    // ── Path ──

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.mdb")
    }
    pub fn index_path(&self) -> PathBuf {
        self.root.join(DIR_INDEX).join("index.mdb")
    }
    pub fn stats_path(&self) -> PathBuf {
        self.root.join(DIR_STATS).join("stats.mdb")
    }
    pub fn journal_path(&self) -> PathBuf {
        self.root.join(DIR_JOURNAL).join("journal.mdb")
    }
    fn lock_path(&self) -> PathBuf {
        self.root.join(DIR_LOCK).join("writer.lock")
    }

    /// Path objek CAS: `objects/<hex>.obj` atau `blobs/<hex>.blob`.
    pub fn object_path(&self, content_hash: u64, large: bool) -> PathBuf {
        let ext = if large { BLOB_EXT } else { OBJ_EXT };
        let dir = if large { DIR_BLOBS } else { DIR_OBJECTS };
        self.root
            .join(dir)
            .join(format!("{:016x}.{}", content_hash, ext))
    }

    // ── API ──

    /// Simpan payload untuk `key` (content-addressed). Key sama yang sudah ada
    /// akan di-overwrite (entry index diperbarui). Mengembalikan content hash.
    pub fn put(&mut self, key: &str, bytes: &[u8]) -> Option<u64> {
        if bytes.is_empty() {
            return None;
        }
        let content_hash = compute_checksum(bytes);
        let large = bytes.len() > BLOB_THRESHOLD;
        let path = self.object_path(content_hash, large);
        if !path.exists() {
            if let Err(e) = self.write_object(&path, bytes) {
                eprintln!("[cache] {} put {}: {}", self.category.name(), key, e);
                return None;
            }
        }
        let now = now_ns();
        let prev = self.index.get(key);
        let kind = if prev.map(|e| e.large) == Some(large) {
            prev.map(|e| e.kind).unwrap_or(self.category.kind())
        } else {
            self.category.kind()
        };
        self.index.put(
            key.to_string(),
            CacheIndexEntry {
                content_hash,
                size: bytes.len() as u64,
                large,
                kind,
                created_ns: prev.map(|e| e.created_ns).unwrap_or(now),
                accessed_ns: now,
            },
        );
        self.dirty = true;
        Some(content_hash)
    }

    /// Ambil payload `key`. Memperbarui LRU (accessed_ns) agar tidak di-evict.
    pub fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        let entry = match self.index.get(key) {
            Some(e) => e.clone(),
            None => {
                self.misses += 1;
                self.dirty = true;
                return None;
            }
        };
        let bytes = match self.load_object(&entry) {
            Some(b) => b,
            None => {
                // Objek hilang/corrupt → entry dianggap miss, dibuang.
                self.index.remove(key);
                self.misses += 1;
                self.dirty = true;
                return None;
            }
        };
        self.hits += 1;
        let mut e = entry;
        e.accessed_ns = now_ns();
        self.index.put(key.to_string(), e);
        self.dirty = true;
        Some(bytes)
    }

    /// Apakah `key` terdaftar.
    pub fn contains(&self, key: &str) -> bool {
        self.index.contains(key)
    }

    /// Hapus `key`. Mengembalikan ukuran payload yang dibuang.
    pub fn remove(&mut self, key: &str) -> Option<u64> {
        let e = self.index.remove(key)?;
        self.dirty = true;
        Some(e.size)
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Total bytes payload.
    pub fn bytes(&self) -> u64 {
        self.index.bytes()
    }

    /// Waktu akses terakhir `key` (ns).
    pub fn touched_ns(&self, key: &str) -> Option<u64> {
        self.index.get(key).map(|e| e.accessed_ns)
    }

    /// Test-only: paksa waktu akses satu key (determinisme test LRU).
    #[cfg(test)]
    pub(crate) fn set_accessed_for_test(&mut self, key: &str, at: u64) {
        if let Some(e) = self.index.entries.get_mut(key) {
            e.accessed_ns = at;
        }
    }

    /// Kunci dengan akses terlama + waktu aksesnya (Fase 4: GC global
    /// lintas-kategori meng-evict yang terlama di SELURUH layer, bukan
    /// per-kategori). `None` bila kosong.
    pub(crate) fn oldest_key(&self) -> Option<(String, u64)> {
        self.index
            .entries
            .iter()
            .min_by_key(|(_, e)| e.accessed_ns)
            .map(|(k, e)| (k.clone(), e.accessed_ns))
    }

    /// Content hash tersimpan untuk `key`.
    pub fn content_hash_of(&self, key: &str) -> Option<u64> {
        self.index.get(key).map(|e| e.content_hash)
    }

    /// Semua key.
    pub fn keys(&self) -> Vec<String> {
        self.index.entries.keys().cloned().collect()
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }
    pub fn misses(&self) -> u64 {
        self.misses
    }

    pub fn stats(&self) -> CategoryStats {
        let mut oldest = None;
        let mut newest = None;
        for e in self.index.entries.values() {
            oldest = Some(oldest.map_or(e.accessed_ns, |o: u64| o.min(e.accessed_ns)));
            newest = Some(newest.map_or(e.accessed_ns, |n: u64| n.max(e.accessed_ns)));
        }
        CategoryStats {
            category: self.category,
            entries: self.index.len(),
            bytes: self.index.bytes(),
            hits: self.hits,
            misses: self.misses,
            oldest_ns: oldest,
            newest_ns: newest,
            rebuilt: self.rebuilt,
        }
    }

    /// Simpan store dalam satu transaksi (journal → objek → index/manifest/
    /// stats → bersihkan journal). Skip bila tidak ada perubahan.
    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let _lock = acquire_write_lock(
            &self.lock_path(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| io::Error::other(e.to_string()))?;

        // Intent transaksi: daftar store MDB yang ikut (path absolut).
        let names = vec![self.index_path(), self.manifest_path(), self.stats_path()];
        txn::write_journal(&self.journal_path(), &names)?;

        // Update manifest.
        self.manifest.entry_count = self.index.len() as u64;
        self.manifest.bytes = self.index.bytes();
        self.manifest.updated_ns = now_ns();

        // Commit MDB (tiap file atomik temp+rename).
        self.index
            .save(&self.index_path(), self.category.compression())?;
        self.write_manifest()?;
        self.write_stats()?;

        // Sukses → bersihkan journal, sweep objek yatim.
        let _ = std::fs::remove_file(self.journal_path());
        self.sweep();
        self.dirty = false;
        Ok(())
    }

    /// GC: buang entry paling lama diakses bila melebihi budget/TTL, lalu
    /// sapu objek yang tidak lagi dirujuk. Mengembalikan jumlah entry dibuang.
    pub fn gc(&mut self) -> usize {
        let now = now_ns();
        let mut removed = 0usize;

        // TTL (Kritik 6: TTL).
        if self.ttl_ns > 0 {
            let cutoff = now.saturating_sub(self.ttl_ns);
            let stale: Vec<String> = self
                .index
                .entries
                .iter()
                .filter(|(_, e)| e.accessed_ns < cutoff)
                .map(|(k, _)| k.clone())
                .collect();
            for k in stale {
                self.index.remove(&k);
                removed += 1;
            }
        }

        // Budget LRU (Kritik 6: LRU).
        if self.budget_bytes > 0 && self.index.bytes() > self.budget_bytes {
            let mut order: Vec<(String, u64)> = self
                .index
                .entries
                .iter()
                .map(|(k, e)| (k.clone(), e.accessed_ns))
                .collect();
            order.sort_by_key(|(_, at)| *at);
            for (k, _) in order {
                if self.index.bytes() <= self.budget_bytes {
                    break;
                }
                self.index.remove(&k);
                removed += 1;
            }
        }

        if removed > 0 {
            self.dirty = true;
            self.sweep();
        }
        removed
    }

    /// Bersihkan seluruh store (index + objek) — untuk `--cache-clear`.
    pub fn clear(&mut self) -> io::Result<()> {
        let _ = acquire_write_lock(
            &self.lock_path(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        for d in [DIR_OBJECTS, DIR_BLOBS, DIR_TEMP] {
            let _ = std::fs::remove_dir_all(self.root.join(d));
            let _ = std::fs::create_dir_all(self.root.join(d));
        }
        for p in [self.index_path(), self.manifest_path(), self.stats_path()] {
            let _ = std::fs::remove_file(p);
        }
        self.index = CacheIndex::new();
        self.hits = 0;
        self.misses = 0;
        self.manifest = CacheManifest::fresh(self.manifest.config_hash);
        self.dirty = true;
        Ok(())
    }

    // ── Internal ──

    fn write_object(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Object CAS immutable + verifikasi di load → tanpa fsync per file
        // (ribuan fsync untuk design besar = bottleneck). Durability: temp +
        // rename atomik; index MDB (fsync) ditulis belakangan; checksum
        // diverifikasi saat load_object.
        super::super::format::write_tmp_unflushed(path, bytes)?;
        super::super::format::commit_tmp(path)
    }

    fn load_object(&self, e: &CacheIndexEntry) -> Option<Vec<u8>> {
        let path = self.object_path(e.content_hash, e.large);
        let bytes = std::fs::read(&path).ok()?;
        // Object CAS ditulis tanpa fsync — verifikasi isi vs content hash di
        // nama file agar file partial/corrupt (crash) tidak pernah disajikan
        // sebagai data valid: langsung jadi miss, dibangun ulang.
        if compute_checksum(&bytes) != e.content_hash {
            return None;
        }
        Some(bytes)
    }

    fn write_manifest(&self) -> io::Result<()> {
        let mut w = MdbWriter::new();
        w.put(
            KEY_SINGLETON,
            KIND_MANIFEST,
            bincode::serialize(&self.manifest).map_err(io::Error::other)?,
        );
        w.write_to(&self.manifest_path())
    }

    fn write_stats(&self) -> io::Result<()> {
        let mut w = MdbWriter::new();
        w.put(
            KEY_SINGLETON,
            KIND_STATS,
            bincode::serialize(&(self.hits, self.misses)).map_err(io::Error::other)?,
        );
        w.write_to(&self.stats_path())
    }

    fn load_stats(&mut self) {
        if let Ok(r) = MdbReader::open(&self.stats_path()) {
            if let Some(b) = r.get(KEY_SINGLETON) {
                if let Ok((h, m)) = bincode::deserialize::<(u64, u64)>(&b) {
                    self.hits = h;
                    self.misses = m;
                }
            }
        }
    }

    /// Sapu objek yatim: buang `<hash>.obj`/`<hash>.blob` yang tidak lagi
    /// dirujuk index, plus sisa `.tmp` dari crash.
    fn sweep(&self) {
        let live: HashSet<u64> = self
            .index
            .entries
            .values()
            .map(|e| e.content_hash)
            .collect();
        for (dir, ext) in [(DIR_OBJECTS, OBJ_EXT), (DIR_BLOBS, BLOB_EXT)] {
            let d = self.root.join(dir);
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            let suffix = format!(".{}", ext);
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".tmp") {
                    let _ = std::fs::remove_file(entry.path());
                    continue;
                }
                if let Some(hx) = name.strip_suffix(&suffix) {
                    if let Ok(h) = u64::from_str_radix(hx, 16) {
                        if !live.contains(&h) {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::super::manifest::CACHE_SCHEMA_VERSION;
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mivon_cache_store_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_open_creates_uniform_layout() {
        let root = test_root("layout");
        let st = CategoryStore::open(&root, CacheCategory::Parser, 0);
        for d in [
            DIR_OBJECTS,
            DIR_BLOBS,
            DIR_TEMP,
            DIR_LOCK,
            DIR_INDEX,
            DIR_JOURNAL,
            DIR_STATS,
        ] {
            assert!(root.join(d).is_dir(), "{} harus ada", d);
        }
        assert_eq!(st.len(), 0);
        assert_eq!(st.category, CacheCategory::Parser);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_put_get_roundtrip_persist() {
        let root = test_root("roundtrip");
        {
            let mut st = CategoryStore::open(&root, CacheCategory::Parser, 0);
            st.budget_bytes = 1 << 20;
            let h = st.put("a.sv", b"module a; endmodule").unwrap();
            assert_eq!(st.len(), 1);
            assert_eq!(st.content_hash_of("a.sv"), Some(h));
            assert_eq!(st.get("a.sv").unwrap(), b"module a; endmodule");
            assert!(st.contains("a.sv"));
            st.save().unwrap();
        }
        {
            let mut st = CategoryStore::open(&root, CacheCategory::Parser, 0);
            assert_eq!(st.len(), 1);
            assert_eq!(st.get("a.sv").unwrap(), b"module a; endmodule");
            assert!(!st.journal_path().exists(), "journal bersih setelah save");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_content_addressed_dedup() {
        let root = test_root("dedup");
        let mut st = CategoryStore::open(&root, CacheCategory::Lexer, 0);
        let payload = b"logic [7:0] x = 8'hAA;";
        let h1 = st.put("x.sv", payload).unwrap();
        let h2 = st.put("y.sv", payload).unwrap();
        assert_eq!(h1, h2, "isi sama → satu objek CAS");
        assert_eq!(st.len(), 2, "index beda key, objek sama");
        // Objek fisik hanya satu file.
        let objs = std::fs::read_dir(root.join(DIR_OBJECTS)).unwrap().count();
        assert_eq!(objs, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_large_payload_goes_to_blobs() {
        let root = test_root("blobs");
        let mut st = CategoryStore::open(&root, CacheCategory::Elaborate, 0);
        let big = vec![0xABu8; BLOB_THRESHOLD + 1];
        st.put("big.sv", &big).unwrap();
        assert!(root.join(DIR_BLOBS).read_dir().unwrap().next().is_some());
        assert_eq!(st.get("big.sv").unwrap(), big);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_remove_and_miss() {
        let root = test_root("remove");
        let mut st = CategoryStore::open(&root, CacheCategory::Resolve, 0);
        st.put("sym", b"v").unwrap();
        assert_eq!(st.remove("sym"), Some(1));
        assert!(!st.contains("sym"));
        assert!(st.get("nope").is_none());
        assert_eq!(st.misses(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_crash_recovery_discards_corrupt() {
        let root = test_root("crash");
        let cat = CacheCategory::Verify;
        {
            let mut st = CategoryStore::open(&root, cat, 0);
            st.put("k", b"data").unwrap();
            st.save().unwrap();
        }
        // Simulasikan crash saat commit: journal tersisa + index korup.
        let st = CategoryStore::open(&root, cat, 0);
        let names = vec![st.index_path(), st.manifest_path(), st.stats_path()];
        txn::write_journal(&st.journal_path(), &names).unwrap();
        std::fs::write(st.index_path(), vec![0xFF; 64]).unwrap();
        drop(st);

        // Open → recovery: index corrupt dibuang, manifest valid dipertahankan.
        let st2 = CategoryStore::open(&root, cat, 0);
        assert!(!st2.journal_path().exists(), "journal dihapus recovery");
        assert_eq!(st2.len(), 0, "index corrupt tidak di-load");
        assert!(!st2.rebuilt, "manifest valid → bukan rebuild");
        // Save berikutnya membangun ulang index + menyinkronkan manifest.
        let mut st3 = st2;
        st3.put("k", b"data").unwrap();
        st3.save().unwrap();
        assert_eq!(st3.manifest.entry_count, 1);
        let st4 = CategoryStore::open(&root, cat, 0);
        assert_eq!(st4.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_schema_mismatch_rebuilds_empty() {
        let root = test_root("schema");
        let cat = CacheCategory::Type;
        {
            let mut st = CategoryStore::open(&root, cat, 0);
            st.put("m", b"sig").unwrap();
            st.save().unwrap();
        }
        // Palsukan manifest dengan schema lama.
        {
            let path = root.join("manifest.mdb");
            let old = CacheManifest {
                schema_version: CACHE_SCHEMA_VERSION - 1,
                ..CacheManifest::fresh(0)
            };
            let mut w = MdbWriter::new();
            w.put(
                KEY_SINGLETON,
                KIND_MANIFEST,
                bincode::serialize(&old).unwrap(),
            );
            w.write_to(&path).unwrap();
        }
        let st = CategoryStore::open(&root, cat, 0);
        assert!(st.rebuilt, "schema mismatch → rebuilt");
        assert_eq!(st.len(), 0, "store lama dibangun ulang kosong");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_compiler_version_mismatch_rebuilds_empty() {
        // ROUND 36: cache yang ditulis binary LAMA (COMPILER_VERSION berbeda)
        // tidak boleh di-restore binary BARU — stale IR bug. Manifest dengan
        // compiler_version berbeda → rebuild kosong, walau schema_version sama.
        let root = test_root("cver");
        let cat = CacheCategory::Elaborate;
        {
            let mut st = CategoryStore::open(&root, cat, 0);
            st.put("ir:v1:top", b"stale-ir").unwrap();
            st.save().unwrap();
            assert_eq!(st.len(), 1);
        }
        // Palsukan manifest seolah ditulis versi compiler lama.
        {
            let path = root.join("manifest.mdb");
            let old = CacheManifest {
                compiler_version: "Mivon 0.0.0-p0-old".to_string(),
                ..CacheManifest::fresh(0)
            };
            let mut w = MdbWriter::new();
            w.put(
                KEY_SINGLETON,
                KIND_MANIFEST,
                bincode::serialize(&old).unwrap(),
            );
            w.write_to(&path).unwrap();
        }
        let st = CategoryStore::open(&root, cat, 0);
        assert!(st.rebuilt, "compiler_version mismatch → rebuilt");
        assert_eq!(st.len(), 0, "entry IR lama tidak boleh di-restore");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_gc_lru_and_ttl() {
        let root = test_root("gc");
        let mut st = CategoryStore::open(&root, CacheCategory::Parser, 0);
        st.budget_bytes = 30;
        // 3 entry @ 20B = 60B; budget 30B → 2 ter-evict, 1 tersisa.
        st.put("a", &[1u8; 20]).unwrap();
        st.put("b", &[2u8; 20]).unwrap();
        st.put("c", &[3u8; 20]).unwrap();
        let removed = st.gc();
        assert!(removed >= 1);
        assert!(st.bytes() <= 30);
        // TTL: entry dengan accessed_ns kuno dibuang.
        st.ttl_ns = 5_000_000_000;
        st.index.put(
            "old".into(),
            CacheIndexEntry {
                content_hash: 999,
                size: 5,
                large: false,
                kind: 1,
                created_ns: 0,
                accessed_ns: now_ns() - 10_000_000_000,
            },
        );
        assert_eq!(st.gc(), 1);
        assert!(!st.contains("old"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
