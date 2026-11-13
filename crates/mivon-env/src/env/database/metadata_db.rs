use mivon_compiler::node_cache::{FileMeta, ProjectCache};
use std::path::Path;

/// Metadata file (hash konten, ukuran, status) dari database.
pub fn file_meta<'a>(db: &'a ProjectCache, path: &Path) -> Option<&'a FileMeta> {
    db.files.get(path)
}

/// Jumlah file yang terdaftar di database.
pub fn file_count(db: &ProjectCache) -> usize {
    db.files.len()
}

/// Jumlah file dengan status Recompiled (berubah pada build terakhir).
pub fn recompiled_count(db: &ProjectCache) -> usize {
    db.files
        .values()
        .filter(|m| m.status == mivon_compiler::node_cache::FileStatus::Recompiled)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_compiler::node_cache::{FileStatus, ProjectCache};
    use std::path::{Path, PathBuf};

    #[test]
    fn test_metadata_queries() {
        let root = std::env::temp_dir().join("mivon_db_meta");
        let _ = std::fs::create_dir_all(&root);
        let mut db = ProjectCache::open_project(&root, "pid-m");
        db.record_file(
            PathBuf::from("a.sv"),
            42,
            vec![],
            FileStatus::Recompiled,
            0,
            10,
            vec![],
        );
        assert_eq!(file_count(&db), 1);
        assert!(file_meta(&db, Path::new("a.sv")).is_some());
        assert_eq!(recompiled_count(&db), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
