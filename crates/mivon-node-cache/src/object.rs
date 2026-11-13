//! Object store: artefak immutable, content-addressed (node_cache.md §2).
//!
//! `objects/ab/cd/<content-hash>` — isi identik berbagi satu file, lintas
//! project sekalipun. Object tidak mengetahui node mana yang memakainya.

use crate::filesystem::{atomic_write, walk_sharded, Layout, NodeLock};
use crate::identity::ContentId;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Store artefak content-addressed.
pub struct ObjectStore<'a> {
    layout: &'a Layout,
}

impl<'a> ObjectStore<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        ObjectStore { layout }
    }

    pub fn path(&self, id: &ContentId) -> PathBuf {
        self.layout.object_path(id)
    }

    pub fn exists(&self, id: &ContentId) -> bool {
        self.path(id).is_file()
    }

    pub fn size(&self, id: &ContentId) -> Option<u64> {
        std::fs::metadata(self.path(id)).map(|m| m.len()).ok()
    }

    /// Validasi keberadaan + ukuran (deteksi artefak terpotong).
    pub fn has_valid(&self, id: &ContentId, size: u64) -> bool {
        self.size(id).map(|s| s == size).unwrap_or(false)
    }

    /// Ambil isi artefak. `None` bila hilang/rusak.
    pub fn get(&self, id: &ContentId) -> Option<Vec<u8>> {
        let bytes = std::fs::read(self.path(id)).ok()?;
        // Content-addressed: verifikasi ulang isi (deteksi korupsi disk).
        (ContentId::of(&bytes) == *id).then_some(bytes)
    }

    /// Simpan artefak. Mengembalikan `(id, size, newly_written)`.
    ///
    /// Dua worker yang menulis isi sama berbagi satu file: lock per-content,
    /// lalu cek ulang setelah lock diperoleh.
    pub fn put(&self, bytes: &[u8]) -> io::Result<(ContentId, u64, bool)> {
        let id = ContentId::of(bytes);
        let size = bytes.len() as u64;
        if self.exists(&id) {
            return Ok((id, size, false));
        }
        let lock = NodeLock::acquire(
            self.layout,
            &format!("object-{}", id.to_hex()),
            Duration::from_millis(500),
        );
        if self.exists(&id) {
            drop(lock);
            return Ok((id, size, false));
        }
        atomic_write(&self.path(&id), bytes)?;
        drop(lock);
        Ok((id, size, true))
    }

    /// Simpan isi file sebagai artefak.
    pub fn put_file(&self, path: &Path) -> io::Result<(ContentId, u64, bool)> {
        let bytes = std::fs::read(path)?;
        self.put(&bytes)
    }

    /// Hapus artefak yang tidak dirujuk. Mengembalikan `(jumlah, bytes)`.
    pub fn sweep(&self, live: &HashSet<ContentId>) -> (usize, u64) {
        let mut removed = 0usize;
        let mut bytes = 0u64;
        let mut victims: Vec<(PathBuf, u64)> = Vec::new();
        walk_sharded(&self.layout.objects_dir(), |path, name| {
            let Some(id) = ContentId::from_hex(name) else {
                // Sisa `.tmp`/file sampah dari crash.
                if name.starts_with(".tmp-") {
                    let _ = std::fs::remove_file(path);
                }
                return;
            };
            if live.contains(&id) {
                return;
            }
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            victims.push((path.to_path_buf(), size));
        });
        for (path, size) in victims {
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
                bytes += size;
            }
        }
        (removed, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;

    fn temp_layout(tag: &str) -> (Layout, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mivon-object-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        (Layout::open(&root).unwrap(), root)
    }

    #[test]
    fn put_is_deduplicated_and_verified() {
        let (layout, root) = temp_layout("put");
        let store = ObjectStore::new(&layout);
        let (a, size, new_a) = store.put(b"payload").unwrap();
        let (b, _, new_b) = store.put(b"payload").unwrap();
        assert_eq!(a, b);
        assert_eq!(size, 7);
        assert!(new_a);
        assert!(!new_b);
        assert_eq!(store.get(&a).as_deref(), Some(&b"payload"[..]));

        // Korupsi isi → get menolak (content hash tidak cocok).
        std::fs::write(store.path(&a), b"tampered").unwrap();
        assert!(store.get(&a).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_keeps_live_objects_only() {
        let (layout, root) = temp_layout("sweep");
        let store = ObjectStore::new(&layout);
        let (live, _, _) = store.put(b"live").unwrap();
        let (_dead, _, _) = store.put(b"dead").unwrap();
        let mut keep = HashSet::new();
        keep.insert(live);
        let (removed, bytes) = store.sweep(&keep);
        assert_eq!(removed, 1);
        assert_eq!(bytes, 4);
        assert!(store.exists(&live));
        let _ = std::fs::remove_dir_all(&root);
    }
}
