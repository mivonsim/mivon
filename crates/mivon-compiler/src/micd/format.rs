//! MDB1 — format objek biner untuk MICD (Mivon Incremental Compilation Database).
//!
//! Bukan relational database, melainkan **object database**: setiap file `.mdb`
//! adalah kumpulan objek biner yang di-index oleh `key` u64 (hash path / hash
//! konten / singleton). Lookup O(1) via tabel objek yang dibangun sekali saat
//! mmap, lalu random access langsung ke region mmap (tanpa `read()` penuh).
//!
//! Layout file:
//!
//! ```text
//! +----------------+  64 byte fixed header
//! +----------------+
//! + Object Table   +  object_count x ObjectEntry (24 byte)
//! +----------------+
//! + Payload        +  blobs objek (offset relatif ke awal payload)
//! +----------------+
//! ```
//!
//! Header: magic `MDB1`, version, flags, checksum (xxh3 atas payload),
//! object_count, header_crc (xxh3 atas field header [0..28]).
//!
//! Kompresi (Kritik 15 db.md): seluruh blob dalam satu file `.mdb` dapat
//! dikompresi LZ4 (`frame`) sekaligus. Flag LZ4 disimpan di field header.
//! Checksum dihitung atas PAYLOAD TERSIMPAN (yang terkompresi), sehingga
//! pembaca memverifikasi integritas byte mentah — bukan hasil decompress.
//!
//! Penulisan atomik (temp + rename) agar pembaca paralel (IDE/LSP/compiler)
//! selalu melihat file yang utuh.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use lz4_flex::frame::FrameDecoder;
use lz4_flex::frame::FrameEncoder;
use memmap2::Mmap;

use crate::cache::checksum::compute_checksum;

/// Magic bytes identitas format MDB1.
pub const MAGIC: [u8; 4] = *b"MDB1";
/// Versi format saat ini.
pub const VERSION: u32 = 2;
// VERSION 1 → 2: `FileDiags` kini menyimpan `DiagLevel` (mivon-diag pusat),
// bukan enum lama `DiagSeverity`. Urutan/arity enum berubah → payload lama
// TIDAK kompatibel; bump memaksa rebuild penuh (cache di-restore ulang).

/// Flag header: blob terkompresi LZ4 (frame).
pub const FLAG_LZ4: u32 = 0x0000_0001;

/// Ukuran fixed header (byte).
pub const HEADER_SIZE: usize = 64;
/// Ukuran satu entry tabel objek (byte).
pub const ENTRY_SIZE: usize = 24;

/// Mode kompresi satu store `.mdb` (Kritik 15 db.md).
///
/// Per store (bukan per objek): AST/graph besar dikompresi, blob kecil
/// (metadata) tidak — mengurangi overhead tanpa mengorbankan kecepatan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// Tanpa kompresi (metadata, blob kecil).
    #[default]
    None,
    /// LZ4 frame — AST, graph, preprocessed source (kompresi cepat).
    Lz4,
}

impl Compression {
    fn flag(self) -> u32 {
        match self {
            Compression::None => 0,
            Compression::Lz4 => FLAG_LZ4,
        }
    }
}

/// Kode kind objek (metadata penyimpanan).
pub const KIND_STRING: u8 = 1;
pub const KIND_META: u8 = 2;
pub const KIND_AST: u8 = 3;
pub const KIND_GRAPH: u8 = 4;
pub const KIND_VERIFY: u8 = 5;
pub const KIND_DIAG: u8 = 6;
pub const KIND_SYMBOL: u8 = 7;
pub const KIND_TYPE: u8 = 8;
pub const KIND_PREPROC: u8 = 9;
pub const KIND_MANIFEST: u8 = 10;
pub const KIND_SNAPSHOT: u8 = 11;
pub const KIND_STATS: u8 = 12;

/// Key singleton untuk manifest (daftar semua key/path yang tersimpan).
pub const KEY_MANIFEST: u64 = 0x0000_0000_0000_0001;

/// Satu entry di tabel objek.
#[derive(Debug, Clone, Copy)]
pub struct ObjectEntry {
    pub key: u64,
    pub offset: u32,
    pub len: u32,
    pub kind: u8,
}

impl ObjectEntry {
    fn from_bytes(b: &[u8]) -> ObjectEntry {
        ObjectEntry {
            key: u64::from_le_bytes(b[0..8].try_into().unwrap()),
            offset: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            len: u32::from_le_bytes(b[12..16].try_into().unwrap()),
            kind: b[16],
        }
    }
    fn to_bytes(self) -> [u8; ENTRY_SIZE] {
        let mut out = [0u8; ENTRY_SIZE];
        out[0..8].copy_from_slice(&self.key.to_le_bytes());
        out[8..12].copy_from_slice(&self.offset.to_le_bytes());
        out[12..16].copy_from_slice(&self.len.to_le_bytes());
        out[16] = self.kind;
        out
    }
}

/// Error saat membuka/membaca file `.mdb`.
#[derive(Debug)]
pub enum MdbError {
    Io(std::io::Error),
    BadMagic,
    BadVersion(u32),
    BadHeaderCrc,
    BadChecksum,
    Truncated,
    Format(String),
}

impl std::fmt::Display for MdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MdbError::Io(e) => write!(f, "mdb I/O: {}", e),
            MdbError::BadMagic => write!(f, "mdb: bad magic (bukan file MDB1)"),
            MdbError::BadVersion(v) => write!(f, "mdb: version tak dikenal {}", v),
            MdbError::BadHeaderCrc => write!(f, "mdb: header corrupt (crc mismatch)"),
            MdbError::BadChecksum => write!(f, "mdb: payload corrupt (checksum mismatch)"),
            MdbError::Truncated => write!(f, "mdb: file terpotong"),
            MdbError::Format(s) => write!(f, "mdb: format error: {}", s),
        }
    }
}

impl std::error::Error for MdbError {}

impl From<std::io::Error> for MdbError {
    fn from(e: std::io::Error) -> Self {
        MdbError::Io(e)
    }
}

/// Writer untuk satu file `.mdb`. Objek dikumpulkan lalu ditulis atomik.
#[derive(Default)]
pub struct MdbWriter {
    objects: Vec<ObjectEntry>,
    blobs: Vec<Vec<u8>>,
    keys: std::collections::HashSet<u64>,
    /// Mode kompresi seluruh blob store ini (Kritik 15 db.md).
    compression: Compression,
}

impl MdbWriter {
    pub fn new() -> Self {
        MdbWriter::default()
    }

    /// Writer dengan mode kompresi tertentu (default: [`Compression::None`]).
    pub fn with_compression(compression: Compression) -> Self {
        MdbWriter {
            compression,
            ..MdbWriter::default()
        }
    }

    /// Tulis objek `data` dengan `key` dan `kind`.
    /// Duplicate key: objek terakhir menang (hapus entry lama).
    pub fn put(&mut self, key: u64, kind: u8, data: Vec<u8>) {
        if self.keys.contains(&key) {
            let pos = self.objects.iter().position(|o| o.key == key).unwrap();
            self.objects[pos].len = data.len() as u32;
            self.objects[pos].kind = kind;
            self.blobs[pos] = data;
            return;
        }
        self.keys.insert(key);
        self.objects.push(ObjectEntry {
            key,
            offset: 0, // dihitung saat serialize
            len: data.len() as u32,
            kind,
        });
        self.blobs.push(data);
    }

    /// Banyak objek.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Hitung checksum payload (xxh3) dari entry + blob.
    /// Penjumlahan (komutatif) sehingga urutan iterasi pembaca tak masalah.
    fn checksum_of(entries: &[ObjectEntry], blobs: &[Vec<u8>]) -> u64 {
        let mut h = 0u64;
        for e in entries {
            h = h.wrapping_add(compute_checksum(&e.to_bytes()));
        }
        for b in blobs {
            h = h.wrapping_add(compute_checksum(b));
        }
        h
    }

    /// Serialisasi ke bytes (header + object table + payload). Bila mode
    /// kompresi aktif, setiap blob dikompresi LZ4 sebelum masuk payload, dan
    /// checksum dihitung atas byte TERSIMPAN (terkompresi).
    ///
    /// Mengembalikan `Err` bila kompresi LZ4 gagal — TIDAK boleh diam-diam
    /// mengganti blob dengan data kosong (korupsi senyap). Pemanggil harus
    /// menangani error (batal tulis, bukan menulis data rusak).
    pub fn serialize(&self) -> Result<Vec<u8>, MdbError> {
        let mut stored: Vec<Vec<u8>> = Vec::with_capacity(self.blobs.len());
        for blob in &self.blobs {
            match self.compression {
                Compression::None => stored.push(blob.clone()),
                Compression::Lz4 => {
                    let mut enc = FrameEncoder::new(Vec::new());
                    enc.write_all(blob)
                        .map_err(|e| MdbError::Format(format!("LZ4 compress: {}", e)))?;
                    let compressed = enc
                        .finish()
                        .map_err(|e| MdbError::Format(format!("LZ4 finish: {}", e)))?;
                    stored.push(compressed);
                }
            }
        }

        let mut payload = Vec::new();
        let mut entries = Vec::with_capacity(self.objects.len());
        for (o, blob) in self.objects.iter().zip(stored.iter()) {
            let off = payload.len() as u32;
            payload.extend_from_slice(blob);
            entries.push(ObjectEntry {
                key: o.key,
                offset: off,
                len: blob.len() as u32,
                kind: o.kind,
            });
        }

        let mut out = Vec::with_capacity(HEADER_SIZE + entries.len() * ENTRY_SIZE + payload.len());
        let checksum = Self::checksum_of(&entries, &stored);
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&MAGIC);
        header[4..8].copy_from_slice(&VERSION.to_le_bytes());
        header[8..12].copy_from_slice(&self.compression.flag().to_le_bytes());
        header[12..20].copy_from_slice(&checksum.to_le_bytes());
        header[20..24].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        let header_crc = compute_checksum(&header[0..28]);
        header[28..36].copy_from_slice(&header_crc.to_le_bytes());
        out.extend_from_slice(&header);
        for e in &entries {
            out.extend_from_slice(&e.to_bytes());
        }
        out.extend_from_slice(&payload);
        Ok(out)
    }

    /// Tulis ke path secara atomik (temp + rename).
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        let data = self.serialize().map_err(std::io::Error::other)?;
        write_tmp(path, &data)?;
        commit_tmp(path)
    }
}

/// Path file temp untuk `path` (dipakai transaksi batch — Kritik 4 db.md).
pub fn tmp_path(path: &Path) -> std::path::PathBuf {
    path.with_extension("mdb.tmp")
}

/// Tulis data ke `path` + `.tmp` lalu sync. Belum di-commit (tidak menyentuh
/// file final). Gagal di tengah → file final tidak terpengaruh.
///
/// DIPAKAI untuk file state/transaksi `.mdb` (index/manifest/stats/journal &
/// store state) — durability penuh: setelah `commit_tmp` (rename) data sudah
/// benar-benar sampai media.
pub fn write_tmp(path: &Path, data: &[u8]) -> std::io::Result<()> {
    write_tmp_impl(path, data, true)
}

/// Tulis data ke `path` + `.tmp` TANPA fsync. Belum di-commit.
///
/// DIPAKAI HANYA untuk object content-addressed (CAS) yang immutable:
/// `objects/`, `blobs/`, `*.ast`, `*.preproc`, module precompiled. fsync per
/// object = ribuan syscall lambat (OpenTitan ~30-60k objek → hang puluhan
/// detik hingga menit). Durability dipertahankan berlapis:
/// 1. Tulis temp + rename tetap atomik — pembaca tidak pernah melihat file
///    setengah jadi;
/// 2. Index/MDB ditulis BELAKANGAN dengan fsync (`write_tmp`) — index baru
///    hanya visible setelah seluruh object dipublish;
/// 3. Object di-verifikasi saat load: checksum isi dicocokkan dengan hash di
///    nama file (dan format/version) — file partial akibat crash terdeteksi
///    → dianggap miss → dibangun ulang. Cache non-kritis: kehilangan entry
///    OK, menyajikan data korup TIDAK.
///
/// Dilarang memakai fungsi ini untuk file state `.mdb` (transaksi durability).
pub fn write_tmp_unflushed(path: &Path, data: &[u8]) -> std::io::Result<()> {
    write_tmp_impl(path, data, false)
}

fn write_tmp_impl(path: &Path, data: &[u8], sync: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        if sync {
            f.sync_all()?;
        }
    }
    Ok(())
}

/// Commit transaksi: rename `path.tmp` → `path` (atomik).
pub fn commit_tmp(path: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp_path(path), path)?;
    Ok(())
}

/// Reader untuk satu file `.mdb`. File di-mmap; objek diakses random access.
pub struct MdbReader {
    mmap: Mmap,
    index: HashMap<u64, (u32, u32, u8)>,
    /// Payload terkompresi LZ4 (flag dari header).
    compressed: bool,
}

impl std::fmt::Debug for MdbReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MdbReader")
            .field("objects", &self.index.len())
            .field("mmap_bytes", &self.mmap.len())
            .finish()
    }
}

impl MdbReader {
    /// Buka file `.mdb`. File yang corrupt → `Err` (pemanggil bisa fallback
    /// ke database kosong — MICD bersifat best-effort).
    pub fn open(path: &Path) -> Result<MdbReader, MdbError> {
        let file = std::fs::File::open(path)?;
        // SAFETY: file hanya dibaca; tidak dimodifikasi selama hidup.
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_mmap(mmap)
    }

    fn from_mmap(mmap: Mmap) -> Result<MdbReader, MdbError> {
        if mmap.len() < HEADER_SIZE {
            return Err(MdbError::Truncated);
        }
        if mmap[0..4] != MAGIC {
            return Err(MdbError::BadMagic);
        }
        let version = u32::from_le_bytes(mmap[4..8].try_into().unwrap());
        if version != VERSION {
            return Err(MdbError::BadVersion(version));
        }
        let flags = u32::from_le_bytes(mmap[8..12].try_into().unwrap());
        let compressed = flags & FLAG_LZ4 != 0;
        let header_crc = u64::from_le_bytes(mmap[28..36].try_into().unwrap());
        let actual_crc = compute_checksum(&mmap[0..28]);
        if header_crc != actual_crc {
            return Err(MdbError::BadHeaderCrc);
        }
        let checksum = u64::from_le_bytes(mmap[12..20].try_into().unwrap());
        let count = u32::from_le_bytes(mmap[20..24].try_into().unwrap()) as usize;
        let table_bytes = count
            .checked_mul(ENTRY_SIZE)
            .ok_or(MdbError::Format("object table overflow".into()))?;
        let payload_start = HEADER_SIZE + table_bytes;
        if mmap.len() < payload_start {
            return Err(MdbError::Truncated);
        }
        let mut index = HashMap::with_capacity(count);
        for i in 0..count {
            let off = HEADER_SIZE + i * ENTRY_SIZE;
            let e = ObjectEntry::from_bytes(&mmap[off..off + ENTRY_SIZE]);
            let end = e.offset as usize + e.len as usize;
            if end > mmap.len() - payload_start {
                return Err(MdbError::Truncated);
            }
            index.insert(e.key, (e.offset, e.len, e.kind));
        }
        // Verifikasi checksum payload (hitung ulang dari blobs).
        let mut h = 0u64;
        for (k, (o, l, kind)) in &index {
            let start = payload_start + *o as usize;
            h = h.wrapping_add(compute_checksum(&mmap[start..start + *l as usize]));
            let mut entry = [0u8; ENTRY_SIZE];
            entry[0..8].copy_from_slice(&k.to_le_bytes());
            entry[8..12].copy_from_slice(&o.to_le_bytes());
            entry[12..16].copy_from_slice(&l.to_le_bytes());
            entry[16] = *kind;
            h = h.wrapping_add(compute_checksum(&entry));
        }
        if h != checksum {
            return Err(MdbError::BadChecksum);
        }
        Ok(MdbReader {
            mmap,
            index,
            compressed,
        })
    }

    fn payload_start(&self) -> usize {
        HEADER_SIZE + self.index.len() * ENTRY_SIZE
    }

    /// Ambil objek mentah (copy dari mmap, di-decompress bila perlu).
    /// `None` jika key tak ada.
    pub fn get(&self, key: u64) -> Option<Vec<u8>> {
        let (o, l, _) = self.index.get(&key)?;
        let start = self.payload_start() + *o as usize;
        let raw = &self.mmap[start..start + *l as usize];
        if self.compressed {
            let mut dec = FrameDecoder::new(std::io::Cursor::new(raw));
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut dec, &mut buf).ok()?;
            Some(buf)
        } else {
            Some(raw.to_vec())
        }
    }

    /// Ambil slice objek langsung dari mmap (tanpa copy).
    /// Hanya valid untuk store TANPA kompresi (store terkompresi → `None`,
    /// karena hasil decompress tidak tinggal di region mmap).
    pub fn get_slice(&self, key: u64) -> Option<&[u8]> {
        if self.compressed {
            return None;
        }
        let (o, l, _) = self.index.get(&key)?;
        let start = self.payload_start() + *o as usize;
        Some(&self.mmap[start..start + *l as usize])
    }

    /// Kind objek untuk sebuah key.
    pub fn kind(&self, key: u64) -> Option<u8> {
        self.index.get(&key).map(|(_, _, k)| *k)
    }

    /// Semua (key, kind) yang tersimpan.
    pub fn keys(&self) -> Vec<(u64, u8)> {
        self.index
            .iter()
            .map(|(k, (_, _, kind))| (*k, *kind))
            .collect()
    }

    /// Banyak objek.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mivon_mdb_test_{}_{}", std::process::id(), name));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{}.mdb", name))
    }

    #[test]
    fn test_roundtrip() {
        let path = test_path("roundtrip");
        let mut w = MdbWriter::new();
        w.put(100, KIND_STRING, b"hello".to_vec());
        w.put(200, KIND_META, vec![1, 2, 3, 4, 5]);
        w.put(KEY_MANIFEST, KIND_MANIFEST, b"['a','b']".to_vec());
        w.write_to(&path).unwrap();

        let r = MdbReader::open(&path).unwrap();
        assert_eq!(r.get(100).unwrap(), b"hello");
        assert_eq!(r.get(200).unwrap(), vec![1, 2, 3, 4, 5]);
        assert_eq!(r.get(KEY_MANIFEST).unwrap(), b"['a','b']");
        assert_eq!(r.kind(200), Some(KIND_META));
        assert_eq!(r.get_slice(100).unwrap(), b"hello");
        assert!(r.get(99).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_overwrite_key() {
        let path = test_path("overwrite");
        let mut w = MdbWriter::new();
        w.put(7, KIND_STRING, b"old".to_vec());
        w.put(7, KIND_STRING, b"new".to_vec());
        assert_eq!(w.len(), 1);
        w.write_to(&path).unwrap();
        let r = MdbReader::open(&path).unwrap();
        assert_eq!(r.get(7).unwrap(), b"new");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_empty_store() {
        let path = test_path("empty");
        let w = MdbWriter::new();
        w.write_to(&path).unwrap();
        let r = MdbReader::open(&path).unwrap();
        assert!(r.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_bad_magic() {
        let path = test_path("badmagic");
        // File > HEADER_SIZE dengan magic salah → BadMagic (bukan Truncated).
        let junk = vec![b'X'; HEADER_SIZE + 32];
        std::fs::write(&path, &junk).unwrap();
        assert!(matches!(
            MdbReader::open(&path).unwrap_err(),
            MdbError::BadMagic
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_corrupt_payload_detected() {
        let path = test_path("corrupt");
        let mut w = MdbWriter::new();
        w.put(5, KIND_STRING, b"valid data".to_vec());
        w.write_to(&path).unwrap();
        // Rusak payload: flip byte di objek pertama.
        let data = std::fs::read(&path).unwrap();
        let table = ENTRY_SIZE;
        let corrupt = {
            let mut d = data.clone();
            let off = HEADER_SIZE + table;
            d[off + 1] ^= 0xFF;
            d
        };
        std::fs::write(&path, &corrupt).unwrap();
        let err = MdbReader::open(&path);
        assert!(err.is_err(), "payload corrupt harus terdeteksi");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_many_objects() {
        let path = test_path("many");
        let mut w = MdbWriter::new();
        for i in 0..1000u64 {
            w.put(i + 100, KIND_STRING, format!("obj-{}", i).into_bytes());
        }
        w.write_to(&path).unwrap();
        let r = MdbReader::open(&path).unwrap();
        assert_eq!(r.len(), 1000);
        assert_eq!(r.get(500).unwrap(), b"obj-400");
        let _ = std::fs::remove_file(&path);
    }

    // ── Kritik 15 db.md: Compression (LZ4) ──

    #[test]
    fn test_lz4_compression_roundtrip() {
        let path = test_path("lz4");
        // Data berulang → sangat compressible.
        let big: Vec<u8> = format!(
            "module x;\n{}\nendmodule",
            "  logic [7:0] a = 8'hAA;\n".repeat(500)
        )
        .into_bytes();
        let mut w = MdbWriter::with_compression(Compression::Lz4);
        w.put(1, KIND_AST, big.clone());
        let data = w.serialize().unwrap();
        assert!(
            data.len() < big.len(),
            "LZ4 harus mengecilkan blob ({})",
            data.len()
        );
        w.write_to(&path).unwrap();

        let r = MdbReader::open(&path).unwrap();
        assert!(r.compressed, "flag LZ4 terbaca");
        assert_eq!(r.get(1).unwrap(), big, "decompress = data asli");
        assert!(
            r.get_slice(1).is_none(),
            "get_slice tidak valid utk store terkompresi"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_compression_modes_write_to_disk() {
        let path = test_path("modes");
        let mut w = MdbWriter::with_compression(Compression::Lz4);
        w.put(1, KIND_AST, vec![1, 2, 3, 4, 5]);
        w.write_to(&path).unwrap();
        let r = MdbReader::open(&path).unwrap();
        assert_eq!(r.get(1).unwrap(), vec![1, 2, 3, 4, 5]);
        let _ = std::fs::remove_file(&path);
    }
}
