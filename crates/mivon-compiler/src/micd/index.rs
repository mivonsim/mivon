//! Index per-file: `record_file`, dependency graph, verification cache, dan
//! index simbol/tipe.
use super::*;

impl MicdDatabase {
    /// Daftarkan hasil kompilasi satu file.
    /// Tidak menandai `dirty` bila metadata identik (mtime diabaikan —
    /// agar warm run tidak menulis ulang metadata.mdb tanpa perubahan nyata).
    #[allow(clippy::too_many_arguments)]
    pub fn record_file(
        &mut self,
        path: PathBuf,
        content_hash: u64,
        deps: Vec<PathBuf>,
        status: FileStatus,
        flags_hash: u64,
        size: u64,
        include_hashes: Vec<(PathBuf, u64)>,
    ) {
        let prev = self.files.get(&path).map(|m| m.content_hash);
        if prev != Some(content_hash) {
            self.changed += 1;
        }
        let changed = match self.files.get(&path) {
            Some(p) => {
                p.content_hash != content_hash
                    || p.deps != deps
                    || p.include_hashes != include_hashes
                    || p.flags_hash != flags_hash
                    || p.size != size
                    || p.status != status
            }
            None => true,
        };
        let meta = FileMeta {
            path: path.clone(),
            content_hash,
            mtime_ns: std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
            size,
            status,
            flags_hash,
            deps,
            include_hashes,
            compiled_at_ns: now_ns(),
            ast_format_version: AST_FORMAT_VERSION,
        };
        self.files.insert(path, meta);
        if changed {
            self.dirty = true;
        }
    }

    /// Verifikasi bahwa seluruh dependensi file (termasuk include) tidak
    /// berubah sejak hash `content_hash` tersimpan. `None` jika file tidak
    /// terdaftar. Ini koreksi correctness: preprocessed output meng-embed
    /// konten header — bila header berubah, cache tidak boleh di-reuse.
    pub fn deps_unchanged(&self, path: &Path, content_hash: u64) -> Option<bool> {
        let meta = self.files.get(path)?;
        if meta.content_hash != content_hash {
            return Some(false);
        }
        for (dep, stored_hash) in &meta.include_hashes {
            let current = std::fs::read(dep)
                .ok()
                .map(|b| crate::cache::checksum::compute_checksum(&b));
            match current {
                // Header hilang → tidak bisa diverifikasi → jangan reuse.
                None => return Some(false),
                Some(h) if h != *stored_hash => return Some(false),
                Some(_) => {}
            }
        }
        Some(true)
    }

    pub fn get_verify(&self, content_hash: u64) -> Option<&VerifyResult> {
        self.verify.get(&content_hash)
    }

    /// Tambah simbol + tandai symbol.mdb perlu ditulis ulang.
    pub fn add_symbol(&mut self, name: String, kind: String, path: PathBuf) {
        self.symbols.add(name, kind, path);
        self.dirty_symbol = true;
    }

    /// Set signature tipe module + tandai types.mdb perlu ditulis ulang.
    pub fn set_module_type(&mut self, name: String, sig: u64) {
        self.type_index.insert(name, sig);
        self.dirty_type = true;
    }

    /// Set dependensi file di graph + tandai graph.mdb perlu ditulis ulang.
    pub fn set_file_deps(&mut self, file: PathBuf, deps: Vec<PathBuf>) {
        self.graph.set_deps(file, deps);
        self.dirty_graph = true;
    }

    /// Set definisi simbol di graph (level simbol, Kritik 2) + tandai
    /// graph.mdb perlu ditulis ulang.
    pub fn set_symbol_def(&mut self, name: String, file: PathBuf) {
        self.graph.set_symbol_def(name, file);
        self.dirty_graph = true;
    }

    /// Catat pemakaian simbol di graph (Kritik 2) + tandai graph.mdb perlu
    /// ditulis ulang.
    pub fn add_symbol_use(&mut self, file: PathBuf, symbol: String) {
        self.graph.add_symbol_use(file, symbol);
        self.dirty_graph = true;
    }

    /// Fase 2 (Kritik A): selain state/ (`verify.mdb`), tulis juga cermin ke
    /// kategori `cache/verify` (format `VerifyPayload` identik dengan yang
    /// dulu ditulis populator; belum ada pembaca tools hari ini — future-proof
    /// menuju satu engine).
    /// Satu setter = satu jalur tulis (write-through); populator derivasi
    /// terpisah tidak lagi dibutuhkan untuk kategori ini (Fase 2b menghapusnya).
    /// Best-effort: gagal tulis cache tidak menggagalkan state.
    pub fn set_verify(&mut self, result: VerifyResult) {
        if result.ast_hash != 0 {
            self.verify_ast_index
                .insert(result.ast_hash, result.content_hash);
        }
        if result.semantic_hash != 0 {
            self.verify_semantic_index
                .insert(result.semantic_hash, result.content_hash);
        }
        self.verify_accessed.insert(result.content_hash, now_ns());
        // Cermin cache ditulis SEBELUM move (borrow result masih hidup).
        self.mirror_verify_to_cache(&result);
        self.verify.insert(result.content_hash, result);
        self.dirty = true;
        self.dirty_verify = true;
    }

    /// Cermin `VerifyResult` → `cache/verify` (format baca tools).
    fn mirror_verify_to_cache(&mut self, result: &VerifyResult) {
        let Some(layer) = self.cache_layer.as_mut() else {
            return;
        };
        let payload = cache::pipeline::VerifyPayload {
            parse_ok: result.parse_ok,
            elab_ok: result.elab_ok,
            err_count: result.err_count,
            warn_count: result.warn_count,
            info_count: result.info_count,
            checks: result.checks.iter().map(|(k, c)| (*k, c.clone())).collect(),
        };
        if let Ok(bytes) = bincode::serialize(&payload) {
            let key = format!("{:016x}", result.content_hash);
            let _ = layer.put(cache::CacheCategory::Verify, &key, &bytes);
        }
    }

    /// Lookup verification dengan multi-level hash (Kritik 1 db.md).
    /// Prioritas: content hash → AST hash → semantic hash. Bila level
    /// AST/semantic yang cocok, hasil verifikasi tetap valid walau content
    /// hash berubah (mis. hanya komentar yang berubah) — tidak perlu
    /// lint/verify ulang.
    ///
    /// Semua level O(1) via index (Kritik E: sebelumnya semantic = O(n) scan).
    pub fn reuse_verify(
        &self,
        content_hash: u64,
        ast_hash: u64,
        semantic_hash: u64,
    ) -> Option<&VerifyResult> {
        if let Some(v) = self.verify.get(&content_hash) {
            return Some(v);
        }
        if ast_hash != 0 {
            if let Some(ch) = self.verify_ast_index.get(&ast_hash) {
                if let Some(v) = self.verify.get(ch) {
                    if v.matches_ast(ast_hash) {
                        return Some(v);
                    }
                }
            }
        }
        if semantic_hash != 0 {
            if let Some(ch) = self.verify_semantic_index.get(&semantic_hash) {
                if let Some(v) = self.verify.get(ch) {
                    if v.matches_semantic(semantic_hash) {
                        return Some(v);
                    }
                }
            }
        }
        None
    }
}
