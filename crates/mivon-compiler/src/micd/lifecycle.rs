//! Siklus hidup database: statistik build, prune entri basi, penanda
//! elaborasi, IR cache, diagnostic per file, dan `affected`.
use super::*;

impl MicdDatabase {
    /// Rekam profil build ke stats.mdb (Kritik 14 db.md).
    ///
    /// Fase 2 (Kritik A): sekaligus cermin ke `cache/profile` key `"last"`
    /// (format yang dibaca `mprof`) — menggantikan tulis manual di
    /// `save_micd` (satu jalur, bukan dua).
    pub fn set_stats(&mut self, profile: BuildProfile) {
        if let Ok(bytes) = bincode::serialize(&profile) {
            if let Some(layer) = self.cache_layer.as_mut() {
                let _ = layer.put(cache::CacheCategory::Profile, "last", &bytes);
            }
        }
        self.stats_db.record(profile);
        self.dirty_stats = true;
    }

    /// Buang file yang tidak lagi menjadi bagian dari project aktif.
    ///
    /// MICD adalah cache per-project: database menyimpan file yang di-compile
    /// sesi ini (sources + libfiles). File dari run lain yang menempel di root
    /// yang sama (mis. `test/counter.sv` setelah `--filelist opentitan_rtl.f`)
    /// adalah sampah lintas-project — AST/preproc/verify/diag miliknya harus
    /// dibuang agar `files` dan cache mencerminkan project aktif, bukan
    /// akumulasi semua project yang pernah dicoba di direktori ini.
    ///
    /// Mengembalikan jumlah file yang dibuang. Menandai dirty (metadata,
    /// ast, preproc, diag) bila ada yang dihapus.
    pub fn prune_stale(&mut self, active: &[PathBuf]) -> usize {
        let active_set: std::collections::HashSet<&PathBuf> = active.iter().collect();
        let stale: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|p| !active_set.contains(p))
            .cloned()
            .collect();
        if stale.is_empty() {
            return 0;
        }
        for p in &stale {
            self.files.remove(p);
            self.ast_accessed.remove(p);
            self.preproc_accessed.remove(p);
            if let Some((_, b)) = self.ast_cache.remove(p) {
                self.ast_bytes = self.ast_bytes.saturating_sub(b.len() as u64);
            }
            if let Some(e) = self.preproc_cache.remove(p) {
                self.preproc_bytes = self.preproc_bytes.saturating_sub(e.combined.len() as u64);
            }
            self.diags.remove(p);
            self.graph.remove_file(p);
        }
        // Sweep CAS objects di disk secara langsung (Gap #13): objek yang
        // tidak lagi dirujuk ast_cache/preproc_cache dihapus sekarang, bukan
        // ditunda ke save() berikutnya — mencegah akumulasi sampah bila save()
        // gagal atau tidak dipanggil.
        {
            let live_ast: std::collections::HashSet<u64> =
                self.ast_cache.values().map(|(h, _)| *h).collect();
            let _ = sweep_objects(&self.objects_dir(), OBJ_AST, &live_ast);
            let live_preproc: std::collections::HashSet<u64> = self
                .preproc_cache
                .values()
                .map(|e| e.content_hash)
                .collect();
            let _ = sweep_objects(&self.objects_dir(), OBJ_PREPROC, &live_preproc);
        }
        self.dirty = true;
        self.dirty_ast = true;
        self.dirty_preproc = true;
        self.dirty_diag = true;
        stale.len()
    }

    /// Tandai seluruh verify entry sebagai ter-elaborasi + simpan verify.mdb
    /// SAJA (ringan — tidak menulis ulang metadata/ast/graph).
    /// GC di-skip: mark_elaborated hanya menandai flag, tidak perlu evict.
    pub fn mark_elaborated(&mut self) -> io::Result<()> {
        for v in self.verify.values_mut() {
            v.elab_ok = true;
            v.set_check(VerifyCheckKind::Elaborate, CheckResult::pass(v.result_hash));
        }
        self.dirty_verify = true;
        let prev_gc = self.gc_on_save;
        self.gc_on_save = false;
        let result = self.save().map(|_| ());
        self.gc_on_save = prev_gc;
        result
    }

    /// Sidik seluruh input elaborasi (Fase 6): lipat terurut (path,
    /// content_hash, include_hashes) semua file + flags_hash + top. IR cache
    /// hanya valid bila sidik saat restore SAMA dengan saat store — libfile
    /// yang berubah (tak tercakup gerbang hitung-file) tetap menggagalkan
    /// reuse. Deterministik (sort path) lintas run.
    pub fn elaboration_inputs_hash(&self, top: &str) -> u64 {
        use crate::cache::checksum::{checksum_fold, compute_checksum};
        let mut paths: Vec<&PathBuf> = self.files.keys().collect();
        paths.sort();
        let mut h: Vec<u64> = Vec::with_capacity(paths.len() * 2 + 3);
        for p in paths {
            let m = &self.files[p];
            h.push(compute_checksum(p.to_string_lossy().as_bytes()));
            h.push(m.content_hash);
            for (dep, hh) in &m.include_hashes {
                h.push(compute_checksum(dep.to_string_lossy().as_bytes()));
                h.push(*hh);
            }
        }
        h.push(self.flags_hash);
        h.push(compute_checksum(top.as_bytes()));
        checksum_fold(&h)
    }

    /// Simpan `IrDesign` hasil elaborasi LENGKAP ke cache pipeline (db.md
    /// "5. elaborate/", key `ir:<top>`). Dipakai warm run untuk melewati
    /// elaborator sepenuhnya. Best-effort — gagal menyimpan IR tidak
    /// menggagalkan save utama.
    pub fn store_elaborate_ir(&mut self, ir: &mivon_ir::IrDesign, inputs_hash: u64, context: &str) {
        use crate::micd::cache::CacheCategory;
        let Some(layer) = self.cache_layer.as_mut() else {
            return;
        };
        // IR dengan array memori flatten raksasa (AetherX cache 1e9 bit dll):
        // bincode::serialize penuh = buffer 1GB+ sementara + 1GB cache —
        // menyumbang puncak heap (heaptrack: ~1.2GiB x2 utk 41 modul) & OOM
        // mesin kecil. Warm run mengelaborasi ulang (AetherX elab ~2.4s) —
        // jauh lebih murah daripada serialize+restore 1GB. Skip cache bila
        // estimasi total bit sinyal besar. IR normal tetap di-cache.
        let est_bits: u64 = ir
            .modules
            .values()
            .flat_map(|m| m.signals.iter())
            .map(|s| s.width as u64)
            .sum();
        const IR_CACHE_SKIP_BITS: u64 = 8 * 1024 * 1024; // 8M bit sinyal → skip
        if est_bits > IR_CACHE_SKIP_BITS {
            if std::env::var("MIVON_DEBUG_MICD").is_ok() {
                eprintln!(
                    "[MICD] skip IR cache: est_bits={} (> {}) — serialize 1GB+ utk desain besar",
                    est_bits, IR_CACHE_SKIP_BITS
                );
            }
            return;
        }
        let Ok(bytes) = crate::micd::ast::serialize_ir(ir) else {
            return;
        };
        // Key menyertakan IR_FORMAT_VERSION: skema IR berubah (tanpa bump
        // PIPELINE_REV) → entry lama tidak pernah di-restore. Cache layer
        // sendiri juga di-invalidasi via manifest.compiler_version.
        let key = format!(
            "ir:v{}:{}",
            crate::micd::ast::IR_FORMAT_VERSION,
            ir.top.name.as_str()
        );
        let _ = layer.put(CacheCategory::Elaborate, &key, &bytes);
        // Fase 6: sidecar (sidik input, konteks mode) — restore wajib cocok
        // (anti stale IR saat input berubah tanpa mengubah hitungan file).
        let sidecar = format!(
            "irinputs:v{}:{}",
            crate::micd::ast::IR_FORMAT_VERSION,
            ir.top.name.as_str()
        );
        if let Ok(sb) = bincode::serialize(&(inputs_hash, context)) {
            let _ = layer.put(CacheCategory::Elaborate, &sidecar, &sb);
        }
    }

    /// Coba restore `IrDesign` hasil elaborasi dari cache pipeline (db.md
    /// "5. elaborate/"). `None` bila tidak ada entry / corrupt / top berbeda
    /// / sidik input beda / konteks mode beda (Fase 6: libfile/flag/header
    /// berubah → jangan pakai).
    pub fn restore_elaborate_ir(
        &mut self,
        top: &str,
        expected_inputs_hash: u64,
        expected_context: &str,
    ) -> Option<mivon_ir::IrDesign> {
        use crate::micd::cache::CacheCategory;
        let layer = self.cache_layer.as_mut()?;
        let key = format!("ir:v{}:{}", crate::micd::ast::IR_FORMAT_VERSION, top);
        let sidecar = format!("irinputs:v{}:{}", crate::micd::ast::IR_FORMAT_VERSION, top);
        let sb = layer.get(CacheCategory::Elaborate, &sidecar)?;
        let (saved_hash, saved_ctx): (u64, String) = bincode::deserialize(&sb).ok()?;
        if saved_hash != expected_inputs_hash || saved_ctx != expected_context {
            return None;
        }
        let bytes = layer.get(CacheCategory::Elaborate, &key)?;
        let ir = crate::micd::ast::deserialize_ir(&bytes)?;
        if ir.top.name.as_str() != top {
            return None;
        }
        Some(ir)
    }

    pub fn get_diags(&self, path: &Path) -> Option<&FileDiags> {
        self.diags.get(path)
    }

    pub fn set_diags(&mut self, diags: FileDiags) {
        self.diags.insert(diags.path.clone(), diags);
        self.dirty_diag = true;
    }

    /// File yang terdampak bila `changed` berubah (via dependency graph).
    pub fn affected(&mut self, changed: &[PathBuf]) -> Vec<PathBuf> {
        self.graph.affected(changed)
    }

}

