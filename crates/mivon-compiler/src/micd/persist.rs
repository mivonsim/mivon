//! Menyimpan database ke disk: tulis singleton/entries, object AST &
//! preprocess, plus statistik ringkas.
use super::*;

impl MicdDatabase {
    /// Simpan store ke disk dalam satu transaksi (Kritik 4 & 5 db.md).
    ///
    /// Protokol:
    /// 1. Serialisasi SEMUA store yang dirty ke memori (belum menyentuh disk).
    /// 2. Tulis `journal.mdb` (intent: daftar file yang ikut transaksi).
    /// 3. Tulis tiap store ke `*.mdb.tmp` + fsync (belum commit).
    /// 4. Rename semua tmp → final (commit point, per-file atomik).
    /// 5. Hapus journal.
    ///
    /// Crash di fase 1–3 → file final tidak tersentuh. Crash di fase 4 →
    /// journal tersisa; recovery di [`MicdDatabase::open`] memvalidasi
    /// checksum tiap store terdaftar dan membuang yang corrupt. Tidak pernah
    /// ada database "setengah" tanpa jejak recovery.
    pub fn save(&mut self) -> io::Result<MicdStats> {
        // GC otomatis (Kritik 6 db.md): jalankan SEBELUM cek dirty agar entry
        // yang di-evict ikut ditulis ulang pada save ini. Best-effort —
        // eviction hanya menandai store terkait, bukan menggagalkan save.
        // Fase 4: + GC cache terpadu (satu budget global) + reklamasi pid idle.
        if self.gc_on_save {
            run_gc(self, &GcConfig::default());
            if let Some(layer) = self.cache_layer.as_mut() {
                gc::run_cache_gc(layer, gc::GLOBAL_CACHE_BUDGET_BYTES);
            }
            let _ = gc::reclaim_idle_projects(&self.root, &self.pid, &gc::ReclaimConfig::default());
        }
        // Registry: catat last_built (tiap run, warm atau tidak) di root.
        register_project(self);
        let any_dirty = self.dirty
            || self.dirty_ast
            || self.dirty_preproc
            || self.dirty_verify
            || self.dirty_symbol
            || self.dirty_type
            || self.dirty_graph
            || self.dirty_stats
            || self.dirty_diag;
        if !any_dirty {
            // State tidak berubah, tapi lapisan cache pipeline bisa saja dirty
            // (mis. tool menulis cache saja). Simpan cache, state dilewati.
            if let Some(layer) = self.cache_layer.as_mut() {
                if layer.is_dirty() {
                    let _ = layer.save();
                }
            }
            let stats = MicdStats {
                files: self.files.len(),
                restored_designs: self.restored,
                changed_files: self.changed,
                verify_hits: self.verify_hits(),
                verify_misses: self.verify_misses(),
                snapshot_id: self.snapshots.last().copied().unwrap_or(0),
                bytes_written: 0,
            };
            return Ok(stats);
        }

        std::fs::create_dir_all(self.state_dir())?;
        std::fs::create_dir_all(self.objects_dir())?;
        std::fs::create_dir_all(self.root.join(DIR_LOCKS))?;

        // ── Fase 1: serialisasi semua store yang dirty ke memori. ──
        let mut pending: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let st = self.state_dir();

        // metadata.mdb
        if self.dirty {
            let manifest = MetadataManifest {
                paths: self.files.keys().cloned().collect(),
                flags_hash: self.flags_hash,
                compiler_version: manifest_compiler_version(),
                created_ns: self.created_ns,
                schema_version: SCHEMA_VERSION,
                stage_schemas: current_stage_schemas(),
            };
            let mut w = MdbWriter::new();
            w.put(
                KEY_SINGLETON,
                format::KIND_MANIFEST,
                bincode::serialize(&manifest).map_err(io::Error::other)?,
            );
            for (path, meta) in self.files.iter() {
                let key = path_hash(path);
                w.put(
                    key,
                    format::KIND_META,
                    bincode::serialize(meta).map_err(io::Error::other)?,
                );
            }
            pending.push((
                st.join(DB_METADATA),
                w.serialize().map_err(io::Error::other)?,
            ));
        }

        // graph.mdb — rebuild reverse index sebelum serialize (set_deps
        // tidak rebuild per-call).
        if self.dirty_graph {
            self.graph.rebuild();
            pending.push((
                st.join(DB_GRAPH),
                serialize_mdb_singleton_compressed(
                    &self.graph,
                    format::KIND_GRAPH,
                    format::Compression::Lz4,
                )?,
            ));
        }

        // verify.mdb
        if self.dirty_verify {
            let mut w = MdbWriter::new();
            for v in self.verify.values() {
                w.put(
                    v.content_hash,
                    format::KIND_VERIFY,
                    bincode::serialize(v).map_err(io::Error::other)?,
                );
            }
            pending.push((st.join(DB_VERIFY), w.serialize().map_err(io::Error::other)?));
        }

        // diagnostics.mdb — hanya tulis bila dirty_diag (Kritik B: sebelumnya
        // ikut dirty metadata → tulis ulang metadata.mdb tanpa perlu).
        if self.dirty_diag {
            let mut w = MdbWriter::new();
            for (path, d) in self.diags.iter() {
                w.put(
                    path_hash(path),
                    format::KIND_DIAG,
                    bincode::serialize(d).map_err(io::Error::other)?,
                );
            }
            pending.push((st.join(DB_DIAG), w.serialize().map_err(io::Error::other)?));
        }

        // symbol.mdb
        if self.dirty_symbol {
            pending.push((
                st.join(DB_SYMBOL),
                serialize_mdb_singleton(&self.symbols, format::KIND_SYMBOL)?,
            ));
        }

        // types.mdb
        if self.dirty_type {
            pending.push((
                st.join(DB_TYPE),
                serialize_mdb_singleton(&self.type_index, format::KIND_TYPE)?,
            ));
        }

        // ── Objek CAS (payload immutable) — ditulis di luar transaksi batch:
        // tiap objek atomik sendiri (temp + rename), content-addressed.
        // Objek lama yang tidak lagi dirujuk metadata di-sweep (GC).
        if self.dirty_ast {
            self.write_ast_objects()?;
        }
        if self.dirty_preproc {
            self.write_preproc_objects()?;
        }

        // stats.mdb (Kritik 14 db.md).
        if self.dirty_stats {
            pending.push((
                st.join(DB_STATS),
                serialize_mdb_singleton(&self.stats_db, format::KIND_STATS)?,
            ));
        }

        // ── Fase 2–5: transaksi (lock → journal → tmp → commit → bersihkan). ──
        // Lock writer exclusive (Kritik 7 db.md): dua writer tidak boleh
        // commit berbarengan. Reader tidak lock (rename atomik + mmap).
        let _lock = acquire_write_lock(
            &self.lock_path(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        let names: Vec<PathBuf> = pending
            .iter()
            .map(|(p, _)| p.file_name().map(PathBuf::from).unwrap_or_default())
            .collect();
        write_journal(&st.join(DB_JOURNAL), &names)?;
        for (p, data) in &pending {
            format::write_tmp(p, data)?;
        }
        let mut bytes_written = 0u64;
        for (p, data) in &pending {
            format::commit_tmp(p)?;
            bytes_written += data.len() as u64;
        }
        let _ = std::fs::remove_file(st.join(DB_JOURNAL));

        // Lapisan cache pipeline (db.md cache/): simpan store kategori yang
        // berubah. Best-effort — kegagalan cache tidak menggagalkan save
        // state utama (cache non-kritis, dibangun ulang otomatis).
        if let Some(layer) = self.cache_layer.as_mut() {
            if layer.is_dirty() {
                let _ = layer.save();
            }
        }

        // Artefak precompiled: simpan manifest bila ada perubahan.
        if let Some(pdb) = self.precompiled_db.as_mut() {
            if pdb.dirty {
                let _ = pdb.save();
            }
        }

        let stats = MicdStats {
            files: self.files.len(),
            restored_designs: self.restored,
            changed_files: self.changed,
            verify_hits: self.verify_hits(),
            verify_misses: self.verify_misses(),
            snapshot_id: self.snapshots.last().copied().unwrap_or(0),
            bytes_written,
        };
        self.dirty = false;
        self.dirty_ast = false;
        self.dirty_preproc = false;
        self.dirty_verify = false;
        self.dirty_symbol = false;
        self.dirty_type = false;
        self.dirty_graph = false;
        self.dirty_stats = false;
        self.dirty_diag = false;
        self.changed = 0;
        Ok(stats)
    }

    /// Tulis objek AST (payload CAS): satu file per konten hash. Skip bila
    /// objek sudah ada (content-addressed → isi identik). Lalu sweep objek
    /// `.ast` yang tidak lagi dirujuk `ast_cache` (GC disk).
    fn write_ast_objects(&self) -> io::Result<()> {
        let objs = self.objects_dir();
        std::fs::create_dir_all(&objs)?;
        let mut live: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for (hash, bytes) in self.ast_cache.values() {
            live.insert(*hash);
            let obj = self.object_path(*hash, OBJ_AST);
            if obj.exists() {
                continue;
            }
            let payload =
                bincode::serialize(&(AST_FORMAT_VERSION, &**bytes)).map_err(io::Error::other)?;
            // Object CAS — tanpa fsync (lihat write_tmp_unflushed): file partial
            // akibat crash ditolak saat load (bincode deserialize + AST version
            // check → miss → dibangun ulang).
            format::write_tmp_unflushed(&obj, &payload)?;
            format::commit_tmp(&obj)?;
        }
        sweep_objects(&objs, OBJ_AST, &live)
    }

    /// Tulis objek preprocessed source (payload CAS) + sweep seperti AST.
    fn write_preproc_objects(&self) -> io::Result<()> {
        let objs = self.objects_dir();
        std::fs::create_dir_all(&objs)?;
        let mut live: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for entry in self.preproc_cache.values() {
            live.insert(entry.content_hash);
            let obj = self.object_path(entry.content_hash, OBJ_PREPROC);
            if obj.exists() {
                continue;
            }
            let payload = bincode::serialize(&entry).map_err(io::Error::other)?;
            format::write_tmp_unflushed(&obj, &payload)?;
            format::commit_tmp(&obj)?;
        }
        sweep_objects(&objs, OBJ_PREPROC, &live)
    }

    /// Bersihkan seluruh database.
    pub fn clear(&mut self) -> io::Result<()> {
        let _lock = acquire_write_lock(
            &self.lock_path(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        let st = self.state_dir();
        for name in [
            DB_METADATA,
            DB_GRAPH,
            DB_AST,
            DB_PREPROC,
            DB_VERIFY,
            DB_DIAG,
            DB_SYMBOL,
            DB_TYPE,
            DB_STATS,
            DB_JOURNAL,
        ] {
            let _ = std::fs::remove_file(st.join(name));
            let _ = std::fs::remove_file(st.join(name).with_extension("mdb.tmp"));
        }
        let _ = std::fs::remove_dir_all(st.join("snapshots"));
        let _ = std::fs::remove_dir_all(self.objects_dir());
        // Hapus pid dari registri.
        let mut map = read_registry(&self.root);
        map.remove(&self.pid);
        write_registry(&self.root, &map);
        // Lapisan cache pipeline dihapus + dibuka ulang (kosong).
        let _ = CacheLayer::remove_all(&self.root, &self.pid);
        self.cache_layer = CacheLayer::open(&self.root, &self.pid, 0).ok();
        // Artefak precompiled dihapus + dibuka ulang (kosong).
        let precompiled_root = self.root.join(DIR_PRECOMPILED).join(&self.pid);
        let _ = std::fs::remove_dir_all(&precompiled_root);
        self.precompiled_db = Some(precompiled::PrecompiledDb::open(&precompiled_root));
        self.files.clear();
        self.graph = FileGraph::new();
        self.verify.clear();
        self.verify_ast_index.clear();
        self.verify_semantic_index.clear();
        self.diags.clear();
        self.symbols = SymbolIndex::new();
        self.ast_cache.clear();
        self.preproc_cache.clear();
        self.type_index.clear();
        self.stats_db = StatsDb::new();
        self.snapshots.clear();
        self.dirty = false;
        self.dirty_ast = false;
        self.dirty_preproc = false;
        self.dirty_verify = false;
        self.dirty_symbol = false;
        self.dirty_type = false;
        self.dirty_graph = false;
        self.dirty_stats = false;
        self.dirty_diag = false;
        Ok(())
    }

    fn verify_hits(&self) -> usize {
        self.verify.values().filter(|v| v.ok()).count()
    }
    fn verify_misses(&self) -> usize {
        self.files.len().saturating_sub(self.verify_hits())
    }

    /// Statistik ringkas untuk CLI.
    pub fn summary(&self) -> String {
        format!(
            "files={} restored_ast={} changed={} verify_hits={} snapshots={}",
            self.files.len(),
            self.restored,
            self.changed,
            self.verify_hits(),
            self.snapshots.len()
        )
    }
}

