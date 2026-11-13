//! Abstraksi filesystem node_cache: layout, sharding, atomic write, lock.
//!
//! Tidak ada database dan tidak ada index tersembunyi (node_cache.md §2).
//! Semua state adalah file biasa:
//!
//! ```text
//! node_cache/
//! ├── objects/ab/cd/<content-hash>   artefak immutable (content-addressed)
//! ├── nodes/01/02/<node-id>          manifest node
//! ├── graphs/<graph-id>             snapshot graph immutable
//! ├── refs/<project-id>             graph terakhir project
//! ├── transactions/<txn-id>         journal commit
//! ├── tmp/                          artefak sementara
//! ├── locks/<node-id>.lock          lock per-node
//! └── FORMAT                        versi format + algoritma hash
//! ```

use crate::identity::{ContentId, NodeIdentity};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Versi format penyimpanan (naikkan bila layout/format berubah).
pub const STORAGE_VERSION: u32 = 1;

/// Nama file versi format.
pub const FILE_FORMAT: &str = "FORMAT";

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Layout direktori node_cache.
#[derive(Clone, Debug)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Layout { root: root.into() }
    }

    /// Root default: env `MIVON_NODE_CACHE_DIR`, lalu `<root>/node_cache`.
    /// Cache berada langsung di root project (bukan di dalam `.mivon/`).
    pub fn default_root_for(root: &Path) -> PathBuf {
        if let Ok(p) = std::env::var("MIVON_NODE_CACHE_DIR") {
            return PathBuf::from(p);
        }
        root.join("node_cache")
    }

    pub fn default_root_for_cwd() -> PathBuf {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::default_root_for(&cwd)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    pub fn nodes_dir(&self) -> PathBuf {
        self.root.join("nodes")
    }

    pub fn graphs_dir(&self) -> PathBuf {
        self.root.join("graphs")
    }

    pub fn refs_dir(&self) -> PathBuf {
        self.root.join("refs")
    }

    pub fn transactions_dir(&self) -> PathBuf {
        self.root.join("transactions")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }

    /// Buka (dan inisialisasi bila perlu) layout. Format yang tidak dikenal
    /// atau versi berbeda → error (jangan menebak isi cache lama).
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let layout = Layout::new(root);
        for dir in [
            layout.objects_dir(),
            layout.nodes_dir(),
            layout.graphs_dir(),
            layout.refs_dir(),
            layout.transactions_dir(),
            layout.tmp_dir(),
            layout.locks_dir(),
        ] {
            fs::create_dir_all(&dir)?;
        }
        layout.ensure_format()?;
        Ok(layout)
    }

    fn ensure_format(&self) -> io::Result<()> {
        let path = self.root.join(FILE_FORMAT);
        let expected = format!(
            "mivon-node-cache\nstorage_version={}\nhash={}\nnode_id=deterministic\n",
            STORAGE_VERSION,
            crate::identity::HASH_ALGO
        );
        match fs::read_to_string(&path) {
            Ok(found) => {
                if found.trim() != expected.trim() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "node_cache FORMAT tidak kompatibel di {} (ditemukan {:?}, harap hapus cache)",
                            path.display(),
                            found.lines().take(1).collect::<String>()
                        ),
                    ));
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => atomic_write(&path, expected.as_bytes())
                .map(|_| ())
                .map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("gagal menulis FORMAT node_cache: {}", e),
                    )
                }),
            Err(e) => Err(e),
        }
    }

    /// Path object content-addressed (sharding prefix hash).
    pub fn object_path(&self, id: &ContentId) -> PathBuf {
        shard_path(&self.objects_dir(), &id.to_hex())
    }

    /// Path manifest node (sharding prefix hash).
    pub fn node_path(&self, id: &NodeIdentity) -> PathBuf {
        shard_path(&self.nodes_dir(), &id.to_hex())
    }

    /// Path snapshot graph.
    pub fn graph_path(&self, id: &ContentId) -> PathBuf {
        self.graphs_dir().join(id.to_hex())
    }

    /// Path reference project.
    ///
    /// Nama project adalah hash/dirname aman; sanitasi tetap dilakukan agar
    /// tidak ada path traversal dari project id yang dibuat caller.
    pub fn ref_path(&self, project: &str) -> PathBuf {
        self.refs_dir().join(sanitize_name(project))
    }

    /// Path journal transaksi.
    pub fn transaction_path(&self, id: &str) -> PathBuf {
        self.transactions_dir().join(sanitize_name(id))
    }

    /// Path lock node.
    pub fn lock_path(&self, key: &str) -> PathBuf {
        self.locks_dir().join(format!("{}.lock", sanitize_name(key)))
    }
}

/// Sharding `<dir>/ab/cd/<hex>` agar satu direktori tidak berisi jutaan file.
pub fn shard_path(dir: &Path, hex: &str) -> PathBuf {
    if hex.len() < 4 {
        return dir.join(hex);
    }
    dir.join(&hex[..2]).join(&hex[2..4]).join(hex)
}

/// Sanitasi nama file: hanya alfanumerik, `-`, `_`, `.`; sisanya `_`.
/// Kosong → `_`.
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Tulis atomik: tmp di direktori tujuan → fsync file → rename → fsync dir.
///
/// Tmp SENGAJA dibuat di direktori tujuan (bukan `tmp/`) agar rename berada di
/// filesystem yang sama — rename lintas filesystem tidak atomik.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<usize> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path tanpa parent: {}", path.display()),
        )
    })?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".tmp-{}-{}", process::id(), unique_suffix()));
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    match fs::rename(&tmp, path) {
        Ok(()) => {}
        Err(e) => {
            // Windows: rename tidak menimpa file lama. Fallback terukur.
            let _ = fs::remove_file(path);
            fs::rename(&tmp, path).map_err(|_| e)?;
        }
    }
    sync_dir(dir);
    Ok(bytes.len())
}

/// fsync direktori (best-effort; tidak semua platform mendukung).
pub fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Suffix unik proses-lokal (nanos + counter) untuk nama tmp.
pub fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", nanos, n)
}

/// Lock per-node berbasis atomic `create_new`.
///
/// Dua worker yang menghitung node sama berbagi satu hasil: yang kalah lock
/// menunggu sebentar lalu memakai artefak yang sudah dipublikasikan.
pub struct NodeLock {
    path: PathBuf,
}

impl NodeLock {
    /// Coba ambil lock. `None` bila waktu tunggu habis (bukan error fatal).
    pub fn acquire(layout: &Layout, key: &str, timeout: Duration) -> Option<NodeLock> {
        let path = layout.lock_path(key);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let deadline = Instant::now() + timeout;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = writeln!(f, "{}", process::id());
                    return Some(NodeLock { path });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return None,
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for NodeLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Jalan rekursif dua level sharding (objects/nodes).
pub fn walk_sharded(dir: &Path, mut f: impl FnMut(&Path, &str)) {
    let Ok(level1) = fs::read_dir(dir) else {
        return;
    };
    for a in level1.flatten() {
        if !a.path().is_dir() {
            continue;
        }
        let Ok(level2) = fs::read_dir(a.path()) else {
            continue;
        };
        for b in level2.flatten() {
            if !b.path().is_dir() {
                continue;
            }
            let Ok(files) = fs::read_dir(b.path()) else {
                continue;
            };
            for entry in files.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                f(&entry.path(), &name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mivon-node-cache-{}-{}-{}",
            tag,
            process::id(),
            unique_suffix()
        ))
    }

    #[test]
    fn open_writes_format_and_dirs() {
        let root = temp_root("format");
        let layout = Layout::open(&root).unwrap();
        assert!(root.join(FILE_FORMAT).is_file());
        assert!(layout.objects_dir().is_dir());
        assert!(layout.nodes_dir().is_dir());
        assert!(layout.refs_dir().is_dir());
        assert!(layout.transactions_dir().is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sharding_uses_two_prefix_levels() {
        let id = ContentId::of(b"x");
        let layout = Layout::new("/tmp/nc");
        let p = layout.object_path(&id);
        let hex = id.to_hex();
        assert!(p.ends_with(Path::new(&hex[..2]).join(&hex[2..4]).join(&hex)));
    }

    #[test]
    fn atomic_write_replaces_content() {
        let root = temp_root("atomic");
        fs::create_dir_all(&root).unwrap();
        let p = root.join("f.bin");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sanitize_blocks_path_traversal() {
        assert_eq!(sanitize_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_name(""), "_");
    }

    #[test]
    fn incompatible_format_is_rejected() {
        let root = temp_root("badformat");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(FILE_FORMAT), "mivon-node-cache\nstorage_version=999\n").unwrap();
        assert!(Layout::open(&root).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
