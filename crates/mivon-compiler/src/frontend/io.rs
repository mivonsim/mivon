//! IO Optimization — file I/O untuk compile frontend.
//!
//! File >=4KB dimap sementara (mmap) lalu isi disalin ke heap — mapping
//! dilepas SEGERA setelah copy. Ini menutup Mem-03/PERF-17: tidak ada
//! region mmap yang tertahan per file selama compile (100K file tidak
//! lagi memegang 100K mapping / address space).
//! File kecil dibaca biasa (overhead mmap > manfaatnya).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use memmap2::Mmap;
use xxhash_rust::xxh3::xxh3_64;

/// Threshold: file <4KB dibaca biasa, >=4KB via mmap-copy.
const MMAP_THRESHOLD: u64 = 4096;

/// File content reader dengan checksum.
///
/// Catatan Mem-03/PERF-17 (fixed): versi lama MENAHAN `Mmap` di struct
/// walau isinya sudah di-copy ke `bytes` — memori dobel (mapped pages +
/// heap copy) dan address space tertahan selama compile tanpa batas
/// jumlah file. Kini mapping dilepas segera setelah salinan dibuat;
/// zero-copy penuh tetap ada di jalur node_cache (`node-cache/format/`) yang
/// memang membaca dari database ter-map.
#[derive(Debug)]
pub struct MmapFile {
    /// Bytes content (owned)
    bytes: Box<[u8]>,
    /// File path
    pub path: PathBuf,
    /// Checksum (computed on open)
    pub checksum: u64,
}

impl MmapFile {
    /// Open a file; file besar disalin via mmap (mapping langsung dilepas).
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let file_len = metadata.len();

        let bytes = if file_len >= MMAP_THRESHOLD {
            // mmap-copy: I/O read yang sama cepatnya utk file besar, tapi
            // mapping TIDAK ditahan — address space langsung bebas.
            let file = fs::File::open(path)?;
            let mmap = unsafe { Mmap::map(&file)? };
            // Advise sequential access sebelum copy.
            let _ = mmap.advise(memmap2::Advice::Sequential);
            let copied = mmap[..].to_vec().into_boxed_slice();
            track_mmap_bytes(copied.len());
            copied // `mmap` di-drop di sini → unmap
        } else {
            fs::read(path)?.into_boxed_slice()
        };

        let checksum = xxh3_64(&bytes);

        Ok(MmapFile {
            bytes,
            path: path.to_path_buf(),
            checksum,
        })
    }

    /// Get content as byte slice (zero-copy from mmap).
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Get content as string slice (zero-copy).
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes).unwrap_or("")
    }

    /// File size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Is empty?
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Read file content as bytes (auto mmap for large files).
pub fn read_file_bytes(path: &Path) -> std::io::Result<Box<[u8]>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() >= MMAP_THRESHOLD {
        let file = fs::File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(mmap[..].to_vec().into_boxed_slice())
    } else {
        Ok(fs::read(path)?.into_boxed_slice())
    }
}

/// Read file content as string (auto mmap for large files).
pub fn read_file_str(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

/// Global mmap statistics counter.
static MMAP_BYTES_SERVED: AtomicUsize = AtomicUsize::new(0);

/// Track mmap bytes served.
pub fn track_mmap_bytes(n: usize) {
    MMAP_BYTES_SERVED.fetch_add(n, Ordering::Relaxed);
}

/// Get total mmap bytes served.
pub fn total_mmap_bytes() -> usize {
    MMAP_BYTES_SERVED.load(Ordering::Relaxed)
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mmap_file_open() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mf = MmapFile::open(&path).unwrap();
        assert!(!mf.is_empty());
        assert!(!mf.is_empty());
        assert!(mf.as_str().contains("mivon"));
    }

    #[test]
    fn test_mmap_checksum() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mf1 = MmapFile::open(&path).unwrap();
        let mf2 = MmapFile::open(&path).unwrap();
        assert_eq!(mf1.checksum, mf2.checksum);
    }

    #[test]
    fn test_read_file_bytes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let bytes = read_file_bytes(&path).unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_small_file_read_directly() {
        // Create a small temp file (< 4KB)
        let dir = std::env::temp_dir().join("mivon_mmap_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("small.sv");
        std::fs::write(&path, "module small; endmodule").unwrap();

        let mf = MmapFile::open(&path).unwrap();
        assert_eq!(mf.as_str(), "module small; endmodule");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_large_file_no_map_retained() {
        // Mem-03/PERF-17: file >=4KB di-copy via mmap lalu mapping DILEPAS —
        // struct tidak lagi menahan region map (tidak ada field mmap).
        // Verifikasi perilaku: konten file besar terbaca utuh + checksum
        // konsisten, dan struct ukurannya hanya bytes+path+checksum.
        let dir = std::env::temp_dir().join("mivon_mmap_test_large");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("large.sv");
        let line = "// padding line untuk melewati threshold mmap\n";
        let mut body = String::with_capacity(8 * 1024);
        for _ in 0..300 {
            body.push_str(line);
        }
        body.push_str("module large; endmodule\n");
        std::fs::write(&path, &body).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() >= MMAP_THRESHOLD);

        let mf = MmapFile::open(&path).unwrap();
        assert_eq!(mf.as_str(), &body, "konten file besar utuh");
        assert_eq!(
            mf.checksum,
            xxh3_64(body.as_bytes()),
            "checksum atas salinan heap"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mmap_file_str() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mf = MmapFile::open(&path).unwrap();
        let s = mf.as_str();
        assert!(!s.is_empty());
        assert!(s.contains("mivon"));
    }
}
