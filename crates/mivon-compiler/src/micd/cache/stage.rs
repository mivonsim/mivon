//! stage — `StageStore<K, V>` generik di atas satu engine seragam.
//!
//! Fase 1 rombak MICD (Kritik A/B): 21 kategori `cache/` sebelumnya hanya
//! *konseptual* seragam (satu struct `CategoryStore` dipakai 21×) tapi API-nya
//! mentah (`&str` → `&[u8]`, tanpa tipe). `StageStore` adalah facade bertipe:
//! satu implementasi generik untuk semua stage, key sebagai byte-string
//! (bukan u64 hash sebagai identitas), payload lazy (di disk sampai `get`),
//! LRU/TTL via engine (`CategoryStore`).
//!
//! Sengaja **tanpa mengubah format disk**: `StageStore` membungkus
//! `CategoryStore` (komposisi), bukan rewrite. Byte di disk identik —
//! `put_typed`/`get_typed` hanya encode/decode di tepi. Migrasi Fase 2
//! (state/ → kategori) dan Fase 3 (invalidasi per-stage) memakai facade ini.
//!
//! ```text
//! Pipeline ──► StageStore<String, Vec<u8>> ──► CategoryStore ──► MDB1 + CAS
//!              StageStore<PathBuf, String> ──► (sama, beda codec)
//! ```

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use super::category::CacheCategory;
use super::store::CategoryStore;

/// Kunci stage sebagai byte-string.
///
/// Identitas kunci adalah string itu sendiri (disimpan eksplisit di payload
/// index `(String, entry)` + diverifikasi saat load), BUKAN u64 hash.
/// Hash hanya kunci internal MDB; tabrakan hash tidak bisa menukar dua kunci
/// berbeda tanpa terdeteksi (lihat test `collision_mismatch_rejected`).
pub trait StageKey: Clone {
    /// Encode ke kunci string (byte-string).
    fn encode_key(&self) -> String;
    /// Decode dari kunci string. `None` bila format tak cocok.
    fn decode_key(s: &str) -> Option<Self>;
}

/// Nilai stage: encode ke bytes CAS + decode kembali.
pub trait StageValue: Clone {
    fn encode_value(&self) -> Vec<u8>;
    fn decode_value(bytes: &[u8]) -> Option<Self>;
}

impl StageKey for String {
    fn encode_key(&self) -> String {
        self.clone()
    }
    fn decode_key(s: &str) -> Option<Self> {
        Some(s.to_owned())
    }
}

impl StageKey for PathBuf {
    // Lossless di Unix (bytes OsStr di-hex bila non-UTF8), lossy di
    // non-Unix (didokumentasikan). Prefix `__nonutf8__:` mencegah tabrakan
    // dengan path UTF-8 biasa.
    fn encode_key(&self) -> String {
        if let Some(s) = self.to_str() {
            return s.to_owned();
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let hex: String = self
                .as_os_str()
                .as_bytes()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            return format!("__nonutf8__:{hex}");
        }
        #[cfg(not(unix))]
        {
            self.to_string_lossy().into_owned()
        }
    }
    fn decode_key(s: &str) -> Option<Self> {
        if let Some(hex) = s.strip_prefix("__nonutf8__:") {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                if hex.len() % 2 != 0 {
                    return None;
                }
                let bytes: Option<Vec<u8>> = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect();
                let bytes = bytes?;
                return Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)));
            }
            #[cfg(not(unix))]
            {
                return Some(PathBuf::from(s));
            }
        }
        Some(PathBuf::from(s))
    }
}

impl StageKey for u64 {
    fn encode_key(&self) -> String {
        format!("{:016x}", self)
    }
    fn decode_key(s: &str) -> Option<Self> {
        u64::from_str_radix(s, 16).ok()
    }
}

impl StageValue for Vec<u8> {
    fn encode_value(&self) -> Vec<u8> {
        self.clone()
    }
    fn decode_value(bytes: &[u8]) -> Option<Self> {
        Some(bytes.to_vec())
    }
}

impl StageValue for String {
    fn encode_value(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
    fn decode_value(bytes: &[u8]) -> Option<Self> {
        String::from_utf8(bytes.to_vec()).ok()
    }
}

impl StageValue for u64 {
    fn encode_value(&self) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
    fn decode_value(bytes: &[u8]) -> Option<Self> {
        let b: [u8; 8] = bytes.try_into().ok()?;
        Some(u64::from_le_bytes(b))
    }
}

/// Versi skema PER STAGE (Fase 1: semua 1; Fase 3 menaikkan per stage tanpa
/// me-reset seluruh cache).
///
/// Dipakai calon invalidasi per-stage: artefak menyimpan versi stage yang
/// memproduksinya; stage yang formatnya berubah di-rebuild selektif.
pub fn stage_schema_version(_stage: CacheCategory) -> u64 {
    1
}

/// Facade bertipe di atas satu engine (`CategoryStore`).
///
/// `K`/`V` hanya codec tepi — di disk tetap `(String, bytes)` seperti
/// sebelumnya, jadi perilaku tidak berubah (Fase 1: tanpa ubah perilaku).
pub struct StageStore<K, V> {
    inner: CategoryStore,
    _k: PhantomData<K>,
    _v: PhantomData<V>,
}

impl<K: StageKey, V: StageValue> StageStore<K, V> {
    /// Buka store satu stage di `root` — `root` adalah DIREKTORI KATEGORI
    /// itu sendiri (`<db>/cache/<pid>/<stage>/`), sama konvensinya dengan
    /// [`CategoryStore::open`]. Untuk membuka dari root layer, pakai
    /// [`StageStore::open_at_layer`] agar 21 stage tidak bertabrakan.
    pub fn open(root: &Path, category: CacheCategory, config_hash: u64) -> Self {
        StageStore {
            inner: CategoryStore::open(root, category, config_hash),
            _k: PhantomData,
            _v: PhantomData,
        }
    }

    /// Buka stage `category` dari root layer (`<db>/cache/<pid>/`):
    /// join nama stage otomatis (konvensi [`super::CacheLayer::open`]).
    pub fn open_at_layer(
        layer_root: &Path,
        category: CacheCategory,
        config_hash: u64,
    ) -> Self {
        Self::open(&layer_root.join(category.name()), category, config_hash)
    }

    /// Bungkus store yang sudah dibuka (untuk `CacheLayer`).
    pub fn wrap(inner: CategoryStore) -> Self {
        StageStore {
            inner,
            _k: PhantomData,
            _v: PhantomData,
        }
    }

    pub fn stage(&self) -> CacheCategory {
        self.inner.category
    }

    pub fn schema_version(&self) -> u64 {
        stage_schema_version(self.inner.category)
    }

    /// Simpan nilai bertipe. `None` bila payload kosong (engine menolak
    /// bytes kosong — perilaku warisan, bukan error).
    pub fn put_typed(&mut self, key: &K, value: &V) -> Option<u64> {
        let k = key.encode_key();
        let bytes = value.encode_value();
        self.inner.put(&k, &bytes)
    }

    /// Ambil nilai bertipe. Miss (tak ada / objek hilang / corrupt / decode
    /// gagal) → `None`. Payload dibaca lazy dari disk saat dipanggil.
    pub fn get_typed(&mut self, key: &K) -> Option<V> {
        let k = key.encode_key();
        let bytes = self.inner.get(&k)?;
        V::decode_value(&bytes)
    }

    pub fn contains_typed(&self, key: &K) -> bool {
        self.inner.contains(&key.encode_key())
    }

    pub fn remove_typed(&mut self, key: &K) -> bool {
        self.inner.remove(&key.encode_key()).is_some()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn bytes(&self) -> u64 {
        self.inner.bytes()
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.dirty
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.inner.save()
    }

    /// GC LRU/TTL + sweep yatim. Mengembalikan entry dibuang.
    pub fn gc(&mut self) -> usize {
        self.inner.gc()
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.inner.clear()
    }

    /// Akses engine mentah (transisi bertahap Fase 1→2).
    pub fn inner(&self) -> &CategoryStore {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut CategoryStore {
        &mut self.inner
    }

    /// Ambil alih engine (untuk dikembalikan ke `CacheLayer`).
    pub fn into_inner(self) -> CategoryStore {
        self.inner
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::micd::cache::index::{CacheIndex, CacheIndexEntry};
    use crate::micd::format::{MdbWriter, KIND_STRING};

    fn test_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mivon_stage_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_typed_roundtrip_string_vec() {
        let root = test_root("roundtrip");
        {
            let mut st: StageStore<String, Vec<u8>> =
                StageStore::open(&root, CacheCategory::Parser, 0);
            assert_eq!(st.schema_version(), 1);
            st.put_typed(&"a.sv".to_string(), &b"ast-bytes".to_vec());
            assert_eq!(st.get_typed(&"a.sv".to_string()).unwrap(), b"ast-bytes");
            assert!(st.contains_typed(&"a.sv".to_string()));
            st.save().unwrap();
        }
        {
            let mut st: StageStore<String, Vec<u8>> =
                StageStore::open(&root, CacheCategory::Parser, 0);
            assert_eq!(st.get_typed(&"a.sv".to_string()).unwrap(), b"ast-bytes");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_typed_pathbuf_string_keys() {
        let root = test_root("pathkeys");
        {
            let mut st: StageStore<PathBuf, String> =
                StageStore::open(&root, CacheCategory::Preprocess, 0);
            let k = PathBuf::from("rtl/uart.sv");
            st.put_typed(&k, &"module uart; endmodule".to_string());
            assert_eq!(st.get_typed(&k).unwrap(), "module uart; endmodule");
            st.save().unwrap();
        }
        {
            let mut st: StageStore<PathBuf, String> =
                StageStore::open(&root, CacheCategory::Preprocess, 0);
            let k = PathBuf::from("rtl/uart.sv");
            assert_eq!(st.get_typed(&k).unwrap(), "module uart; endmodule");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_typed_u64_value() {
        let root = test_root("u64val");
        let mut st: StageStore<String, u64> =
            StageStore::open(&root, CacheCategory::Type, 0);
        st.put_typed(&"sig".to_string(), &0xDEAD_BEEF_CAFE_1234);
        assert_eq!(st.get_typed(&"sig".to_string()).unwrap(), 0xDEAD_BEEF_CAFE_1234);
        // Bytes corrupt (panjang salah) → decode gagal → None, bukan panic.
        st.inner_mut().put("bad", &[1, 2, 3]);
        let mut st2: StageStore<String, u64> = StageStore::wrap(st.into_inner());
        assert!(st2.get_typed(&"bad".to_string()).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_unicode_dan_kunci_panjang() {
        // Byte-string keys: unicode + 4KB key harus roundtrip utuh.
        let root = test_root("unicode");
        let mut st: StageStore<String, Vec<u8>> =
            StageStore::open(&root, CacheCategory::Resolve, 0);
        let keys = vec![
            "modul_αβγ.sv".to_string(),
            "x".repeat(4096),
            "spasi dan/simbol-!@#$.sv".to_string(),
        ];
        for (i, k) in keys.iter().enumerate() {
            st.put_typed(k, &vec![i as u8; 8]);
        }
        st.save().unwrap();
        drop(st);
        let mut st2: StageStore<String, Vec<u8>> =
            StageStore::open(&root, CacheCategory::Resolve, 0);
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(st2.get_typed(k).unwrap(), vec![i as u8; 8], "key {:?}", &k[..k.len().min(32)]);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_lazy_payload_tidak_dibaca_saat_open() {
        // Lazy: open hanya muat index; payload dibaca saat get.
        let root = test_root("lazy");
        {
            let mut st: StageStore<String, Vec<u8>> =
                StageStore::open(&root, CacheCategory::Elaborate, 0);
            st.put_typed(&"ir:top".to_string(), &vec![0xABu8; 1024]);
            st.save().unwrap();
        }
        // Hapus objek payload di disk SEBELUM open berikutnya.
        let objs: Vec<PathBuf> = std::fs::read_dir(root.join("objects"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(!objs.is_empty());
        for o in &objs {
            std::fs::remove_file(o).unwrap();
        }
        // Open tetap sukses (index ada), get → miss (bukan crash).
        let mut st2: StageStore<String, Vec<u8>> =
            StageStore::open(&root, CacheCategory::Elaborate, 0);
        assert!(st2.contains_typed(&"ir:top".to_string()), "index tetap ada");
        assert!(st2.get_typed(&"ir:top".to_string()).is_none(), "objek hilang → miss");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_collision_mismatch_rejected() {
        // Kritik E: kunci identitas = byte-string. Entry index yang hash
        // MDB-nya tidak cocok dengan hash kunci → ditolak saat load.
        let dir = test_root("collide");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.mdb");
        let entry = CacheIndexEntry {
            content_hash: 1,
            size: 5,
            large: false,
            kind: 1,
            created_ns: 0,
            accessed_ns: 0,
        };
        let mut w = MdbWriter::new();
        w.put(
            0x1234_5678,
            KIND_STRING,
            bincode::serialize(&("kunci-asli".to_string(), entry)).unwrap(),
        );
        w.write_to(&path).unwrap();
        // Premis: kunci salah → hash beda.
        assert_ne!(CacheIndex::key_hash("kunci-asli"), 0x1234_5678);
        let idx = CacheIndex::load(&path);
        assert!(idx.is_empty(), "hash mismatch harus ditolak");
        // Sanity: kunci benar diterima.
        let mut w2 = MdbWriter::new();
        let entry2 = CacheIndexEntry {
            content_hash: 2,
            size: 5,
            large: false,
            kind: 1,
            created_ns: 0,
            accessed_ns: 0,
        };
        w2.put(
            CacheIndex::key_hash("kunci-asli"),
            KIND_STRING,
            bincode::serialize(&("kunci-asli".to_string(), entry2)).unwrap(),
        );
        w2.write_to(&path).unwrap();
        let idx2 = CacheIndex::load(&path);
        assert_eq!(idx2.len(), 1);
        assert!(idx2.contains("kunci-asli"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_schema_version_seragam_saat_ini() {
        // Fase 1: semua stage versi 1 (fondasi Fase 3).
        for cat in CacheCategory::ALL {
            assert_eq!(stage_schema_version(cat), 1, "{:?}", cat);
        }
    }

    #[test]
    fn test_empty_payload_ditolak_engine() {
        // Perilaku warisan didokumentasikan: bytes kosong → None (miss).
        let root = test_root("empty");
        let mut st: StageStore<String, Vec<u8>> =
            StageStore::open(&root, CacheCategory::Parser, 0);
        assert!(st.put_typed(&"e".to_string(), &Vec::new()).is_none());
        assert!(st.get_typed(&"e".to_string()).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_u64_key_roundtrip_dan_persist() {
        let root = test_root("u64key");
        {
            let mut st: StageStore<u64, String> =
                StageStore::open(&root, CacheCategory::Verify, 0);
            st.put_typed(&0x1234_5678_9ABC_DEF0, &"hasil".to_string());
            assert_eq!(st.get_typed(&0x1234_5678_9ABC_DEF0).unwrap(), "hasil");
            assert!(st.contains_typed(&0x1234_5678_9ABC_DEF0));
            assert!(!st.contains_typed(&0x0));
            st.save().unwrap();
        }
        {
            let mut st: StageStore<u64, String> =
                StageStore::open(&root, CacheCategory::Verify, 0);
            assert_eq!(st.get_typed(&0x1234_5678_9ABC_DEF0).unwrap(), "hasil");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_u64_value_persist_lewat_reopen() {
        let root = test_root("u64persist");
        {
            let mut st: StageStore<String, u64> =
                StageStore::open(&root, CacheCategory::Type, 0);
            st.put_typed(&"sig".to_string(), &0xCAFE_F00D_DEAD_BEEF);
            st.save().unwrap();
        }
        {
            let mut st: StageStore<String, u64> =
                StageStore::open(&root, CacheCategory::Type, 0);
            assert_eq!(st.get_typed(&"sig".to_string()).unwrap(), 0xCAFE_F00D_DEAD_BEEF);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_salah_tipe_baca_gagal_aman() {
        // Tulis sebagai String, baca sebagai u64 → None (bukan panic/data sampah).
        let root = test_root("mistype");
        let mut st: StageStore<String, String> =
            StageStore::open(&root, CacheCategory::Lint, 0);
        st.put_typed(&"k".to_string(), &"bukan-angka".to_string());
        let raw = st.inner_mut().get("k");
        drop(st);
        assert!(raw.is_some());
        assert!(u64::decode_value(&raw.unwrap()).is_none());
        // Via facade beda V: get_typed<u64> atas bytes String pendek → None.
        let mut st2: StageStore<String, u64> =
            StageStore::open(&root, CacheCategory::Lint, 0);
        // "bukan-angka" = 11 bytes ≠ 8 → decode gagal.
        assert!(st2.get_typed(&"k".to_string()).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_remove_typed() {
        let root = test_root("remove_t");
        let mut st: StageStore<String, Vec<u8>> =
            StageStore::open(&root, CacheCategory::Macro, 0);
        st.put_typed(&"d".to_string(), &b"v".to_vec());
        assert!(st.contains_typed(&"d".to_string()));
        assert!(st.remove_typed(&"d".to_string()));
        assert!(!st.contains_typed(&"d".to_string()));
        assert!(!st.remove_typed(&"d".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn test_pathbuf_non_utf8_lossless() {
        use std::os::unix::ffi::OsStrExt;
        let root = test_root("nonutf8");
        let raw = vec![0x66, 0x6F, 0x6F, 0xFF, 0xFE, 0x62, 0x61, 0x72];
        let key = PathBuf::from(std::ffi::OsStr::from_bytes(&raw));
        assert!(key.to_str().is_none(), "premis: benar non-UTF8");
        {
            let mut st: StageStore<PathBuf, String> =
                StageStore::open(&root, CacheCategory::Include, 0);
            st.put_typed(&key, &"v".to_string());
            assert_eq!(st.get_typed(&key).unwrap(), "v");
            // Kunci UTF-8 biasa tidak tertukar dengan encoding non-UTF8.
            assert!(!st.contains_typed(&PathBuf::from("foo")));
            st.save().unwrap();
        }
        {
            let mut st: StageStore<PathBuf, String> =
                StageStore::open(&root, CacheCategory::Include, 0);
            assert_eq!(st.get_typed(&key).unwrap(), "v");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_open_at_layer_join_nama_stage() {
        // S2: open_at_layer join otomatis — dua stage tidak bertabrakan.
        let layer_root = test_root("atlayer");
        std::fs::create_dir_all(&layer_root).unwrap();
        {
            let mut a: StageStore<String, Vec<u8>> = StageStore::open_at_layer(
                &layer_root,
                CacheCategory::Parser,
                0,
            );
            let mut b: StageStore<String, Vec<u8>> = StageStore::open_at_layer(
                &layer_root,
                CacheCategory::Lexer,
                0,
            );
            a.put_typed(&"k".to_string(), &b"parser".to_vec());
            b.put_typed(&"k".to_string(), &b"lexer".to_vec());
            a.save().unwrap();
            b.save().unwrap();
        }
        assert!(layer_root.join("parser").is_dir());
        assert!(layer_root.join("lexer").is_dir());
        {
            let mut a: StageStore<String, Vec<u8>> = StageStore::open_at_layer(
                &layer_root,
                CacheCategory::Parser,
                0,
            );
            let mut b: StageStore<String, Vec<u8>> = StageStore::open_at_layer(
                &layer_root,
                CacheCategory::Lexer,
                0,
            );
            assert_eq!(a.get_typed(&"k".to_string()).unwrap(), b"parser");
            assert_eq!(b.get_typed(&"k".to_string()).unwrap(), b"lexer");
        }
        let _ = std::fs::remove_dir_all(&layer_root);
    }
}
