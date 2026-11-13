//! Helpers buka/identifikasi node_cache — membungkus `ProjectCache` root & id.

use mivon_compiler::node_cache::ProjectCache;
use std::path::{Path, PathBuf};

/// Root default cache (`node_cache/` di root project, override
/// `MIVON_NODE_CACHE_DIR`) berdasarkan CWD. F37 fix:
/// `ProjectCache::default_root()` (CWD-relative) diganti `database_root_for`
/// (root eksplisit) di jalur produksi — fungsi ini didelegasikan agar tidak ada
/// dua sumber kebenaran yang divergen.
pub fn default_database_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    database_root_for(&cwd)
}

/// F37 fix: root database untuk project tertentu — CWD-independen. Sebelumnya
/// `default_database_root()` memakai path RELATIF cwd → saat cwd berada di
/// direktori crate (mis. `cargo test -p mivon-env`), cache dibuat di
/// `crates/mivon-env/` — salah tempat. Root dihitung dari workspace root
/// eksplisit. Prioritas:
/// 1. env `MIVON_NODE_CACHE_DIR` (override manual, selalu dipakai).
/// 2. `<root>/node_cache`.
///
/// Satu sumber kebenaran: delegasi ke `ProjectCache::default_root_for`.
pub fn database_root_for(root: &Path) -> PathBuf {
    ProjectCache::default_root_for(root)
}

/// Project id dari cwd + sources + incdirs + defines.
pub fn project_id_for(
    root: &Path,
    sources: &[PathBuf],
    incdirs: &[PathBuf],
    defines: &[(String, String)],
) -> String {
    ProjectCache::project_id(root, sources, incdirs, defines)
}

/// Buka database project. Membuat direktori bila belum ada.
pub fn open_database(db_root: &Path, pid: &str) -> ProjectCache {
    ProjectCache::open_project(db_root, pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_root_nonempty() {
        let r = default_database_root();
        assert!(!r.as_os_str().is_empty());
    }

    // Test memanipulasi env var MIVON_NODE_CACHE_DIR — kunci global agar tidak
    // racy dengan test lain yang membaca env var yang sama (test paralel).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_database_root_for_is_root_based() {
        // F37: root database harus relatif ke workspace root, bukan cwd.
        // MIVON_NODE_CACHE_DIR di-unset (dalam lock) agar tidak memengaruhi test.
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("MIVON_NODE_CACHE_DIR").ok();
        std::env::remove_var("MIVON_NODE_CACHE_DIR");
        let r = database_root_for(Path::new("/some/project"));
        if let Some(v) = saved {
            std::env::set_var("MIVON_NODE_CACHE_DIR", v);
        }
        assert_eq!(
            r,
            PathBuf::from("/some/project/node_cache"),
            "root harus berdasarkan workspace root, bukan cwd"
        );
    }

    #[test]
    fn test_project_id_deterministic() {
        let root = Path::new("/proj");
        let sources = vec![PathBuf::from("a.sv"), PathBuf::from("b.sv")];
        let a = project_id_for(root, &sources, &[], &[]);
        let b = project_id_for(root, &sources, &[], &[]);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }
}
