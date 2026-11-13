//! `ProjectCache` — facade pipeline di atas engine node_cache.
//!
//! Seluruh state persistent disimpan sebagai node, bukan file database:
//!
//! * AST per file → node stage `parse` (artefak = bytes bincode AST);
//! * hasil preprocess per file → node stage `preprocess`;
//! * IR hasil elaborasi → node stage `elaborate`;
//! * metadata file, verify, diagnostic, symbol, type index, dependency graph,
//!   statistik, dan daftar snapshot → satu node stage `index` (payload bincode).
//!
//! Graph snapshot immutable di `graphs/`, reference project di `refs/`.

use crate::dependency::{checksum64, file_checksum, includes_valid};
use crate::entries::*;
use crate::filesystem::{unique_suffix, Layout};
use crate::gc::{self, GcStats};
use crate::graph::{GraphEntry, GraphSnapshot, GraphStore, RefRecord};
use crate::identity::{fingerprint, ContentId, NodeFingerprint, NodeIdentity};
use crate::manifest::{ManifestStore, NodeManifest, OutputRef};
use crate::object::ObjectStore;
use crate::precompiled;
use crate::transaction::Transaction;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Payload node `index`: seluruh state project yang tidak perlu dipecah.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectIndex {
    pub flags_hash: u64,
    pub files: HashMap<PathBuf, FileMeta>,
    pub verify: HashMap<u64, VerifyResult>,
    pub diags: HashMap<PathBuf, mivon_core::diagnostics::FileDiags>,
    pub symbols: SymbolIndex,
    pub type_index: HashMap<String, u64>,
    pub file_deps: FileGraph,
    pub stats: StatsDb,
    pub snapshots: Vec<u64>,
    pub changed: usize,
    pub last_snapshotted_changed: usize,
}

/// Cache project berbasis node_cache.
pub struct ProjectCache {
    /// Root node_cache.
    pub root: PathBuf,
    /// Project id (hash sumber + flags).
    pub pid: String,
    /// Root project (cwd/sumber).
    pub project_root: PathBuf,
    pub flags_hash: u64,
    pub files: HashMap<PathBuf, FileMeta>,
    pub verify: HashMap<u64, VerifyResult>,
    pub diags: HashMap<PathBuf, mivon_core::diagnostics::FileDiags>,
    pub symbols: SymbolIndex,
    pub type_index: HashMap<String, u64>,
    pub graph: FileGraph,
    pub stats_db: StatsDb,
    pub snapshots: Vec<u64>,
    pub restored: usize,
    pub changed: usize,
    pub last_snapshotted_changed: usize,
    pub dirty: bool,
    pub dirty_diag: bool,
    /// Artefak analisis per module (turunan node parse/elaborate).
    pub precompiled_db: Option<precompiled::PrecompiledDb>,
    layout: Layout,
    entries: HashMap<NodeIdentity, GraphEntry>,
    pending: HashMap<NodeIdentity, NodeManifest>,
    preproc_cache: HashMap<PathBuf, PreprocEntry>,
    ast_cache: HashMap<PathBuf, (u64, Arc<[u8]>)>,
    graph_id: Option<ContentId>,
    generation: u64,
    bytes_written: u64,
}

impl std::fmt::Debug for ProjectCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectCache")
            .field("root", &self.root)
            .field("pid", &self.pid)
            .field("files", &self.files.len())
            .field("entries", &self.entries.len())
            .field("dirty", &self.dirty)
            .finish()
    }
}

impl ProjectCache {
    /// Root default (`.mivon/node_cache`, override `MIVON_NODE_CACHE_DIR`).
    pub fn default_root() -> PathBuf {
        Layout::default_root_for_cwd()
    }

    /// Root untuk project tertentu (CWD-independen).
    pub fn default_root_for(root: &Path) -> PathBuf {
        Layout::default_root_for(root)
    }

    /// Project id deterministik dari root + sumber + flags.
    pub fn project_id(
        root: &Path,
        sources: &[PathBuf],
        incdirs: &[PathBuf],
        defines: &[(String, String)],
    ) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"mivon-project\0");
        h.update(root.to_string_lossy().as_bytes());
        let mut paths: Vec<String> = sources
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            h.update(&(p.len() as u64).to_le_bytes());
            h.update(p.as_bytes());
        }
        let mut incs: Vec<String> = incdirs.iter().map(|p| p.to_string_lossy().to_string()).collect();
        incs.sort();
        for i in incs {
            h.update(&(i.len() as u64).to_le_bytes());
            h.update(i.as_bytes());
        }
        let mut defs = defines.to_vec();
        defs.sort();
        for (k, v) in defs {
            h.update(&(k.len() as u64).to_le_bytes());
            h.update(k.as_bytes());
            h.update(&(v.len() as u64).to_le_bytes());
            h.update(v.as_bytes());
        }
        let hex = ContentId(*h.finalize().as_bytes()).to_hex();
        hex[..16].to_string()
    }

    /// Buka project (tanpa konteks sumber).
    /// Buka project berdasarkan direktori kerja saat ini (kompatibilitas).
    pub fn open(root: impl AsRef<Path>) -> Self {
        let proot = std::env::current_dir().unwrap_or_default();
        let pid = Self::project_id(&proot, &[], &[], &[]);
        Self::open_project(root.as_ref(), &pid)
    }

    pub fn open_project(db_root: &Path, pid: &str) -> Self {
        Self::open_inner(db_root, pid, &std::env::current_dir().unwrap_or_default())
    }

    /// Buka project dengan konteks root/sumber.
    pub fn open_project_with_context(
        db_root: &Path,
        pid: &str,
        project_root: &Path,
        _sources: &[PathBuf],
    ) -> Self {
        Self::open_inner(db_root, pid, project_root)
    }

    fn open_inner(db_root: &Path, pid: &str, project_root: &Path) -> Self {
        let layout = Layout::open(db_root).unwrap_or_else(|e| {
            eprintln!(
                "[node_cache] tidak bisa membuka {}: {} — cache berjalan read-only",
                db_root.display(),
                e
            );
            Layout::new(db_root)
        });
        let report = crate::recovery::recover(&layout);
        if !report.is_empty() {
            if std::env::var("MIVON_DEBUG_NODE_CACHE").is_ok() {
                eprintln!(
                    "[node_cache] recovery: completed={} discarded={} tmp={}",
                    report.journals_completed, report.journals_discarded, report.tmp_removed
                );
            }
        }
        let precompiled_db = Some(precompiled::PrecompiledDb::open(
            &precompiled::precompiled_root(&layout, pid),
        ));
        let mut cache = ProjectCache {
            root: db_root.to_path_buf(),
            pid: pid.to_string(),
            project_root: project_root.to_path_buf(),
            flags_hash: 0,
            files: HashMap::new(),
            verify: HashMap::new(),
            diags: HashMap::new(),
            symbols: SymbolIndex::new(),
            type_index: HashMap::new(),
            graph: FileGraph::new(),
            stats_db: StatsDb::new(),
            snapshots: Vec::new(),
            restored: 0,
            changed: 0,
            last_snapshotted_changed: 0,
            dirty: false,
            dirty_diag: false,
            precompiled_db,
            layout,
            entries: HashMap::new(),
            pending: HashMap::new(),
            preproc_cache: HashMap::new(),
            ast_cache: HashMap::new(),
            graph_id: None,
            generation: 0,
            bytes_written: 0,
        };
        cache.load_project();
        cache
    }

    fn graph_store(&self) -> GraphStore<'_> {
        GraphStore::new(&self.layout)
    }

    fn manifests(&self) -> ManifestStore<'_> {
        ManifestStore::new(&self.layout)
    }

    fn objects(&self) -> ObjectStore<'_> {
        ObjectStore::new(&self.layout)
    }

    /// Muat graph + index + artefak preprocess dari disk.
    fn load_project(&mut self) {
        let Some((graph_id, graph)) = self.graph_store().current(&self.pid) else {
            return;
        };
        self.graph_id = Some(graph_id);
        self.generation = graph.generation;
        self.flags_hash = graph.flags;
        for entry in &graph.entries {
            self.entries.insert(entry.node, entry.clone());
        }

        // Snapshot adalah reference terpisah `<pid>.snapshot.<id>` — daftar
        // snapshot dipulihkan dari nama reference (tidak bergantung pada
        // index yang dipublikasikan sebelum snapshot dibuat).
        let prefix = format!("{}.snapshot.", self.pid);
        if let Ok(entries) = std::fs::read_dir(self.layout.refs_dir()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(suffix) = name.strip_prefix(&prefix) {
                    if let Ok(id) = suffix.parse::<u64>() {
                        self.snapshots.push(id);
                    }
                }
            }
            self.snapshots.sort_unstable();
            self.snapshots.dedup();
        }

        let Some(index_node) = graph.index else { return };
        let Some(manifest) = self.manifests().read(&index_node) else {
            return;
        };
        let Some(object) = manifest.output("index") else { return };
        let Some(bytes) = self.objects().get(&object.content) else {
            return;
        };
        let Ok(index) = bincode::deserialize::<ProjectIndex>(&bytes) else {
            return;
        };
        self.files = index.files;
        self.verify = index.verify;
        self.diags = index.diags;
        self.symbols = index.symbols;
        self.type_index = index.type_index;
        self.graph = index.file_deps;
        self.stats_db = index.stats;
        for id in index.snapshots {
            if !self.snapshots.contains(&id) {
                self.snapshots.push(id);
            }
        }
        self.snapshots.sort_unstable();
        self.snapshots.dedup();
        self.changed = index.changed;
        self.last_snapshotted_changed = index.last_snapshotted_changed;

        // Muat hasil preprocess (dipakai jalur hangat tanpa clone dari disk).
        let preproc_nodes: Vec<NodeIdentity> = self
            .entries
            .values()
            .filter(|e| e.stage == "preprocess")
            .map(|e| e.node)
            .collect();
        for node in preproc_nodes {
            let Some(manifest) = self.manifests().read(&node) else {
                continue;
            };
            let Some(out) = manifest.output("preproc") else {
                continue;
            };
            let Some(bytes) = self.objects().get(&out.content) else {
                continue;
            };
            if let Ok(entry) = bincode::deserialize::<PreprocEntry>(&bytes) {
                let key = String::from_utf8(manifest.key.clone()).unwrap_or_default();
                let path = PathBuf::from(key);
                // Graph menyimpan SEMUA versi node historis. Hanya entry yang
                // cocok dengan hash file saat ini yang boleh masuk cache —
                // kalau tidak, versi lama bisa menimpa versi baru (urutan
                // iterasi HashMap tidak deterministik) dan warm run MISS.
                if let Some(meta) = self.files.get(&path) {
                    if meta.content_hash != entry.content_hash {
                        continue;
                    }
                }
                self.preproc_cache.insert(path, entry);
            }
        }
    }

    // ── Identitas node ───────────────────────────────────────────────

    fn content_input(hash: u64) -> ContentId {
        ContentId::of(&hash.to_le_bytes())
    }

    /// File yang terdampak (transitive) bila `changed` berubah.
    pub fn affected(&mut self, changed: &[PathBuf]) -> Vec<PathBuf> {
        self.graph.affected(changed)
    }

    /// Ringkasan satu baris untuk log/debug.
    pub fn summary(&self) -> String {
        format!(
            "pid={} files={} nodes={} flags={:x} dirty={}",
            self.pid,
            self.files.len(),
            self.entries.len(),
            self.flags_hash,
            self.dirty
        )
    }

    /// Simpan artefak + siapkan manifest (dipublikasikan saat save).
    fn stage_entry(
        &mut self,
        stage: &str,
        stage_version: u32,
        key: &[u8],
        inputs: Vec<ContentId>,
        payload: &[u8],
        schema: u32,
        kind: &str,
    ) -> io::Result<NodeIdentity> {
        let (content, size, newly) = self.objects().put(payload)?;
        if newly {
            self.bytes_written += size;
        }
        let fp = NodeFingerprint {
            stage,
            stage_version,
            key,
            inputs: &inputs,
            dependencies: &[],
            config: self.flags_hash,
            toolchain: TOOLCHAIN,
            target: "sv",
            schema,
        };
        let id = fp.id();
        let key_hash = fp.key_hash();
        let manifest = NodeManifest::new(id, stage, stage_version, key)
            .with_config(self.flags_hash)
            .with_toolchain(TOOLCHAIN)
            .with_target("sv")
            .with_schema(schema)
            .with_inputs(inputs)
            .with_output(OutputRef {
                kind: kind.to_string(),
                content,
                size,
                schema,
            });
        self.entries.insert(
            id,
            GraphEntry {
                node: id,
                stage: stage.to_string(),
                key_hash,
            },
        );
        self.pending.insert(id, manifest);
        Ok(id)
    }

    fn read_node(&self, id: &NodeIdentity) -> Option<NodeManifest> {
        if let Some(m) = self.pending.get(id) {
            return Some(m.clone());
        }
        self.manifests().read(id)
    }

    fn read_artifact(&self, id: &NodeIdentity, kind: &str) -> Option<Vec<u8>> {
        let manifest = self.read_node(id)?;
        let out = manifest.output(kind)?;
        self.objects().get(&out.content)
    }

    // ── File metadata ────────────────────────────────────────────────

    pub fn get_file_meta(&self, path: &Path) -> Option<&FileMeta> {
        self.files.get(path)
    }

    pub fn known_paths(&self) -> Vec<PathBuf> {
        self.files.keys().cloned().collect()
    }

    /// Rekam metadata file. Menambah counter `changed` bila hash berubah.
    pub fn record_file(
        &mut self,
        path: PathBuf,
        content_hash: u64,
        deps: Vec<PathBuf>,
        status: FileStatus,
        flags: u64,
        size: u64,
        include_hashes: Vec<(PathBuf, u64)>,
    ) {
        let prev_hash = self.files.get(&path).map(|m| m.content_hash);
        let changed = prev_hash != Some(content_hash);
        if changed {
            self.changed += 1;
        }
        let meta = FileMeta {
            path: path.clone(),
            content_hash,
            mtime_ns: std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
            size,
            status,
            flags_hash: flags,
            deps,
            include_hashes,
            compiled_at_ns: now_ns(),
            ast_format_version: crate::ast::AST_FORMAT_VERSION,
        };
        self.files.insert(path, meta);
        self.dirty = true;
    }

    /// Apakah AST/preprocessed untuk `path` masih valid untuk `content_hash`?
    /// Header include ikut diverifikasi (bukan hanya mtime/ukuran).
    pub fn deps_unchanged(&self, path: &Path, content_hash: u64) -> io::Result<bool> {
        let Some(meta) = self.files.get(path) else {
            return Ok(false);
        };
        if meta.content_hash != content_hash {
            return Ok(false);
        }
        Ok(includes_valid(&meta.include_hashes))
    }

    /// Tandai akses (kompatibilitas API; GC sekarang berbasis reference).
    pub fn touch_ast(&mut self, _path: &Path, _content_hash: u64) {}
    pub fn touch_preproc(&mut self, _path: &Path, _content_hash: u64) {}
    pub fn touch_verify(&mut self, _content_hash: u64) {}

    // ── AST ─────────────────────────────────────────────────────────

    pub fn has_valid_ast(&self, path: &Path, content_hash: u64) -> bool {
        self.ast_cache
            .get(path)
            .map(|(h, _)| *h == content_hash)
            .unwrap_or(false)
            || self
                .resolve_ast_node(path, content_hash)
                .map(|id| self.read_node(&id).is_some())
                .unwrap_or(false)
    }

    fn resolve_ast_node(&self, path: &Path, content_hash: u64) -> Option<NodeIdentity> {
        let key = path.to_string_lossy();
        let inputs = [Self::content_input(content_hash)];
        Some(
            fingerprint(
                "parse",
                1,
                key.as_bytes(),
                &inputs,
                self.flags_hash,
                TOOLCHAIN,
                "sv",
                crate::ast::AST_FORMAT_VERSION as u32,
            ),
        )
    }

    /// Ambil AST terserialisasi bila hash cocok.
    pub fn get_ast(&self, path: &Path, content_hash: u64) -> Option<Arc<[u8]>> {
        if let Some((h, bytes)) = self.ast_cache.get(path) {
            if *h == content_hash {
                return Some(Arc::clone(bytes));
            }
        }
        let id = self.resolve_ast_node(path, content_hash)?;
        self.read_artifact(&id, "ast").map(Arc::from)
    }

    pub fn cache_ast(&mut self, path: PathBuf, content_hash: u64, bytes: Vec<u8>) {
        let key = path.to_string_lossy().to_string();
        if let Err(e) = self.stage_entry(
            "parse",
            1,
            key.as_bytes(),
            vec![Self::content_input(content_hash)],
            &bytes,
            crate::ast::AST_FORMAT_VERSION as u32,
            "ast",
        ) {
            eprintln!("[node_cache] gagal menyimpan AST {}: {}", path.display(), e);
            return;
        }
        self.ast_cache
            .insert(path, (content_hash, Arc::from(bytes)));
        self.dirty = true;
    }

    // ── Preprocessed source ─────────────────────────────────────────

    pub fn get_preprocessed(&self, path: &Path, content_hash: u64) -> Option<PreprocEntry> {
        let entry = self.preproc_cache.get(path)?;
        (entry.content_hash == content_hash).then(|| entry.clone())
    }

    pub fn get_preprocessed_ref(
        &self,
        path: &Path,
        content_hash: u64,
    ) -> Option<&PreprocEntry> {
        let entry = self.preproc_cache.get(path)?;
        (entry.content_hash == content_hash).then_some(entry)
    }

    pub fn get_preprocessed_combined(&self, path: &Path, content_hash: u64) -> Option<&str> {
        self.get_preprocessed_ref(path, content_hash)
            .map(|e| e.combined.as_str())
    }

    pub fn has_valid_preproc(&self, path: &Path, content_hash: u64) -> bool {
        self.preproc_cache
            .get(path)
            .map(|e| e.content_hash == content_hash)
            .unwrap_or(false)
    }

    pub fn cache_preprocessed(&mut self, path: PathBuf, entry: PreprocEntry) {
        let key = path.to_string_lossy().to_string();
        let payload = match bincode::serialize(&entry) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[node_cache] gagal serialize preproc {}: {}", path.display(), e);
                return;
            }
        };
        if let Err(e) = self.stage_entry(
            "preprocess",
            1,
            key.as_bytes(),
            vec![Self::content_input(entry.content_hash)],
            &payload,
            PREPROC_SCHEMA,
            "preproc",
        ) {
            eprintln!("[node_cache] gagal menyimpan preproc {}: {}", path.display(), e);
            return;
        }
        self.preproc_cache.insert(path, entry);
        self.dirty = true;
    }

    /// Lepas AST dari RAM (payload sudah durable di object store).
    pub fn release_memory_cache(&mut self) {
        self.ast_cache.clear();
    }

    // ── Verify & diagnostic ─────────────────────────────────────────

    pub fn get_verify(&self, content_hash: u64) -> Option<&VerifyResult> {
        self.verify.get(&content_hash)
    }

    pub fn set_verify(&mut self, result: VerifyResult) {
        self.verify.insert(result.content_hash, result);
        self.dirty = true;
    }

    pub fn get_diags(&self, path: &Path) -> Option<&mivon_core::diagnostics::FileDiags> {
        self.diags.get(path)
    }

    pub fn set_diags(&mut self, diags: mivon_core::diagnostics::FileDiags) {
        self.diags.insert(diags.path.clone(), diags);
        self.dirty_diag = true;
    }

    // ── Index simbol / type / graph ─────────────────────────────────

    pub fn add_symbol(&mut self, name: String, kind: String, file: PathBuf) {
        self.symbols.add(name, kind, file);
        self.dirty = true;
    }

    pub fn set_symbol_def(&mut self, name: String, file: PathBuf) {
        self.symbols.set_def(name.clone(), file.clone());
        self.graph.set_def(name, file);
        self.dirty = true;
    }

    pub fn add_symbol_use(&mut self, file: PathBuf, name: String) {
        self.symbols.add_use(file, name);
        self.dirty = true;
    }

    pub fn set_module_type(&mut self, name: String, signature: u64) {
        self.type_index.insert(name, signature);
        self.dirty = true;
    }

    pub fn set_file_deps(&mut self, file: PathBuf, deps: Vec<PathBuf>) {
        self.graph.set_file_deps(file, deps);
        self.dirty = true;
    }

    /// Tandai seluruh file terverifikasi (parse + elaborate ok).
    pub fn mark_elaborated(&mut self) {
        let hashes: Vec<u64> = self.files.values().map(|m| m.content_hash).collect();
        for hash in hashes {
            let mut v = self
                .verify
                .get(&hash)
                .cloned()
                .unwrap_or_else(|| VerifyResult::fresh(hash));
            v.elab_ok = true;
            v.set_check(VerifyCheckKind::Elaborate, CheckResult::fresh());
            self.verify.insert(hash, v);
        }
        self.dirty = true;
    }

    // ── Prune ───────────────────────────────────────────────────────

    /// Buang file yang bukan bagian project aktif. Mengembalikan jumlah file
    /// yang dibuang. Object/node lama tidak dihapus di sini — GC yang mengurus.
    pub fn prune_stale(&mut self, active: &[PathBuf]) -> usize {
        let active: std::collections::HashSet<&PathBuf> = active.iter().collect();
        let before = self.files.len();
        self.files.retain(|p, _| active.contains(p));
        self.preproc_cache.retain(|p, _| active.contains(p));
        self.diags.retain(|p, _| active.contains(p));
        let removed = before.saturating_sub(self.files.len());
        if removed > 0 {
            self.dirty = true;
        }
        removed
    }

    // ── Elaborated IR ───────────────────────────────────────────────

    fn ir_key(top: &str, inputs_hash: u64, context: &str) -> String {
        format!("{}\u{0}{}\u{0}{}", top, inputs_hash, context)
    }

    fn ir_node(&self, top: &str, inputs_hash: u64, context: &str) -> NodeIdentity {
        let key = Self::ir_key(top, inputs_hash, context);
        fingerprint(
            "elaborate",
            1,
            key.as_bytes(),
            &[],
            self.flags_hash,
            TOOLCHAIN,
            "sv",
            crate::ast::IR_FORMAT_VERSION as u32,
        )
    }

    /// Sidik input elaborasi: seluruh file (path, hash, include) + flags + top.
    pub fn elaboration_inputs_hash(&self, top: &str) -> u64 {
        let mut h = blake3::Hasher::new();
        h.update(b"elaborate-inputs\0");
        h.update(self.pid.as_bytes());
        h.update(top.as_bytes());
        h.update(&self.flags_hash.to_le_bytes());
        h.update(&crate::ast::AST_FORMAT_VERSION.to_le_bytes());
        h.update(&crate::ast::IR_FORMAT_VERSION.to_le_bytes());
        let mut metas: Vec<&FileMeta> = self.files.values().collect();
        metas.sort_by(|a, b| a.path.cmp(&b.path));
        for meta in metas {
            h.update(meta.path.to_string_lossy().as_bytes());
            h.update(&meta.content_hash.to_le_bytes());
            let mut incs = meta.include_hashes.clone();
            incs.sort();
            for (p, hash) in incs {
                h.update(p.to_string_lossy().as_bytes());
                h.update(&hash.to_le_bytes());
            }
        }
        u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap())
    }

    pub fn store_elaborate_ir(&mut self, ir: &mivon_ir::IrDesign, inputs_hash: u64, context: &str) {
        let bytes = match bincode::serialize(ir) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[node_cache] gagal serialize IR: {}", e);
                return;
            }
        };
        let key = Self::ir_key(ir.top.name.as_str(), inputs_hash, context);
        if let Err(e) = self.stage_entry(
            "elaborate",
            1,
            key.as_bytes(),
            Vec::new(),
            &bytes,
            crate::ast::IR_FORMAT_VERSION as u32,
            "ir",
        ) {
            eprintln!("[node_cache] gagal menyimpan IR: {}", e);
            return;
        }
        self.dirty = true;
    }

    pub fn restore_elaborate_ir(
        &self,
        top: &str,
        expected_inputs_hash: u64,
        expected_context: &str,
    ) -> Option<mivon_ir::IrDesign> {
        let id = self.ir_node(top, expected_inputs_hash, expected_context);
        let debug = std::env::var("MIVON_DEBUG_NODE_CACHE").is_ok();
        let Some(manifest) = self.read_node(&id) else {
            if debug {
                eprintln!("[node_cache] IR miss: manifest {} tidak ada (top={:?})", id.short(), top);
            }
            return None;
        };
        let Some(out) = manifest.output("ir") else {
            if debug {
                eprintln!("[node_cache] IR miss: manifest {} tanpa output ir", id.short());
            }
            return None;
        };
        let Some(bytes) = self.objects().get(&out.content) else {
            if debug {
                eprintln!("[node_cache] IR miss: artefak {} hilang/rusak", out.content.short());
            }
            return None;
        };
        let Ok(ir) = bincode::deserialize::<mivon_ir::IrDesign>(&bytes) else {
            if debug {
                eprintln!("[node_cache] IR miss: payload tidak bisa di-deserialize");
            }
            return None;
        };
        (ir.top.name.as_str() == top).then_some(ir)
    }

    // ── Statistik ───────────────────────────────────────────────────

    pub fn set_stats(&mut self, profile: BuildProfile) {
        self.stats_db.record(profile);
        self.dirty = true;
    }

    // ── Persistensi ─────────────────────────────────────────────────

    fn flush_manifests(&mut self) -> io::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        for (_, manifest) in pending {
            if self.manifests().exists(&manifest.node) {
                continue;
            }
            for out in &manifest.outputs {
                if !self.objects().has_valid(&out.content, out.size) {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "artefak {} hilang sebelum manifest dipublikasikan",
                            out.kind
                        ),
                    ));
                }
            }
            let size = manifest.encode().len() as u64;
            self.manifests().write(&manifest)?;
            self.bytes_written += size;
        }
        Ok(())
    }

    fn index_payload(&self) -> Result<Vec<u8>, io::Error> {
        let index = ProjectIndex {
            flags_hash: self.flags_hash,
            files: self.files.clone(),
            verify: self.verify.clone(),
            diags: self.diags.clone(),
            symbols: self.symbols.clone(),
            type_index: self.type_index.clone(),
            file_deps: self.graph.clone(),
            stats: self.stats_db.clone(),
            snapshots: self.snapshots.clone(),
            changed: self.changed,
            last_snapshotted_changed: self.last_snapshotted_changed,
        };
        bincode::serialize(&index).map_err(io::Error::other)
    }

    fn write_index(&mut self) -> io::Result<NodeIdentity> {
        let payload = self.index_payload()?;
        let input = ContentId::of(&payload);
        self.stage_entry(
            "index",
            1,
            b"project",
            vec![input],
            &payload,
            INDEX_SCHEMA,
            "index",
        )
    }

    fn publish_graph(&mut self, note: &str) -> io::Result<ContentId> {
        self.flush_manifests()?;
        let index_node = self.write_index()?;
        self.flush_manifests()?;

        let generation = self.generation + 1;
        let mut graph = GraphSnapshot::new(&self.pid, generation, self.flags_hash, TOOLCHAIN);
        graph.note = note.to_string();
        graph.index = Some(index_node);
        graph.entries = self.entries.values().cloned().collect();
        graph.entries.sort_by(|a, b| a.node.cmp(&b.node));

        let graph_id = self.graph_store().write(&graph)?;
        let txn = Transaction::begin(&self.layout, &self.pid, graph_id, generation)?;
        txn.commit()?;
        self.graph_id = Some(graph_id);
        self.generation = generation;
        Ok(graph_id)
    }

    /// Simpan seluruh state sebagai satu graph snapshot + reference atomic.
    pub fn save(&mut self) -> io::Result<CacheStats> {
        if self.dirty || self.dirty_diag || !self.pending.is_empty() {
            self.publish_graph("")?;
            self.dirty = false;
            self.dirty_diag = false;
        }
        Ok(CacheStats {
            files: self.files.len(),
            restored_designs: self.restored,
            changed_files: self.changed,
            verify_hits: 0,
            verify_misses: 0,
            snapshot_id: self.snapshots.last().copied().unwrap_or(0),
            bytes_written: self.bytes_written,
        })
    }

    /// Snapshot graph berlabel (rollback point). Reference snapshot terpisah
    /// agar GC tidak menghapus graph lama.
    pub fn snapshot(&mut self, note: impl Into<String>) -> io::Result<u64> {
        let note = note.into();
        let graph_id = self.publish_graph(&note)?;
        let snapshot_id = u64::from_le_bytes(graph_id.as_bytes()[..8].try_into().unwrap());
        self.snapshots.push(snapshot_id);
        self.last_snapshotted_changed = self.changed;
        let snap_project = format!("{}.snapshot.{}", self.pid, snapshot_id);
        self.graph_store().write_ref(&RefRecord {
            project: snap_project,
            graph: graph_id,
            generation: self.generation,
        })?;
        self.dirty = true;
        Ok(snapshot_id)
    }

    /// Hapus state project (reference + snapshot refs) lalu jalankan GC.
    pub fn clear(&mut self) -> io::Result<()> {
        self.graph_store().remove_ref(&self.pid)?;
        let prefix = format!("{}.snapshot.", self.pid);
        if let Ok(entries) = std::fs::read_dir(self.layout.refs_dir()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = gc::collect(&self.layout);
        self.files.clear();
        self.verify.clear();
        self.diags.clear();
        self.symbols = SymbolIndex::new();
        self.type_index.clear();
        self.graph = FileGraph::new();
        self.stats_db = StatsDb::new();
        self.snapshots.clear();
        self.entries.clear();
        self.pending.clear();
        self.preproc_cache.clear();
        self.ast_cache.clear();
        self.changed = 0;
        self.last_snapshotted_changed = 0;
        self.restored = 0;
        self.generation = 0;
        self.graph_id = None;
        self.dirty = false;
        self.dirty_diag = false;
        Ok(())
    }

    /// Jalankan GC mark-and-sweep.
    pub fn gc(&self) -> io::Result<GcStats> {
        gc::collect(&self.layout)
    }

    /// Jumlah node yang diketahui pada graph sesaat ini.
    pub fn node_count(&self) -> usize {
        self.entries.len()
    }

    /// Graph id terakhir (untuk diagnostik).
    pub fn graph_id(&self) -> Option<ContentId> {
        self.graph_id
    }

    /// Path object/node (diagnostik).
    pub fn object_path(&self, id: &ContentId) -> PathBuf {
        self.layout.object_path(id)
    }

    /// Checksum isi file (helper untuk caller).
    pub fn file_checksum(path: &Path) -> Option<u64> {
        file_checksum(path)
    }

    /// Checksum isi bytes (helper untuk caller).
    pub fn checksum(bytes: &[u8]) -> u64 {
        checksum64(bytes)
    }
}

/// Suffix unik publik (dipakai test/fuzz untuk isolasi direktori).
pub fn unique_cache_suffix() -> String {
    unique_suffix()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mivon-project-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ))
    }

    fn source(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn record(cache: &mut ProjectCache, path: &Path) {
        let bytes = std::fs::read(path).unwrap();
        let hash = checksum64(&bytes);
        cache.record_file(
            path.to_path_buf(),
            hash,
            Vec::new(),
            FileStatus::Recompiled,
            0,
            bytes.len() as u64,
            Vec::new(),
        );
    }

    #[test]
    fn cold_warm_warm_reuses_nodes() {
        let root = temp_root("coldwarm");
        let proj = root.join("proj");
        let src = source(&proj, "a.sv", "module a; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[src.clone()], &[], &[]);

        {
            let mut cache = ProjectCache::open_project(&cache_root, &pid);
            record(&mut cache, &src);
            cache.cache_ast(src.clone(), checksum64(b"module a; endmodule"), b"AST".to_vec());
            cache
                .cache_preprocessed(
                    src.clone(),
                    PreprocEntry {
                        content_hash: checksum64(b"module a; endmodule"),
                        combined: "preprocessed".into(),
                        timescale: None,
                        timescale_segments: vec![(0, ("1ns".into(), "1ps".into()))],
                    },
                );
            cache.save().unwrap();
        }
        // Run 2: HIT.
        {
            let mut cache = ProjectCache::open_project(&cache_root, &pid);
            let hash = checksum64(&std::fs::read(&src).unwrap());
            assert_eq!(cache.get_ast(&src, hash).as_deref(), Some(&b"AST"[..]));
            assert_eq!(cache.get_preprocessed_combined(&src, hash), Some("preprocessed"));
            assert!(cache.deps_unchanged(&src, hash).unwrap());
            cache.save().unwrap();
        }
        // Run 3: tetap HIT tanpa invalidasi karena cache terpakai.
        {
            let cache = ProjectCache::open_project(&cache_root, &pid);
            let hash = checksum64(&std::fs::read(&src).unwrap());
            assert_eq!(cache.get_ast(&src, hash).as_deref(), Some(&b"AST"[..]));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn source_mutation_invalidates_only_that_file() {
        let root = temp_root("mutation");
        let proj = root.join("proj");
        let a = source(&proj, "a.sv", "module a; endmodule");
        let b = source(&proj, "b.sv", "module b; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[a.clone(), b.clone()], &[], &[]);

        let hash_a = checksum64(&std::fs::read(&a).unwrap());
        let hash_b = checksum64(&std::fs::read(&b).unwrap());
        {
            let mut cache = ProjectCache::open_project(&cache_root, &pid);
            record(&mut cache, &a);
            record(&mut cache, &b);
            cache.cache_ast(a.clone(), hash_a, b"AST-A".to_vec());
            cache.cache_ast(b.clone(), hash_b, b"AST-B".to_vec());
            cache.save().unwrap();
        }
        // Mutasi A dengan ukuran identik.
        std::fs::write(&a, "module a; endmodulE").unwrap();
        let new_hash_a = checksum64(&std::fs::read(&a).unwrap());
        assert_ne!(new_hash_a, hash_a);
        let cache = ProjectCache::open_project(&cache_root, &pid);
        assert!(cache.get_ast(&a, new_hash_a).is_none(), "A harus MISS");
        assert_eq!(cache.get_ast(&b, hash_b).as_deref(), Some(&b"AST-B"[..]), "B tetap HIT");
        assert!(!cache.deps_unchanged(&a, new_hash_a).unwrap());
        assert!(cache.deps_unchanged(&b, hash_b).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn timestamp_manipulation_does_not_create_false_hit() {
        let root = temp_root("timestamp");
        let proj = root.join("proj");
        let src = source(&proj, "a.sv", "module a; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[src.clone()], &[], &[]);
        let old_hash = checksum64(&std::fs::read(&src).unwrap());
        {
            let mut cache = ProjectCache::open_project(&cache_root, &pid);
            record(&mut cache, &src);
            cache.cache_ast(src.clone(), old_hash, b"AST".to_vec());
            cache.save().unwrap();
        }
        std::fs::write(&src, "module a; endmodulX").unwrap();
        let cache = ProjectCache::open_project(&cache_root, &pid);
        let new_hash = checksum64(&std::fs::read(&src).unwrap());
        assert_ne!(old_hash, new_hash);
        assert!(cache.get_ast(&src, new_hash).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compiler_upgrade_rejects_incompatible_artifacts() {
        let root = temp_root("upgrade");
        let proj = root.join("proj");
        let src = source(&proj, "a.sv", "module a; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[src.clone()], &[], &[]);
        let hash = checksum64(&std::fs::read(&src).unwrap());
        {
            let mut cache = ProjectCache::open_project(&cache_root, &pid);
            record(&mut cache, &src);
            cache.cache_ast(src.clone(), hash, b"AST".to_vec());
            cache.save().unwrap();
        }
        // Simulasikan skema stage berbeda: node AST dibaca dengan schema lain
        // (versi implementasi berubah) → resolver menolak.
        let cache = ProjectCache::open_project(&cache_root, &pid);
        let id = cache.resolve_ast_node(&src, hash).unwrap();
        let manifest = cache.read_node(&id).unwrap();
        let lossy = src.to_string_lossy();
        let fp = NodeFingerprint {
            stage: "parse",
            stage_version: 1,
            key: lossy.as_bytes(),
            inputs: &[ProjectCache::content_input(hash)],
            dependencies: &[],
            config: cache.flags_hash,
            toolchain: TOOLCHAIN,
            target: "sv",
            schema: crate::ast::AST_FORMAT_VERSION as u32 + 1,
        };
        assert!(crate::resolver::NodeResolver::new(&crate::filesystem::Layout::new(&cache_root))
            .valid_manifest(&manifest, &fp)
            == false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupted_object_is_rejected() {
        let root = temp_root("corrupt");
        let proj = root.join("proj");
        let src = source(&proj, "a.sv", "module a; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[src.clone()], &[], &[]);
        let hash = checksum64(&std::fs::read(&src).unwrap());
        {
            let mut cache = ProjectCache::open_project(&cache_root, &pid);
            record(&mut cache, &src);
            cache.cache_ast(src.clone(), hash, b"AST".to_vec());
            cache.save().unwrap();
        }
        // Rusak object AST.
        let layout = crate::filesystem::Layout::new(&cache_root);
        let id = ContentId::of(b"AST");
        std::fs::write(layout.object_path(&id), b"corrupt").unwrap();
        let cache = ProjectCache::open_project(&cache_root, &pid);
        assert!(cache.get_ast(&src, hash).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_keeps_referenced_artifacts() {
        let root = temp_root("gc");
        let proj = root.join("proj");
        let src = source(&proj, "a.sv", "module a; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[src.clone()], &[], &[]);
        let hash = checksum64(&std::fs::read(&src).unwrap());
        let mut cache = ProjectCache::open_project(&cache_root, &pid);
        record(&mut cache, &src);
        cache.cache_ast(src.clone(), hash, b"AST".to_vec());
        cache.save().unwrap();
        let stats = cache.gc().unwrap();
        assert_eq!(stats.objects_removed, 0, "artefak aktif tidak boleh dihapus");
        assert_eq!(cache.get_ast(&src, hash).as_deref(), Some(&b"AST"[..]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn default_root_is_project_level() {
        if std::env::var("MIVON_NODE_CACHE_DIR").is_ok() {
            return;
        }
        let root = Path::new("/proj");
        assert_eq!(
            ProjectCache::default_root_for(root),
            PathBuf::from("/proj/node_cache"),
            "cache langsung di root project, bukan di .mivon/"
        );
        assert!(!ProjectCache::default_root_for(root).starts_with("/proj/.mivon"));
    }

    #[test]
    fn affected_is_transitive() {
        let mut graph = FileGraph::new();
        graph.set_file_deps(PathBuf::from("top.sv"), vec![PathBuf::from("mid.sv")]);
        graph.set_file_deps(PathBuf::from("mid.sv"), vec![PathBuf::from("leaf.sv")]);
        let affected = graph.affected(&[PathBuf::from("leaf.sv")]);
        assert_eq!(
            affected,
            vec![PathBuf::from("mid.sv"), PathBuf::from("top.sv")]
        );
        assert!(graph.affected(&[PathBuf::from("none.sv")]).is_empty());
    }

    #[test]
    fn cross_project_objects_are_shared() {
        let root = temp_root("cross");
        let proj_a = root.join("a");
        let proj_b = root.join("b");
        let a = source(&proj_a, "top.sv", "module top; endmodule");
        let b = source(&proj_b, "top.sv", "module top; endmodule");
        let cache_root = root.join("cache");
        let pid_a = ProjectCache::project_id(&proj_a, &[a.clone()], &[], &[]);
        let pid_b = ProjectCache::project_id(&proj_b, &[b.clone()], &[], &[]);
        assert_ne!(pid_a, pid_b, "dua project harus punya identitas berbeda");

        {
            let mut cache = ProjectCache::open_project(&cache_root, &pid_a);
            record(&mut cache, &a);
            let hash = checksum64(&std::fs::read(&a).unwrap());
            cache.cache_ast(a.clone(), hash, b"SHARED".to_vec());
            cache.save().unwrap();
        }

        // Object content-addressed dipakai ulang lintas project: penulisan
        // kedua tidak menghasilkan file baru dan isinya identik.
        let layout = Layout::new(&cache_root);
        let store = ObjectStore::new(&layout);
        let (id, _, newly) = store.put(b"SHARED").unwrap();
        assert!(!newly, "artefak identik lintas project tidak ditulis ulang");
        assert_eq!(store.get(&id).as_deref(), Some(&b"SHARED"[..]));

        // Project B dapat membuka object tersebut tanpa mengenal node project A.
        let cache_b = ProjectCache::open_project(&cache_root, &pid_b);
        assert_eq!(cache_b.node_count(), 0);
        assert_eq!(store.get(&id).as_deref(), Some(&b"SHARED"[..]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_removes_project_state_and_gc_frees_objects() {
        let root = temp_root("clear");
        let proj = root.join("proj");
        let src = source(&proj, "a.sv", "module a; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[src.clone()], &[], &[]);
        let hash = checksum64(&std::fs::read(&src).unwrap());
        let mut cache = ProjectCache::open_project(&cache_root, &pid);
        record(&mut cache, &src);
        cache.cache_ast(src.clone(), hash, b"AST".to_vec());
        cache.save().unwrap();
        cache.clear().unwrap();
        let reopened = ProjectCache::open_project(&cache_root, &pid);
        assert!(reopened.files.is_empty());
        assert!(reopened.get_ast(&src, hash).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn elaborate_ir_roundtrip_is_guarded_by_input_hash() {
        let root = temp_root("ir");
        let cache_root = root.join("cache");
        let mut cache = ProjectCache::open_project(&cache_root, "pid-ir");
        let ir = mivon_ir::IrDesign::default();
        let top = ir.top.name.as_str().to_string();
        let ih = cache.elaboration_inputs_hash(&top);
        cache.store_elaborate_ir(&ir, ih, "StrictSimulation");
        cache.save().unwrap();
        let reopened = ProjectCache::open_project(&cache_root, "pid-ir");
        let restored = reopened
            .restore_elaborate_ir(&top, ih, "StrictSimulation")
            .expect("IR harus restore");
        assert_eq!(restored.top.name.as_str(), top);
        assert!(
            reopened
                .restore_elaborate_ir(&top, ih ^ 1, "StrictSimulation")
                .is_none(),
            "input hash beda → MISS"
        );
        assert!(
            reopened
                .restore_elaborate_ir(&top, ih, "AnalysisRecovery")
                .is_none(),
            "context beda → MISS"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_is_published_and_referenced() {
        let root = temp_root("snapshot");
        let proj = root.join("proj");
        let src = source(&proj, "a.sv", "module a; endmodule");
        let cache_root = root.join("cache");
        let pid = ProjectCache::project_id(&proj, &[src.clone()], &[], &[]);
        let mut cache = ProjectCache::open_project(&cache_root, &pid);
        record(&mut cache, &src);
        cache.save().unwrap();
        let id = cache.snapshot("build").unwrap();
        assert!(id != 0);
        let reopened = ProjectCache::open_project(&cache_root, &pid);
        assert!(reopened.snapshots.contains(&id));
        let layout = crate::filesystem::Layout::new(&cache_root);
        let graphs = GraphStore::new(&layout);
        assert!(graphs.read_ref(&format!("{}.snapshot.{}", pid, id)).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
