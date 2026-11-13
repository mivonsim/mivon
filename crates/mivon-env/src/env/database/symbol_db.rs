use mivon_compiler::node_cache::ProjectCache;

/// Query index simbol: cari file definisi `name` berjenis `kind`.
pub fn locate_symbol<'a>(
    db: &'a ProjectCache,
    name: &str,
    kind: &str,
) -> Option<&'a std::path::PathBuf> {
    db.symbols.locate(name, kind)
}

/// Daftar nama simbol yang terindeks.
pub fn symbol_names(db: &ProjectCache) -> Vec<String> {
    db.symbols.names()
}

/// Jumlah simbol terindeks.
pub fn symbol_count(db: &ProjectCache) -> usize {
    db.symbols.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_compiler::node_cache::ProjectCache;
    use std::path::PathBuf;

    #[test]
    fn test_symbol_queries() {
        let root = std::env::temp_dir().join("mivon_db_symbol");
        let _ = std::fs::create_dir_all(&root);
        let mut db = ProjectCache::open_project(&root, "pid-test");
        db.add_symbol("top".into(), "module".into(), PathBuf::from("top.sv"));
        assert_eq!(symbol_count(&db), 1);
        assert_eq!(
            locate_symbol(&db, "top", "module"),
            Some(&PathBuf::from("top.sv"))
        );
        assert!(locate_symbol(&db, "nope", "module").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
