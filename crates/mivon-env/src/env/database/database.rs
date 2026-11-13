use mivon_compiler::node_cache::ProjectCache;
use std::path::Path;

/// DatabaseContext — semua persistent storage (node_cache).
///
/// Compiler tidak pernah membuka file cache langsung. Context membungkus
/// ProjectCache: metadata, graph (deps), ast, preproc, verify, symbol, diags.
#[derive(Default)]
pub struct DatabaseContext {
    db: Option<ProjectCache>,
}

impl std::fmt::Debug for DatabaseContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseContext")
            .field("open", &self.is_open())
            .field(
                "files",
                &self.db.as_ref().map(|d| d.files.len()).unwrap_or(0),
            )
            .finish()
    }
}

impl DatabaseContext {
    pub fn new() -> Self {
        DatabaseContext { db: None }
    }

    /// Buka database project di `db_root` dengan `pid`.
    pub fn open(db_root: &Path, pid: &str) -> Self {
        DatabaseContext {
            db: Some(ProjectCache::open_project(db_root, pid)),
        }
    }

    /// Pasang database yang sudah dibuka caller.
    pub fn attach(&mut self, db: ProjectCache) {
        self.db = Some(db);
    }

    pub fn is_open(&self) -> bool {
        self.db.is_some()
    }

    pub fn db(&self) -> Option<&ProjectCache> {
        self.db.as_ref()
    }

    pub fn db_mut(&mut self) -> Option<&mut ProjectCache> {
        self.db.as_mut()
    }

    /// Simpan database (bila terbuka). Error dibawa ke caller.
    pub fn save(&mut self) -> Result<(), String> {
        if let Some(db) = self.db.as_mut() {
            db.save().map(|_| ()).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }

    /// Hapus isi database (bila terbuka).
    pub fn clear(&mut self) {
        if let Some(db) = self.db.as_mut() {
            let _ = db.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_context() {
        let mut ctx = DatabaseContext::new();
        assert!(!ctx.is_open());
        let root = std::env::temp_dir().join("mivon_db_ctx");
        let _ = std::fs::create_dir_all(&root);
        ctx.attach(ProjectCache::open_project(&root, "pid"));
        assert!(ctx.is_open());
        assert!(ctx.db().is_some());
        ctx.save().unwrap();
        ctx.clear();
        let _ = std::fs::remove_dir_all(&root);
    }
}
