use mivon_compiler::node_cache::ProjectCache;
use std::path::Path;

/// Deps file-level: file yang bergantung pada `file`.
pub fn deps_of(db: &ProjectCache, file: &Path) -> Vec<std::path::PathBuf> {
    db.graph.deps_of(file)
}

/// File definisi simbol (dependency level simbol).
pub fn def_of<'a>(db: &'a ProjectCache, symbol: &str) -> Option<&'a std::path::PathBuf> {
    db.graph.def_of(symbol)
}

/// Jumlah edge file di graph.
pub fn file_dep_count(db: &ProjectCache) -> usize {
    db.graph.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_compiler::node_cache::ProjectCache;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_graph_queries() {
        let root = std::env::temp_dir().join("mivon_db_graph");
        let _ = std::fs::create_dir_all(&root);
        let mut db = ProjectCache::open_project(&root, "pid-g");
        db.set_file_deps(PathBuf::from("a.sv"), vec![PathBuf::from("b.sv")]);
        db.set_symbol_def("b".into(), PathBuf::from("b.sv"));
        assert_eq!(deps_of(&db, Path::new("a.sv")), vec![PathBuf::from("b.sv")]);
        assert_eq!(def_of(&db, "b"), Some(&PathBuf::from("b.sv")));
        assert!(file_dep_count(&db) >= 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
