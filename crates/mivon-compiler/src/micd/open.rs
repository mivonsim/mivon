//! Membuka database: baca registry/lock, restore metadata, graph, AST, dan
//! object preprocessed dari disk (mmap) ke memori.
use super::*;

impl MicdDatabase {
    /// Coba buat root; bila gagal (mis. parent berupa file, seperti `.mivon`
    /// project file untuk `-f`), fallback otomatis ke folder terpisah
    /// agar database tetap tersimpan — bukan hanya "save warning".
    pub fn open(root: &Path) -> MicdDatabase {
        Self::open_with_pid(root, "default")
    }

    /// Buka database root untuk sebuah project. Layout Git-style:
    /// `VERSION` + `registry.json` + `locks/` di root; payload immutable di
    /// `objects/<pid>/`, index mutable di `state/<pid>/`.
    pub fn open_with_pid(root: &Path, pid: &str) -> MicdDatabase {
        let root = if std::fs::create_dir_all(root).is_ok() {
            root.to_path_buf()
        } else {
            let alt = Self::fallback_root();
            let _ = std::fs::create_dir_all(&alt);
            alt
        };

        // VERSION — penanda skema database di root (Kritik 3 db.md).
        let version_path = root.join(FILE_VERSION);
        let version_ok = std::fs::read_to_string(&version_path)
            .map(|v| v.trim() == SCHEMA_VERSION.to_string())
            .unwrap_or(false);
        if !version_ok {
            let _ = std::fs::write(&version_path, format!("{}\n", SCHEMA_VERSION));
        }

        let st = state_dir(&root, pid);

        // Migrasi layout lama `projects/<pid>/` → `state/` + `objects/`.
        migrate_legacy(&root, pid);
        // Fase 2: sapu store mati (`ast.mdb`/`preproc.mdb` gaya lama di
        // state/ — tidak dibaca lagi sejak CAS). Perluasan terakhir legacy.
        sweep_dead_stores(&st);

        // Crash recovery (Kritik 5 db.md): journal tersisa → transaksi
        // sebelumnya terputus. Validasi store yang terdaftar, buang yang
        // corrupt; sisanya dibangun ulang di save berikutnya.
        recover(&st, &st.join(DB_JOURNAL));

        // Fase 5 (Fase 3b): baca manifest DULU untuk keputusan evict —
        // wipe total hanya bila base/schema tak cocok; beda versi STAGE
        // hanya menggugurkan kategori cache terkait (state tetap).
        // SATU reader dipakai untuk keputusan + load entri (tanpa re-read:
        // anti TOCTOU antar-proses di antara dua baca).
        let mdb: Option<MdbReader> = MdbReader::open(&st.join(DB_METADATA)).ok();
        let stored_manifest: Option<MetadataManifest> = mdb.as_ref().and_then(|r| {
            r.get(KEY_SINGLETON)
                .and_then(|b| bincode::deserialize::<MetadataManifest>(&b).ok())
        });
        let base_ok = stored_manifest.as_ref().is_some_and(|m| {
            m.schema_version == SCHEMA_VERSION
                && stored_compiler_version_matches(&m.compiler_version)
        });
        // Stage yang versinya beda dari current (Fase 5). Manifest tanpa
        // stage_schemas + base ok hanya dari tulisan tangan (produksi selalu
        // 21 entri) → `unknown_stages` = wipe total (strict, aman).
        let mut mismatched_stages: Vec<String> = Vec::new();
        let mut unknown_stages = false;
        if base_ok {
            let stored = stored_manifest.as_ref().unwrap();
            if stored.stage_schemas.is_empty() {
                unknown_stages = true;
            } else {
                let cur = current_stage_schemas();
                for (name, ver) in &cur {
                    if stored.stage_schemas.iter().find(|(n, _)| n == name)
                        != Some(&(name.clone(), *ver))
                    {
                        mismatched_stages.push(name.clone());
                    }
                }
                for (name, _) in &stored.stage_schemas {
                    if !cur.iter().any(|(n, _)| n == name) {
                        mismatched_stages.push(name.clone());
                    }
                }
            }
        }

        let mut db = MicdDatabase {
            root: root.clone(),
            pid: pid.to_string(),
            registry: ProjectInfo::default(),
            flags_hash: 0,
            compiler_version: manifest_compiler_version(),
            created_ns: now_ns(),
            schema_version: 0,
            files: HashMap::new(),
            graph: FileGraph::new(),
            verify: HashMap::new(),
            verify_ast_index: HashMap::new(),
            verify_semantic_index: HashMap::new(),
            diags: HashMap::new(),
            symbols: SymbolIndex::new(),
            ast_cache: HashMap::new(),
            preproc_cache: HashMap::new(),
            type_index: HashMap::new(),
            stats_db: StatsDb::new(),
            ast_accessed: HashMap::new(),
            preproc_accessed: HashMap::new(),
            verify_accessed: HashMap::new(),
            ast_bytes: 0,
            preproc_bytes: 0,
            gc_on_save: true,
            dirty: false,
            dirty_ast: false,
            dirty_preproc: false,
            dirty_verify: false,
            dirty_symbol: false,
            dirty_type: false,
            dirty_graph: false,
            dirty_stats: false,
            dirty_diag: false,
            snapshots: Vec::new(),
            restored: 0,
            changed: 0,
            last_snapshotted_changed: 0,
            cache_layer: None,
            precompiled_db: None,
        };

        db.snapshots = list_snapshots(&st);

        // Fase 5: gugurkan direktori kategori cache yang versinya beda
        // SEBELUM layer dibuka (dibuka = fresh). Nama disanitasi: hanya
        // kategori dikenal yang boleh dihapus (anti path-traversal dari
        // manifest rusak/jahat).
        for stage in &mismatched_stages {
            if cache::CacheCategory::from_name(stage).is_some() {
                let _ = std::fs::remove_dir_all(root.join(cache::DIR_CACHE).join(pid).join(stage));
            }
        }

        // Database artefak precompiled dibuka di open_cache_and_precompiled
        // (setelah keputusan wipe — lihat bawah).

        // metadata.mdb — pintu schema (Kritik 3 db.md). Fase 5: base/schema
        // tak cocok ATAU stage tak-dikenal → wipe total; beda versi stage
        // (dikenal) → hanya kategori cache terkait yang digugurkan di atas,
        // state (metadata/graph/verify/...) tetap di-load.
        //
        // Batasan jujur (S1): versi stage meng-gate FORMAT payload cache
        // kategori itu saja. Perubahan SEMANTIK (hasil analisis beda untuk
        // input sama) WAJIB bump `-p<N>` (COMPILER_VERSION), bukan versi
        // stage — kecuali stage `verify` yang sekaligus menggugurkan
        // verify.mdb di bawah (hasil verifikasi = downstream semua stage).
        let mut schema_ok = false;
        if base_ok && !unknown_stages {
            if let Some(r) = mdb.as_ref() {
                if let Some(manifest) = r
                    .get(KEY_SINGLETON)
                    .and_then(|b| bincode::deserialize::<MetadataManifest>(&b).ok())
                {
                    // base_ok dari pre-read menjamin versi cocok.
                    schema_ok = true;
                    db.schema_version = manifest.schema_version;
                    db.flags_hash = manifest.flags_hash;
                    db.compiler_version = manifest.compiler_version;
                    db.created_ns = manifest.created_ns;
                    for p in manifest.paths {
                        let key = path_hash(&p);
                        if let Some(b) = r.get(key) {
                            if let Ok(meta) = bincode::deserialize::<FileMeta>(&b) {
                                db.files.insert(p, meta);
                            }
                        }
                    }
                }
            }
        }
        if !schema_ok {
            // Kritik 3 db.md: schema berubah → SELURUH database lama tidak
            // kompatibel. Buang store lama secara deterministik (bukan
            // dibiarkan) agar tidak ada state campuran — mis. metadata schema
            // baru + graph/object/verify lama yang di-load pada run berikutnya.
            // Fase 5: wipe BENAR-BENAR total — termasuk cache/<pid> dan
            // precompiled/<pid> (dibuka ulang fresh di bawah).
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
            ] {
                let _ = std::fs::remove_file(st.join(name));
            }
            let _ = std::fs::remove_dir_all(objects_dir(&root, pid));
            let _ = std::fs::remove_dir_all(root.join(cache::DIR_CACHE).join(pid));
            let _ = std::fs::remove_dir_all(db.root.join(DIR_PRECOMPILED).join(&db.pid));
            db.schema_version = SCHEMA_VERSION;
            open_cache_and_precompiled(&mut db, &root, pid);
            return db;
        }

        // Jalur normal (termasuk evict selektif): buka layer setelah
        // direktori kategori basi digugurkan.
        open_cache_and_precompiled(&mut db, &root, pid);

        // graph.mdb
        if let Some(g) = read_mdb_singleton::<FileGraph>(&st, DB_GRAPH) {
            db.graph = g;
        }

        // verify.mdb — multi-entry: content_hash → VerifyResult
        for v in read_mdb_entries::<VerifyResult>(&st, DB_VERIFY) {
            if v.ast_hash != 0 {
                db.verify_ast_index.insert(v.ast_hash, v.content_hash);
            }
            if v.semantic_hash != 0 {
                db.verify_semantic_index
                    .insert(v.semantic_hash, v.content_hash);
            }
            let at = v.verified_at_ns;
            db.verify_accessed.insert(v.content_hash, at);
            db.verify.insert(v.content_hash, v);
        }
        // Fase 5 (S1): stage `verify` bump = hasil verifikasi tak dipercaya —
        // gugurkan verify.mdb dari memori + tandai dirty agar save menulis
        // ulang tanpa entri basi (file lama ikut tertimpa saat save).
        if mismatched_stages.iter().any(|s| s == "verify") {
            db.verify.clear();
            db.verify_ast_index.clear();
            db.verify_semantic_index.clear();
            db.verify_accessed.clear();
            db.dirty_verify = true;
        }

        // diagnostics.mdb — multi-entry: path_hash → FileDiags
        for d in read_mdb_entries::<FileDiags>(&st, DB_DIAG) {
            db.diags.insert(d.path.clone(), d);
        }

        // symbol.mdb
        if let Some(s) = read_mdb_singleton::<SymbolIndex>(&st, DB_SYMBOL) {
            db.symbols = s;
        }

        // Objek AST — payload CAS di `objects/<pid>/<hex-hash>.ast`.
        // Path → hash disimpan di metadata store; objek dibaca per file.
        // Content-addressed: file dengan konten identik berbagi satu objek.
        for (p, meta) in db.files.iter() {
            let obj = db.object_path(meta.content_hash, OBJ_AST);
            if let Ok(b) = std::fs::read(&obj) {
                if let Ok((ver, bytes)) = bincode::deserialize::<(u64, Vec<u8>)>(&b) {
                    if ver == AST_FORMAT_VERSION {
                        db.ast_bytes = db.ast_bytes.saturating_add(bytes.len() as u64);
                        let at = db
                            .files
                            .get(p)
                            .map(|m| m.compiled_at_ns)
                            .unwrap_or_else(now_ns);
                        db.ast_accessed.insert(p.clone(), at);
                        db.ast_cache
                            .insert(p.clone(), (meta.content_hash, Arc::from(bytes)));
                    }
                }
            }
        }

        // Objek preprocessed source — payload CAS `objects/<pid>/<hex-hash>.preproc`.
        for (p, meta) in db.files.iter() {
            let obj = db.object_path(meta.content_hash, OBJ_PREPROC);
            if let Ok(b) = std::fs::read(&obj) {
                if let Ok(entry) = bincode::deserialize::<PreprocEntry>(&b) {
                    if entry.content_hash == meta.content_hash {
                        db.preproc_bytes =
                            db.preproc_bytes.saturating_add(entry.combined.len() as u64);
                        let at = db
                            .files
                            .get(p)
                            .map(|m| m.compiled_at_ns)
                            .unwrap_or_else(now_ns);
                        db.preproc_accessed.insert(p.clone(), at);
                        db.preproc_cache.insert(p.clone(), entry);
                    }
                }
            }
        }

        // types.mdb (signature index)
        if let Some(m) = read_mdb_singleton::<HashMap<String, u64>>(&st, DB_TYPE) {
            db.type_index = m;
        }

        // stats.mdb (Kritik 14 db.md)
        if let Some(s) = read_mdb_singleton::<StatsDb>(&st, DB_STATS) {
            db.stats_db = s;
        }

        db
    }

}

