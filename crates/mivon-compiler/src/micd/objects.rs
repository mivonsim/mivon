//! Akses object cache: AST & hasil preprocess per content-hash (get/touch/
//! cache/release), plus metadata per file.
use super::*;

impl MicdDatabase {
    /// Path tersimpan di metadata.
    pub fn known_paths(&self) -> Vec<PathBuf> {
        self.files.keys().cloned().collect()
    }

    pub fn get_file_meta(&self, path: &Path) -> Option<&FileMeta> {
        self.files.get(path)
    }

    /// Apakah AST file `path` valid untuk `content_hash` ini?
    pub fn has_valid_ast(&self, path: &Path, content_hash: u64) -> bool {
        self.ast_cache
            .get(path)
            .map(|(h, _)| *h == content_hash)
            .unwrap_or(false)
    }

    /// Ambil AST terserialisasi bila hash cocok. Mengembalikan Arc clone
    /// (zero-copy: increment refcount, bukan memcpy penuh).
    pub fn get_ast(&self, path: &Path, content_hash: u64) -> Option<Arc<[u8]>> {
        let (h, bytes) = self.ast_cache.get(path)?;
        if *h == content_hash {
            Some(Arc::clone(bytes))
        } else {
            None
        }
    }

    /// Tandai entry cache sebagai baru diakses (LRU, Kritik 6 db.md).
    /// Dipanggil setelah read/restore agar entry yang dipakai tidak di-evict.
    pub fn touch_ast(&mut self, path: &Path, content_hash: u64) {
        if self.has_valid_ast(path, content_hash) {
            self.ast_accessed.insert(path.to_path_buf(), now_ns());
        }
    }

    pub fn touch_preproc(&mut self, path: &Path, content_hash: u64) {
        if self
            .preproc_cache
            .get(path)
            .map(|e| e.content_hash == content_hash)
            .unwrap_or(false)
        {
            self.preproc_accessed.insert(path.to_path_buf(), now_ns());
        }
    }

    pub fn touch_verify(&mut self, content_hash: u64) {
        if self.verify.contains_key(&content_hash) {
            self.verify_accessed.insert(content_hash, now_ns());
        }
    }

    /// Simpan AST cache. Melacak `ast_bytes` (budget GC, Kritik 6 db.md).
    /// Payload disimpan sebagai `Arc<[u8]>` untuk zero-copy clone di get_ast().
    pub fn cache_ast(&mut self, path: PathBuf, content_hash: u64, bytes: Vec<u8>) {
        let changed = match self.ast_cache.get(&path) {
            Some((h, b)) => *h != content_hash || **b != *bytes,
            None => true,
        };
        if changed {
            if let Some((_, old)) = self.ast_cache.get(&path) {
                self.ast_bytes = self.ast_bytes.saturating_sub(old.len() as u64);
            }
            self.ast_bytes = self.ast_bytes.saturating_add(bytes.len() as u64);
            self.ast_accessed.insert(path.clone(), now_ns());
        }
        self.ast_cache
            .insert(path, (content_hash, Arc::from(bytes)));
        if changed {
            self.dirty = true;
            self.dirty_ast = true;
        }
    }

    /// Ambil combined source bila hash cocok.
    pub fn get_preprocessed(&self, path: &Path, content_hash: u64) -> Option<PreprocEntry> {
        let e = self.preproc_cache.get(path)?;
        if e.content_hash == content_hash {
            Some(e.clone())
        } else {
            None
        }
    }

    /// Fase 0 (Kritik B): referensi tanpa clone ke entry preprocessed.
    /// Jalur panas (`attach_micd`) hanya butuh `combined` — pakai ini lalu
    /// clone `&str`-nya saja, tanpa clone `timescale_segments` (Vec) yang
    /// bisa besar. `None` bila path tak ada / hash beda.
    pub fn get_preprocessed_ref(&self, path: &Path, content_hash: u64) -> Option<&PreprocEntry> {
        let e = self.preproc_cache.get(path)?;
        if e.content_hash == content_hash {
            Some(e)
        } else {
            None
        }
    }

    /// Fase 0 (Kritik B): referensi `&str` ke combined source (tanpa clone).
    /// Dipakai jalur panas restore + debug check.
    pub fn get_preprocessed_combined(&self, path: &Path, content_hash: u64) -> Option<&str> {
        self.get_preprocessed_ref(path, content_hash)
            .map(|e| e.combined.as_str())
    }

    /// Fase 0: cek validitas preproc tanpa clone (simetri `has_valid_ast`).
    pub fn has_valid_preproc(&self, path: &Path, content_hash: u64) -> bool {
        self.preproc_cache
            .get(path)
            .map(|e| e.content_hash == content_hash)
            .unwrap_or(false)
    }

    /// Lepas cache AST/preprocessed IN-MEMORY setelah disimpan ke disk
    /// (release_parse_cache). Payload sudah content-addressed immutable —
    /// restore berikutnya membaca dari disk; menahan ~1GB+ bytes AST
    /// dalam RAM selama elaborasi/sim = bocor memori (OOM design besar).
    pub fn release_memory_cache(&mut self) {
        self.ast_cache.clear();
        self.preproc_cache.clear();
        self.ast_bytes = 0;
        self.preproc_bytes = 0;
    }

    pub fn cache_preprocessed(&mut self, path: PathBuf, entry: PreprocEntry) {
        let changed = match self.preproc_cache.get(&path) {
            Some(old) => old.content_hash != entry.content_hash || old.combined != entry.combined,
            None => true,
        };
        if changed {
            if let Some(old) = self.preproc_cache.get(&path) {
                self.preproc_bytes = self.preproc_bytes.saturating_sub(old.combined.len() as u64);
            }
            self.preproc_bytes = self
                .preproc_bytes
                .saturating_add(entry.combined.len() as u64);
            self.preproc_accessed.insert(path.clone(), now_ns());
        }
        self.preproc_cache.insert(path, entry);
        if changed {
            self.dirty = true;
            self.dirty_preproc = true;
        }
    }

}

