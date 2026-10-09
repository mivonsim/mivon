//! MICD — Mivon Incremental Compilation Database.
//!
//! Object database biner (bukan SQL) di `project/.mivon/database/` yang
//! membuat compile lintas run menjadi incremental: file yang tidak berubah
//! tidak di-lex/di-parse/di-verifikasi ulang. Terintegrasi otomatis ke
//! `run` dan `run_fast` — bukan flag tambahan.
//!
//! ```text
//! <db>/                        (default: .mivon/database/)
//!     VERSION                  — versi skema database
//!     registry.json            — pid → info project (root, sources, waktu)
//!     locks/<pid>.lock         — writer lock exclusive per project
//!     objects/<pid>/           — payload IMMUTABLE, content-addressed (CAS)
//!         <hash>.ast           — Design terserialisasi per konten hash
//!         <hash>.preproc       — combined source per konten hash
//!     state/<pid>/             — index MUTABLE per project
//!         metadata.mdb         — per-file: hash konten, mtime, size, status, deps
//!         graph.mdb            — dependency graph file-level (CSR + reverse index)
//!         verify.mdb           — verification cache (by content hash)
//!         diagnostics.mdb      — diagnostic per file (query IDE tanpa compile)
//!         symbol.mdb           — index simbol (module/package → file)
//!         types.mdb            — index tipe/signature module
//!         stats.mdb            — profil build (mprof/mbench)
//!         journal.mdb          — transaksi (crash recovery)
//!         snapshots/build-NNN  — snapshot build (rollback)
//! ```
//!
//! Pemisahan payload vs index (pola Git `objects/` + `refs/`): objek AST dan
//! preprocessed source bersifat immutable dan content-addressed — dua file
//! dengan konten sama berbagi satu objek (dedup), GC tinggal membuang objek
//! yang tidak lagi dirujuk metadata. Index (metadata/graph/verify/symbol/
//! types/diag) adalah state mutable yang ditulis transaksional via journal.

pub mod ast;
pub mod cache;
pub mod diag;
pub mod format;
pub mod gc;
pub mod graph;
pub mod lock;
pub mod metadata;
pub mod precompiled;
pub mod snapshot;
pub mod stats;
pub mod stringpool;
pub mod symbol;
pub mod txn;
pub mod verify;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use ast::{deserialize_design, serialize_design, AST_FORMAT_VERSION};
pub use cache::{CacheCategory, CacheLayer, CacheLayerStats};
pub use diag::{DiagEntry, DiagSeverity, FileDiags};
pub use format::{MdbReader, MdbWriter};
pub use gc::{
    run_cache_gc, run_gc, GcConfig, GcStats, ReclaimConfig, GLOBAL_CACHE_BUDGET_BYTES,
};
pub use graph::FileGraph;
pub use lock::{acquire_write_lock, is_writer_locked, WriteLock};
pub use metadata::{flags_hash, path_hash, FileMeta, FileStatus, MetadataManifest};
pub use precompiled::{PortInfo, PrecompiledDb, PrecompiledModule, PrecompiledStats};
pub use snapshot::{
    history_of, last_snapshot_id, list_snapshots, merge_base, parents_of, read_snapshot,
    snapshot_path, Snapshot,
};
pub use stats::{peak_rss_kb, BuildProfile, StatsDb};
pub use symbol::SymbolIndex;
pub use txn::{read_journal, recover, write_journal, Journal};
pub use verify::{now_ns, CheckResult, VerifyCheckKind, VerifyResult};

/// Versi compiler yang menulis database — bagian dari `project_id()`:
/// berubah → pid baru → seluruh state/ + objects/ + cache/ project dibangun
/// ulang dari kosong (tidak ada reuse antar versi).
///
/// Suffix **`-p<N>`** adalah revisi perilaku pipeline kompilasi: **NAIKKAN
/// `<N>` setiap kali output kompilasi untuk source yang sama berubah**
/// (elaborator/simulator/parser/const-fold semantics — mis. fix signedness,
/// operator precedence). Versi crate tidak berubah antar build dev, jadi tanpa
/// bump manual ini cache MICD akan me-restore hasil LAMA dari binary BARU
/// (stale IR bug — ROUND 36). Kedua sisi enforcement hidup: schema check
/// (bawah) dan `CacheManifest::valid()` membandingkan `compiler_version`.
///
/// FASE 3 (peringatan S1): karena fingerprint binary tak lagi auto-wipe,
/// disiplin bump menjadi SATU-SATUNYA pelindung saat semantik berubah tanpa
/// perubahan format stage. Lupa bump + rebuild = stale-cache senyap. Bila
/// yang berubah hanya format satu stage, naikkan
/// [`cache::stage_schema_version`] stage itu (evict selektif = Fase 3b).
///
/// **Fingerprint otomatis** (`binary_fingerprint`): SEBELUM Fase 3, revisi
/// pipeline efektif menyertakan fingerprint binary (size + mtime
/// `current_exe`) — setiap rebuild cargo me-wipe seluruh database, tepat saat
/// developer paling butuh incremental (Kritik C).
///
/// FASE 3: fingerprint DITURUNKAN menjadi statistik/peringatan, bukan gerbang
/// invalidasi. Gerbang utama kini: `COMPILER_VERSION` (bump `-p<N>` manual
/// saat semantik berubah) + `SCHEMA_VERSION` + versi skema PER STAGE
/// (`stage_schemas` di manifest). Rebuild binary tanpa perubahan format =
/// cache tetap hidup. Fingerprint tercatat di `registry.json` (statistik) dan
/// hanya memicu peringatan debug bila berubah.
pub const COMPILER_VERSION: &str = concat!("Mivon ", env!("CARGO_PKG_VERSION"), "-p2");

/// Fingerprint identitas binary yang sedang berjalan: xxh64 atas ukuran +
/// mtime (nanodetik) file executable. Binary yang sama → fingerprint sama
/// (incremental lintas run tetap hidup); rebuild cargo apa pun yang me-relink
/// binary → mtime berubah → fingerprint baru → seluruh state/objects/cache
/// project dengan pid lama diabaikan (tidak pernah di-reuse antar binary).
/// Gagal stat exe (eksotis) → 0 (fallback: hanya `COMPILER_VERSION`).
pub fn binary_fingerprint() -> u64 {
    use std::sync::OnceLock;
    static FP: OnceLock<u64> = OnceLock::new();
    *FP.get_or_init(|| {
        use crate::cache::checksum::compute_checksum;
        let Ok(exe) = std::env::current_exe() else {
            return 0;
        };
        let Ok(meta) = std::fs::metadata(&exe) else {
            return 0;
        };
        let mut h = Vec::with_capacity(24);
        h.extend_from_slice(&meta.len().to_le_bytes());
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        h.extend_from_slice(&mtime.to_le_bytes());
        h.extend_from_slice(exe.to_string_lossy().as_bytes());
        compute_checksum(&h)
    })
}

/// Revisi pipeline **efektif** LEGACY (sebelum Fase 3): `COMPILER_VERSION` +
/// fingerprint binary. Dipertahankan untuk kompatibilitas baca manifest lama
/// (lihat [`compiler_base_matches`]) + statistik/debug. BUKAN lagi gerbang
/// invalidasi — jangan dipakai untuk pid/check baru.
pub fn pipeline_revision() -> String {
    format!("{}-{:016x}", COMPILER_VERSION, binary_fingerprint())
}

/// Apakah `compiler_version` tersimpan kompatibel dengan binary ini (Fase 3).
///
/// Menerima format baru (`COMPILER_VERSION` telanjang) maupun legacy persis
/// (`COMPILER_VERSION` + `-` + 16 hex fingerprint dari sebelum Fase 3).
/// Beda base (bump `-p<N>`/versi) atau suffix bukan-hex → tolak.
pub fn compiler_base_matches(stored: &str) -> bool {
    if stored == COMPILER_VERSION {
        return true;
    }
    match stored.strip_prefix(&format!("{}-", COMPILER_VERSION)) {
        Some(fp) => fp.len() == 16 && fp.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

/// Versi skema per stage saat ini (Fase 3: fondasi invalidasi per-stage).
/// Disimpan di manifest tiap save; dibandingkan tiap open.
pub fn current_stage_schemas() -> Vec<(String, u64)> {
    cache::CacheCategory::ALL
        .iter()
        .map(|c| {
            (
                c.name().to_string(),
                cache::stage_schema_version(*c),
            )
        })
        .collect()
}

/// Versi skema database (Kritik 3 db.md). Naikkan bila layout/format
/// persistensi berubah (field struct, key store, semantik). Database lama
/// dengan schema version berbeda dianggap tidak kompatibel → dibangun ulang.
///
/// Fase 3: 4 → 5 (manifest + `stage_schemas` per-stage; pid + compiler check
/// lepas dari fingerprint binary).
pub const SCHEMA_VERSION: u64 = 5;

/// Nama file database.
pub const DB_METADATA: &str = "metadata.mdb";
pub const DB_GRAPH: &str = "graph.mdb";
pub const DB_AST: &str = "ast.mdb";
pub const DB_PREPROC: &str = "preproc.mdb";
pub const DB_VERIFY: &str = "verify.mdb";
pub const DB_DIAG: &str = "diagnostics.mdb";
pub const DB_SYMBOL: &str = "symbol.mdb";
pub const DB_TYPE: &str = "types.mdb";
pub const DB_STATS: &str = "stats.mdb";
pub const DB_JOURNAL: &str = "journal.mdb";

/// Direktori di dalam database root (layout Git-style, Opsi B db.md).
pub const DIR_STATE: &str = "state";
pub const DIR_OBJECTS: &str = "objects";
pub const DIR_LOCKS: &str = "locks";
/// Direktori artefak precompiled (mirip VCS AN.DB/).
pub const DIR_PRECOMPILED: &str = "precompiled";
/// File penanda versi skema di database root.
pub const FILE_VERSION: &str = "VERSION";
/// Registri project (pid → info) di database root.
pub const FILE_REGISTRY: &str = "registry.json";
/// Extensi file objek (payload CAS).
pub const OBJ_AST: &str = "ast";
pub const OBJ_PREPROC: &str = "preproc";

/// Key singleton untuk store berisi satu objek besar (graph/symbol).
const KEY_SINGLETON: u64 = 0x0000_0000_0000_0001;

/// Entry cache preprocessed (combined source) per file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreprocEntry {
    pub content_hash: u64,
    pub combined: String,
    pub timescale: Option<(String, String)>,
    /// Segmen `` `timescale `` per baris output preprocessed (F84,
    /// LRM §19.8): `(baris_output_1based, (unit, precision))`.
    ///
    /// Wajib disimpan: directive `` `timescale `` di-consume saat preprocess,
    /// jadi dari `combined` saja info per-module tak bisa diturunkan lagi.
    /// Tanpa field ini, run hangat (AST/preproc cache-hit) kehilangan satuan
    /// per-module dan delay kembali diskalakan dengan satuan global.
    ///
    /// `#[serde(default)]` supaya entry lama (tanpa field) tetap bisa decode —
    /// nilainya kosong = jatuh ke satuan global, sama seperti perilaku
    /// sebelum F84, bukan crash.
    #[serde(default)]
    pub timescale_segments: Vec<(usize, (String, String))>,
}

/// Info project di `registry.json` (identifikasi pid secara manusiawi).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Root direktori project (path absolut).
    pub root: String,
    /// Jumlah source yang terdaftar.
    pub source_count: usize,
    /// Pratinjau source (maks 8) untuk identifikasi cepat.
    pub sources: Vec<String>,
    /// Waktu pertama kali dibangun (unix ns).
    pub created_ns: u64,
    /// Waktu terakhir dibangun (unix ns).
    pub last_built_ns: u64,
    /// Fase 3: revisi compiler saat build terakhir (statistik, bukan gerbang).
    #[serde(default)]
    pub compiler_version: String,
    /// Fase 3: fingerprint binary saat build terakhir (statistik/peringatan).
    #[serde(default)]
    pub binary_fingerprint: u64,
}

/// Statistik MICD setelah `save()`.
#[derive(Debug, Clone, Default)]
pub struct MicdStats {
    /// Jumlah file terdaftar di database.
    pub files: usize,
    /// File yang AST-nya di-restore (parse di-skip).
    pub restored_designs: usize,
    /// File yang berubah pada build ini.
    pub changed_files: usize,
    /// Hit verification cache.
    pub verify_hits: usize,
    /// Miss verification cache.
    pub verify_misses: usize,
    /// ID snapshot terakhir.
    pub snapshot_id: u64,
    /// Bytes yang ditulis ke disk.
    pub bytes_written: u64,
}

/// MICD object database.
pub struct MicdDatabase {
    /// Root database (`.mivon/database`, override `MIVON_MICD_DIR`).
    /// Payload → `objects/<pid>/`, index → `state/<pid>/`.
    pub root: PathBuf,
    /// ProjectID yang sedang dibuka.
    pub pid: String,
    /// Info project untuk `registry.json`.
    pub registry: ProjectInfo,
    pub flags_hash: u64,
    pub compiler_version: String,
    pub created_ns: u64,
    /// Versi skema yang dimuat dari metadata.mdb (0 bila fresh/tidak kompatibel).
    pub schema_version: u64,
    /// Per-file metadata (kunci = path).
    pub files: HashMap<PathBuf, FileMeta>,
    /// Dependency graph file-level.
    pub graph: FileGraph,
    /// Verification cache (kunci = content hash).
    pub verify: HashMap<u64, VerifyResult>,
    /// Indeks AST hash → content hash (Kritik 1 db.md). Untuk reuse
    /// verification saat content hash berubah tapi AST identik (mis. komentar
    /// berubah).
    pub verify_ast_index: HashMap<u64, u64>,
    /// Indeks semantic hash → content hash (Kritik E). Menggantikan
    /// O(n) linear scan di reuse_verify() dengan O(1) lookup.
    pub verify_semantic_index: HashMap<u64, u64>,
    /// Diagnostic per file.
    pub diags: HashMap<PathBuf, FileDiags>,
    /// Index simbol.
    pub symbols: SymbolIndex,
    /// AST cache: path → (content_hash, bytes bincode).
    /// `Arc<[u8]>` untuk zero-copy clone — get_ast() mengembalikan Arc clone
    /// (increment refcount) bukan memcpy penuh. Kritik B: cache hit 500MB
    /// sebelumnya = memcpy 500MB.
    pub ast_cache: HashMap<PathBuf, (u64, Arc<[u8]>)>,
    /// Preprocess cache: path → combined source.
    pub preproc_cache: HashMap<PathBuf, PreprocEntry>,
    /// Index tipe/signature: module → signature hash.
    pub type_index: HashMap<String, u64>,
    /// Statistics database (Kritik 14 db.md).
    pub stats_db: StatsDb,
    /// Waktu akses terakhir tiap entry cache (LRU/TTL — Kritik 6 db.md).
    /// Tidak diserialisasi: dibangun ulang dari mtime/compiled_at saat load.
    pub ast_accessed: HashMap<PathBuf, u64>,
    pub preproc_accessed: HashMap<PathBuf, u64>,
    pub verify_accessed: HashMap<u64, u64>,
    /// Total bytes cache AST (untuk budget GC).
    pub ast_bytes: u64,
    /// Total bytes preprocessed source.
    pub preproc_bytes: u64,
    /// Jalankan GC otomatis saat `save()` (Kritik 6 db.md).
    pub gc_on_save: bool,
    /// Ada perubahan belum disimpan.
    pub dirty: bool,
    /// ast.mdb perlu ditulis ulang (ada AST baru/berubah).
    pub dirty_ast: bool,
    /// preproc.mdb perlu ditulis ulang.
    pub dirty_preproc: bool,
    /// verify.mdb perlu ditulis ulang.
    pub dirty_verify: bool,
    /// symbol.mdb perlu ditulis ulang.
    pub dirty_symbol: bool,
    /// types.mdb perlu ditulis ulang.
    pub dirty_type: bool,
    /// graph.mdb perlu ditulis ulang.
    pub dirty_graph: bool,
    /// stats.mdb perlu ditulis ulang.
    pub dirty_stats: bool,
    /// diagnostics.mdb perlu ditulis ulang (terpisah dari dirty metadata).
    /// Kritik B: set_diags sebelumnya menandai dirty=true yang memicu tulis
    /// ulang SELURUH metadata.mdb — padahal hanya diagnostic yang berubah.
    pub dirty_diag: bool,
    /// Snapshot yang tersedia.
    pub snapshots: Vec<u64>,
    /// Jumlah restore AST pada sesi ini.
    pub restored: usize,
    /// Jumlah file berubah pada sesi ini (kumulatif sejak open).
    pub changed: usize,
    /// Jumlah file berubah pada snapshot terakhir (dedup snapshot).
    pub last_snapshotted_changed: usize,
    /// Lapisan cache pipeline per kategori (`cache/<pid>/`, db.md 1141-1605).
    /// Best-effort: `None` bila database root tidak bisa menulis.
    pub cache_layer: Option<CacheLayer>,
    /// Database artefak precompiled per module (mirip VCS AN.DB/).
    /// Menyimpan hasil analisis per module agar tool downstream bisa
    /// baca tanpa compile ulang.
    pub precompiled_db: Option<precompiled::PrecompiledDb>,
}

// ─── Helper I/O: mengurangi duplikasi open/save ───

/// Baca satu objek singleton dari file MDB. Mengembalikan `None` bila file
/// tidak ada, corrupt, atau key tidak ditemukan.
///
/// Fase 0 (Kritik B): pakai `get_slice` zero-copy (tanpa memcpy) untuk store
/// tanpa kompresi; fallback ke `get` (copy + decompress) untuk store LZ4.
fn read_mdb_singleton<T: serde::de::DeserializeOwned>(
    dir: &Path,
    filename: &str,
) -> Option<T> {
    let r = MdbReader::open(&dir.join(filename)).ok()?;
    if let Some(slice) = r.get_slice(KEY_SINGLETON) {
        if let Ok(v) = bincode::deserialize(slice) {
            return Some(v);
        }
    }
    let b = r.get(KEY_SINGLETON)?;
    bincode::deserialize(&b).ok()
}

/// Baca semua objek (kecuali singleton) dari file MDB.
///
/// Fase 0 (Kritik B): sama seperti singleton — `get_slice` dulu, fallback
/// `get` untuk entry terkompresi.
fn read_mdb_entries<T: serde::de::DeserializeOwned>(
    dir: &Path,
    filename: &str,
) -> Vec<T> {
    let Ok(r) = MdbReader::open(&dir.join(filename)) else {
        return Vec::new();
    };
    r.keys()
        .iter()
        .filter(|(k, _)| *k != KEY_SINGLETON)
        .filter_map(|(k, _)| {
            if let Some(slice) = r.get_slice(*k) {
                if let Ok(v) = bincode::deserialize(slice) {
                    return Some(v);
                }
            }
            r.get(*k).and_then(|b| bincode::deserialize(&b).ok())
        })
        .collect()
}

/// Serialisasi satu objek singleton ke bytes MDB (belum ditulis ke disk).
fn serialize_mdb_singleton<T: serde::Serialize>(
    obj: &T,
    kind: u8,
) -> io::Result<Vec<u8>> {
    let mut w = MdbWriter::new();
    w.put(
        KEY_SINGLETON,
        kind,
        bincode::serialize(obj).map_err(io::Error::other)?,
    );
    w.serialize().map_err(io::Error::other)
}

/// Serialisasi satu objek singleton ke bytes MDB dengan kompresi.
fn serialize_mdb_singleton_compressed<T: serde::Serialize>(
    obj: &T,
    kind: u8,
    compression: format::Compression,
) -> io::Result<Vec<u8>> {
    let mut w = MdbWriter::with_compression(compression);
    w.put(
        KEY_SINGLETON,
        kind,
        bincode::serialize(obj).map_err(io::Error::other)?,
    );
    w.serialize().map_err(io::Error::other)
}

impl MicdDatabase {
    /// Lokasi alternatif saat root default tidak bisa dipakai: `.mivon` adalah
    /// FILE (project file untuk `--filelist`/`-f`), bukan folder → MICD harus
    /// memakai folder terpisah agar tidak konflik.
    fn fallback_root() -> PathBuf {
        PathBuf::from(".mivon_db").join("database")
    }

    /// ProjectID (Kritik arsitektur db.md: database harus scoped per project).
    ///
    /// Hash deterministik atas seluruh konfigurasi yang membedakan satu
    /// "project kompilasi" dari yang lain: root direktori, daftar source,
    /// include dirs, define macro, compiler version, dan language standard.
    /// Hasilnya adalah direktori `state/<ProjectID>/` di dalam database.
    ///
    /// Dua project yang berbeda (mis. OpenTitan vs `test/counter.sv`) selalu
    /// menghasilkan ProjectID berbeda → tidak pernah berbagi object/symbol/
    /// graph/verify store. Ini menghilangkan akar bug "file project lain
    /// menempel" yang muncul saat semua project memakai satu database global.
    pub fn project_id(
        root: &Path,
        sources: &[PathBuf],
        incdirs: &[PathBuf],
        defines: &[(String, String)],
    ) -> String {
        use crate::cache::checksum::{checksum_fold, compute_checksum};
        let mut h: Vec<u64> = Vec::with_capacity(sources.len() + incdirs.len() + defines.len() + 4);
        h.push(compute_checksum(root.to_string_lossy().as_bytes()));
        // Fase 3 (Kritik C): pid stabil lintas rebuild — basis COMPILER_VERSION
        // saja, TANPA fingerprint binary (mtime exe). Bump -p<N> manual saat
        // semantik berubah tetap mengganti pid.
        h.push(compute_checksum(COMPILER_VERSION.as_bytes()));
        h.push(compute_checksum(b"systemverilog-2012"));
        for s in sources {
            h.push(path_hash(s));
        }
        for d in incdirs {
            h.push(path_hash(d));
        }
        for (k, v) in defines {
            h.push(compute_checksum(k.as_bytes()));
            h.push(compute_checksum(v.as_bytes()));
        }
        format!("{:016x}", checksum_fold(&h))
    }

    /// Direktori index (state mutable) sebuah project di database root.
    pub fn project_root(db_root: &Path, pid: &str) -> PathBuf {
        state_dir(db_root, pid)
    }

    /// Direktori payload (objek CAS) sebuah project di database root.
    pub fn objects_root(db_root: &Path, pid: &str) -> PathBuf {
        objects_dir(db_root, pid)
    }

    /// Buka database project secara eksplisit berdasarkan ProjectID.
    pub fn open_project(db_root: &Path, pid: &str) -> MicdDatabase {
        Self::open_with_pid(db_root, pid)
    }

    /// Buka database project + catat konteks project di `registry.json`
    /// (root + pratinjau source) agar pid terbaca manusia.
    pub fn open_project_with_context(
        db_root: &Path,
        pid: &str,
        proot: &Path,
        sources: &[PathBuf],
    ) -> MicdDatabase {
        let mut db = Self::open_with_pid(db_root, pid);
        db.registry.root = proot.to_string_lossy().to_string();
        db.registry.source_count = sources.len();
        db.registry.sources = sources
            .iter()
            .take(8)
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        db
    }

    /// Pilih root database yang BISA dibuat di lingkungan kerja. Prioritas:
    /// 1. env `MIVON_MICD_DIR` (override manual, selalu dipakai).
    /// 2. `.mivon/database` bila `.mivon` kosong/berupa folder.
    /// 3. `.mivon_db/database` bila `.mivon` adalah FILE (project `-f`).
    pub fn default_root() -> PathBuf {
        if let Ok(p) = std::env::var("MIVON_MICD_DIR") {
            return PathBuf::from(p);
        }
        let mivon_path = PathBuf::from(".mivon");
        match std::fs::metadata(&mivon_path) {
            Ok(m) if m.is_file() => Self::fallback_root(),
            _ => mivon_path.join("database"),
        }
    }

    /// Direktori state (index mutable) untuk project yang sedang dibuka.
    pub fn state_dir(&self) -> PathBuf {
        state_dir(&self.root, &self.pid)
    }

    /// Direktori objek (payload CAS) untuk project yang sedang dibuka.
    pub fn objects_dir(&self) -> PathBuf {
        objects_dir(&self.root, &self.pid)
    }

    /// Path file lock writer untuk project ini.
    pub fn lock_path(&self) -> PathBuf {
        lock_path(&self.root, &self.pid)
    }

    /// Path objek CAS: `objects/<pid>/<hex-hash>.<ext>`.
    pub fn object_path(&self, content_hash: u64, ext: &str) -> PathBuf {
        self.objects_dir()
            .join(format!("{:016x}.{}", content_hash, ext))
    }

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

        let mut db = MicdDatabase {
            root: root.clone(),
            pid: pid.to_string(),
            registry: ProjectInfo::default(),
            flags_hash: 0,
            compiler_version: COMPILER_VERSION.to_string(),
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

        // Lapisan cache pipeline per kategori (db.md cache/, baris 1141-1605):
        // 21 store seragam di `cache/<pid>/`. Dibuka sekali saat open; `None`
        // bila database root tak bisa ditulis (best-effort — non-kritis).
        db.cache_layer = CacheLayer::open(&root, &db.pid, db.flags_hash).ok();

        // Database artefak precompiled (VCS AN.DB / Questa _info analog).
        // Menyimpan hasil analisis per module agar tool downstream bisa baca
        // tanpa compile ulang.
        let precompiled_root = db.root.join(DIR_PRECOMPILED).join(&db.pid);
        db.precompiled_db = Some(precompiled::PrecompiledDb::open(&precompiled_root));

        // metadata.mdb — pintu schema (Kritik 3 db.md). Bila schema version
        // tidak cocok, SELURUH store dianggap tidak kompatibel → bangun ulang
        // dari kosong (AST lama diabaikan, tidak akan di-reuse).
        let mut schema_ok = false;
        if let Ok(r) = MdbReader::open(&st.join(DB_METADATA)) {
            if let Some(manifest) = r
                .get(KEY_SINGLETON)
                .and_then(|b| bincode::deserialize::<MetadataManifest>(&b).ok())
            {
                // Fase 3 (Kritik C): gerbang = base compiler + schema + stage.
                // Fingerprint binary TIDAK lagi meng-invalidate (hanya
                // statistik di registry). Manifest legacy (base + suffix hex)
                // tetap diterima via compiler_base_matches. Stage wajib sama
                // persis (strict — tanpa cabang kosong).
                if manifest.schema_version == SCHEMA_VERSION
                    && compiler_base_matches(&manifest.compiler_version)
                    && manifest.stage_schemas == current_stage_schemas()
                {
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
            db.schema_version = SCHEMA_VERSION;
            return db;
        }

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
                        db.ast_cache.insert(p.clone(), (meta.content_hash, Arc::from(bytes)));
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
    pub fn get_preprocessed_ref(
        &self,
        path: &Path,
        content_hash: u64,
    ) -> Option<&PreprocEntry> {
        let e = self.preproc_cache.get(path)?;
        if e.content_hash == content_hash {
            Some(e)
        } else {
            None
        }
    }

    /// Fase 0 (Kritik B): referensi `&str` ke combined source (tanpa clone).
    /// Dipakai jalur panas restore + debug check.
    pub fn get_preprocessed_combined(
        &self,
        path: &Path,
        content_hash: u64,
    ) -> Option<&str> {
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

    /// Simpan `IrDesign` hasil elaborasi LENGKAP ke cache pipeline (db.md
    /// "5. elaborate/", key `ir:<top>`). Dipakai warm run untuk melewati
    /// elaborator sepenuhnya. Best-effort — gagal menyimpan IR tidak
    /// menggagalkan save utama.
    pub fn store_elaborate_ir(&mut self, ir: &mivon_ir::IrDesign) {
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
    }

    /// Coba restore `IrDesign` hasil elaborasi dari cache pipeline (db.md
    /// "5. elaborate/"). `None` bila tidak ada entry / corrupt / top berbeda.
    pub fn restore_elaborate_ir(&mut self, top: &str) -> Option<mivon_ir::IrDesign> {
        use crate::micd::cache::CacheCategory;
        let layer = self.cache_layer.as_mut()?;
        let key = format!("ir:v{}:{}", crate::micd::ast::IR_FORMAT_VERSION, top);
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
                compiler_version: COMPILER_VERSION.to_string(),
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

// Kebalikan path_hash tidak bisa; key numerik tidak perlu dipetakan kembali
// karena object store di-load dengan iterasi metadata store (path → hash →
// objek).

// ─── Layout database (Opsi B db.md) ───

/// Direktori index (state mutable) sebuah project.
pub fn state_dir(db_root: &Path, pid: &str) -> PathBuf {
    db_root.join(DIR_STATE).join(pid)
}

/// Direktori payload (objek CAS immutable) sebuah project.
pub fn objects_dir(db_root: &Path, pid: &str) -> PathBuf {
    db_root.join(DIR_OBJECTS).join(pid)
}

/// Path file lock writer untuk sebuah project (`locks/<pid>.lock`).
pub fn lock_path(db_root: &Path, pid: &str) -> PathBuf {
    db_root.join(DIR_LOCKS).join(format!("{}.lock", pid))
}

/// Sweep objek CAS: buang file `<hash>.<ext>` yang tidak lagi dirujuk
/// (live set), plus sisa `.tmp` dari crash. Content-addressed menjamin file
/// dengan konten sama tetap satu — hanya yang tak terpakai yang dihapus.
pub fn sweep_objects(
    objs: &Path,
    ext: &str,
    live: &std::collections::HashSet<u64>,
) -> io::Result<()> {
    let Ok(entries) = std::fs::read_dir(objs) else {
        return Ok(());
    };
    let suffix = format!(".{}", ext);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        if let Some(hx) = name.strip_suffix(&suffix) {
            if let Ok(h) = u64::from_str_radix(hx, 16) {
                if !live.contains(&h) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    Ok(())
}

/// Baca `registry.json` (pid → info project). Corrupt/hilang → kosong.
fn read_registry(db_root: &Path) -> HashMap<String, ProjectInfo> {
    let path = db_root.join(FILE_REGISTRY);
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Tulis `registry.json` atomik (temp + rename).
fn write_registry(db_root: &Path, map: &HashMap<String, ProjectInfo>) {
    let path = db_root.join(FILE_REGISTRY);
    if let Ok(data) = serde_json::to_vec_pretty(map) {
        let _ = format::write_tmp(&path, &data);
        let _ = format::commit_tmp(&path);
    }
}

/// Catat pid di registri (dipanggil saat save — last_built di-update tiap run).
/// Fase 3: fingerprint + revisi compiler dicatat sebagai statistik (bukan
/// gerbang invalidasi). Beda fingerprint vs build lalu → peringatan debug.
fn register_project(db: &MicdDatabase) {
    let mut map = read_registry(&db.root);
    let now = now_ns();
    let fp = binary_fingerprint();
    let entry = map.entry(db.pid.clone()).or_default();
    if entry.root.is_empty() {
        entry.root = db.registry.root.clone();
        entry.source_count = db.registry.source_count;
        entry.sources = db.registry.sources.clone();
    }
    if entry.created_ns == 0 {
        entry.created_ns = now;
    }
    if entry.binary_fingerprint != 0
        && entry.binary_fingerprint != fp
        && std::env::var("MIVON_DEBUG_MICD").is_ok()
    {
        eprintln!(
            "[MICD] binary berubah sejak build lalu (fp {:016x} → {:016x}) — cache tetap dipakai (Fase 3: fingerprint hanya statistik)",
            entry.binary_fingerprint, fp
        );
    }
    entry.binary_fingerprint = fp;
    entry.compiler_version = COMPILER_VERSION.to_string();
    entry.last_built_ns = now;
    write_registry(&db.root, &map);
}

/// Sapu store mati di `state/<pid>/` (Fase 2, perluasan terakhir legacy).
///
/// `ast.mdb`/`preproc.mdb` gaya lama tidak pernah dibaca lagi sejak payload
/// CAS (`objects/<pid>/*.ast|*.preproc`) — `save()` tidak menulisnya, `open()`
/// tidak membacanya. Sisa file usang hanya bobot disk; hapus deterministik.
/// Idempoten (file tak ada → no-op).
fn sweep_dead_stores(st: &Path) {
    for name in [DB_AST, DB_PREPROC] {
        let p = st.join(name);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
        let _ = std::fs::remove_file(p.with_extension("mdb.tmp"));
    }
}

/// Migrasi layout lama `projects/<pid>/` → `state/<pid>/` + `objects/<pid>/`.
///
/// FROZEN (Fase 2): migrasi ini dibekukan — tidak akan diubah lagi. Layout
/// lama sudah tidak diproduksi sejak CAS; fungsi ini hanya sapu sisa. Test
/// `test_fase2_sweep_store_mati_dan_migrate_idempoten` mengunci perilaku
/// (panggil 2× = sama).
///
/// Layout lama menumpuk semua `.mdb` di satu direktori per project. Migrasi
/// berjalan sekali (idempoten) untuk SEMUA project yang pernah ada: state
/// store dipindah, `ast.mdb`/`preproc.mdb` dipecah menjadi objek CAS per
/// konten hash. Direktori `projects/` dihapus setelah bersih.
fn migrate_legacy(db_root: &Path, _pid: &str) {
    let projects = db_root.join("projects");
    if projects.exists() {
        if let Ok(entries) = std::fs::read_dir(&projects) {
            let pids: Vec<String> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            for p in &pids {
                migrate_legacy_one(db_root, p);
            }
            // Subdir yang masih tersisa hanya yang kosong (tanpa metadata) —
            // buang agar `projects/` benar-benar bersih.
            for p in pids {
                let _ = std::fs::remove_dir(projects.join(p));
            }
        }
        let _ = std::fs::remove_dir(&projects);
    }
}

fn migrate_legacy_one(db_root: &Path, pid: &str) {
    let legacy = db_root.join("projects").join(pid);
    let st = state_dir(db_root, pid);
    if !legacy.join(DB_METADATA).exists() || st.join(DB_METADATA).exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&st);

    // State store dipindah langsung.
    for name in [
        DB_METADATA,
        DB_GRAPH,
        DB_VERIFY,
        DB_DIAG,
        DB_SYMBOL,
        DB_TYPE,
        DB_STATS,
        DB_JOURNAL,
    ] {
        let src = legacy.join(name);
        if src.exists() {
            let _ = std::fs::rename(src, st.join(name));
        }
    }
    // Snapshot (bila ada).
    let snap_src = legacy.join("snapshots");
    if snap_src.exists() {
        let _ = std::fs::rename(snap_src, st.join("snapshots"));
    }

    // ast.mdb → objek per konten hash.
    let objs = objects_dir(db_root, pid);
    let _ = std::fs::create_dir_all(&objs);
    if let Ok(r) = MdbReader::open(&legacy.join(DB_AST)) {
        for (key, _kind) in r.keys() {
            if let Some(b) = r.get(key) {
                if let Ok((_path_str, hash, ver, bytes)) =
                    bincode::deserialize::<(String, u64, u64, Vec<u8>)>(&b)
                {
                    if ver == AST_FORMAT_VERSION {
                        let obj = objs.join(format!("{:016x}.{}", hash, OBJ_AST));
                        if !obj.exists() {
                            if let Ok(payload) = bincode::serialize(&(AST_FORMAT_VERSION, bytes)) {
                                let _ = format::write_tmp_unflushed(&obj, &payload);
                                let _ = format::commit_tmp(&obj);
                            }
                        }
                    }
                }
            }
        }
    }
    // preproc.mdb → objek per konten hash.
    if let Ok(r) = MdbReader::open(&legacy.join(DB_PREPROC)) {
        for (key, _kind) in r.keys() {
            if let Some(b) = r.get(key) {
                if let Ok((_path_str, hash, p)) =
                    bincode::deserialize::<(String, u64, PreprocEntry)>(&b)
                {
                    if p.content_hash == hash {
                        let obj = objs.join(format!("{:016x}.{}", hash, OBJ_PREPROC));
                        if !obj.exists() {
                            if let Ok(payload) = bincode::serialize(&p) {
                                let _ = format::write_tmp_unflushed(&obj, &payload);
                                let _ = format::commit_tmp(&obj);
                            }
                        }
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_dir_all(&legacy);
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mivon_micd_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// State dir untuk `MicdDatabase::open(&root)` (pid "default").
    fn default_state(root: &Path) -> PathBuf {
        state_dir(root, "default")
    }

    #[test]
    fn test_open_empty_db() {
        let root = test_root("empty");
        let db = MicdDatabase::open(&root);
        assert!(db.files.is_empty());
        assert!(db.graph.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Lapisan cache pipeline (db.md cache/, baris 1141-1605) ──

    #[test]
    fn test_open_creates_cache_layer_and_saves() {
        let root = test_root("cache_layer");
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            assert!(db.cache_layer.is_some(), "lapisan cache dibuka saat open");
            if let Some(layer) = db.cache_layer.as_mut() {
                layer
                    .put(CacheCategory::Parser, "a.sv", b"parse-summary")
                    .unwrap();
            }
            db.record_file(path.clone(), 1, vec![], FileStatus::New, 0, 3, vec![]);
            db.save().unwrap();
        }
        {
            let mut db = MicdDatabase::open(&root);
            assert!(db.cache_layer.is_some());
            let got = db
                .cache_layer
                .as_mut()
                .and_then(|l| l.get(CacheCategory::Parser, "a.sv"));
            assert_eq!(
                got,
                Some(b"parse-summary".to_vec()),
                "cache persist lintas open"
            );
            // Struktur `cache/<pid>/<category>/` sesuai db.md.
            let pid = db.pid.clone();
            assert!(db.root.join("cache").join(&pid).join("parser").is_dir());
            // Semua 21 kategori terstruktur seragam.
            assert_eq!(
                db.cache_layer.as_ref().unwrap().stats().stores,
                CacheCategory::ALL.len()
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_clear_removes_cache_layer() {
        let root = test_root("clear_cache");
        {
            let mut db = MicdDatabase::open(&root);
            db.cache_layer
                .as_mut()
                .unwrap()
                .put(CacheCategory::Type, "m", b"sig")
                .unwrap();
            db.save().unwrap();
            db.clear().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            assert!(!db
                .cache_layer
                .as_ref()
                .unwrap()
                .contains(CacheCategory::Type, "m"));
            // clear() membuka ulang lapisan kosong (fungsi tetap), isi bersih.
            assert_eq!(db.cache_layer.as_ref().unwrap().stats().total_entries, 0);
            let objs = db
                .root
                .join("cache")
                .join(&db.pid)
                .join("type")
                .join("objects");
            assert!(
                objs.read_dir()
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(false),
                "objek cache dibersihkan"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── ProjectID: database scoped per project ──

    #[test]
    fn test_project_id_deterministic_and_distinct() {
        let root = Path::new("/proj");
        let src_a = vec![PathBuf::from("top.sv"), PathBuf::from("uart.sv")];
        let src_b = vec![PathBuf::from("top.sv")];
        let defs = vec![("WIDTH".to_string(), "8".to_string())];
        // Deterministik: input sama → ProjectID sama.
        let p1 = MicdDatabase::project_id(root, &src_a, &[], &defs);
        let p2 = MicdDatabase::project_id(root, &src_a, &[], &defs);
        assert_eq!(p1, p2, "input sama → ProjectID sama");
        // Source set berbeda → ProjectID berbeda.
        let p3 = MicdDatabase::project_id(root, &src_b, &[], &defs);
        assert_ne!(p1, p3, "source berbeda → ProjectID berbeda");
        // Defines berbeda → ProjectID berbeda.
        let p4 = MicdDatabase::project_id(root, &src_a, &[], &[]);
        assert_ne!(p1, p4, "defines berbeda → ProjectID berbeda");
        // Root berbeda → ProjectID berbeda.
        let p5 = MicdDatabase::project_id(Path::new("/other"), &src_a, &[], &defs);
        assert_ne!(p1, p5, "root berbeda → ProjectID berbeda");
        assert_eq!(p1.len(), 16, "format hex 16 digit");
    }

    #[test]
    fn test_open_project_isolates_stores() {
        let root = test_root("proj_iso");
        let base = root.join("db");
        let pid_a =
            MicdDatabase::project_id(Path::new("/proj_a"), &[PathBuf::from("a.sv")], &[], &[]);
        let pid_b =
            MicdDatabase::project_id(Path::new("/proj_b"), &[PathBuf::from("a.sv")], &[], &[]);
        assert_ne!(pid_a, pid_b);

        // Project A: tulis satu file.
        {
            let mut db = MicdDatabase::open_project(&base, &pid_a);
            db.record_file(
                PathBuf::from("a.sv"),
                1,
                vec![],
                FileStatus::New,
                0,
                5,
                vec![],
            );
            db.cache_ast(PathBuf::from("a.sv"), 1, vec![1, 2, 3]);
            db.save().unwrap();
        }
        // Project A terbuka kembali → file ada.
        {
            let db = MicdDatabase::open_project(&base, &pid_a);
            assert_eq!(db.files.len(), 1);
            assert!(db.has_valid_ast(&PathBuf::from("a.sv"), 1));
        }
        // Project B → kosong (terisolasi, tidak berbagi store).
        {
            let db = MicdDatabase::open_project(&base, &pid_b);
            assert!(db.files.is_empty(), "project lain tidak boleh berbagi data");
            assert!(db.ast_cache.is_empty());
        }
        // Root DB menyimpan data terpisah per project: index di
        // `state/<pid>/`, payload CAS di `objects/<pid>/`. Project A dan B
        // TIDAK berbagi direktori. metadata.mdb A ada (baru saja di-save);
        // project B belum pernah di-save sehingga file-nya belum dibuat —
        // yang penting adalah isolasi path (bukan keberadaan file).
        let st_a = MicdDatabase::project_root(&base, &pid_a);
        assert!(st_a.join("metadata.mdb").exists());
        // Objek AST content-addressed di objects/<pid_a>/.
        assert!(MicdDatabase::objects_root(&base, &pid_a)
            .join(format!("{:016x}.{}", 1, OBJ_AST))
            .exists());
        assert_ne!(
            st_a,
            MicdDatabase::project_root(&base, &pid_b),
            "project A dan B harus punya store terpisah"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_layout_split_state_and_objects() {
        // Layout Opsi B db.md: VERSION + registry.json + locks/ di root;
        // payload immutable di objects/, index mutable di state/.
        let root = test_root("layout");
        let pid = MicdDatabase::project_id(Path::new("/proj"), &[PathBuf::from("a.sv")], &[], &[]);
        {
            let mut db = MicdDatabase::open_project(&root, &pid);
            db.record_file(
                PathBuf::from("a.sv"),
                42,
                vec![],
                FileStatus::New,
                0,
                5,
                vec![],
            );
            db.cache_ast(PathBuf::from("a.sv"), 42, vec![9, 9, 9]);
            db.save().unwrap();
        }
        // Root: VERSION + registry.
        assert_eq!(
            std::fs::read_to_string(root.join(FILE_VERSION))
                .unwrap()
                .trim(),
            SCHEMA_VERSION.to_string(),
            "VERSION menulis skema terkini"
        );
        assert!(root.join(FILE_REGISTRY).exists(), "registry.json dibuat");
        // Index di state/, payload di objects/.
        let st = state_dir(&root, &pid);
        let objs = objects_dir(&root, &pid);
        assert!(st.join(DB_METADATA).exists());
        assert!(objs.join(format!("{:016x}.{}", 42, OBJ_AST)).exists());
        assert!(!st.join(DB_AST).exists(), "AST bukan store state lagi");
        // Tidak ada cache/ kosong dan lock di locks/ (bukan di state).
        assert!(!st.join("cache").exists());
        assert!(!st.join("lock").exists());
        // registry.json berisi pid yang baru di-build.
        let map = read_registry(&root);
        assert!(map.contains_key(&pid));
        assert!(map[&pid].last_built_ns > 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_migrate_legacy_projects_layout() {
        // Layout lama `projects/<pid>/*.mdb` di-migrasi ke state/ + objects/
        // saat dibuka dengan layout baru.
        let root = test_root("migrate");
        let pid = MicdDatabase::project_id(Path::new("/proj"), &[PathBuf::from("a.sv")], &[], &[]);
        let legacy = root.join("projects").join(&pid);
        std::fs::create_dir_all(&legacy).unwrap();
        // Tulis store state gaya lama.
        {
            let mut w = MdbWriter::new();
            let manifest = MetadataManifest {
                paths: vec![PathBuf::from("a.sv")],
                flags_hash: 0,
                compiler_version: pipeline_revision(),
                created_ns: 0,
                schema_version: SCHEMA_VERSION,
                stage_schemas: current_stage_schemas(),
            };
            w.put(
                KEY_SINGLETON,
                format::KIND_MANIFEST,
                bincode::serialize(&manifest).unwrap(),
            );
            w.put(
                path_hash(Path::new("a.sv")),
                format::KIND_META,
                bincode::serialize(&FileMeta {
                    path: PathBuf::from("a.sv"),
                    content_hash: 7,
                    mtime_ns: 0,
                    size: 3,
                    status: FileStatus::Unchanged,
                    flags_hash: 0,
                    deps: vec![],
                    include_hashes: vec![],
                    compiled_at_ns: 0,
                    ast_format_version: AST_FORMAT_VERSION,
                })
                .unwrap(),
            );
            w.write_to(&legacy.join(DB_METADATA)).unwrap();
            let mut w2 = MdbWriter::new();
            w2.put(
                1,
                format::KIND_AST,
                bincode::serialize(&(
                    "a.sv".to_string(),
                    7u64,
                    AST_FORMAT_VERSION,
                    vec![1u8, 2, 3],
                ))
                .unwrap(),
            );
            w2.write_to(&legacy.join(DB_AST)).unwrap();
        }
        // Buka → migrasi otomatis.
        let db = MicdDatabase::open_project(&root, &pid);
        assert!(
            db.has_valid_ast(&PathBuf::from("a.sv"), 7),
            "AST termigrasi ke objek CAS"
        );
        assert!(!legacy.exists(), "folder legacy dihapus");
        assert!(state_dir(&root, &pid).join(DB_METADATA).exists());
        assert!(objects_dir(&root, &pid)
            .join(format!("{:016x}.{}", 7, OBJ_AST))
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_save_reload_roundtrip() {
        let root = test_root("roundtrip");
        let path = PathBuf::from("test/counter.sv");
        let hash = 0x1234_5678;
        {
            let mut db = MicdDatabase::open(&root);
            db.flags_hash = 42;
            db.record_file(
                path.clone(),
                hash,
                vec![],
                FileStatus::Unchanged,
                42,
                100,
                vec![],
            );
            db.cache_ast(path.clone(), hash, vec![1, 2, 3, 4]);
            db.cache_preprocessed(
                path.clone(),
                PreprocEntry {
                    content_hash: hash,
                    combined: "`line 1 \"test/counter.sv\"\nmodule c;\nendmodule".into(),
                    timescale: None,
                    timescale_segments: Vec::new(),
                },
            );
            db.set_verify(VerifyResult {
                content_hash: hash,
                ast_hash: 0xABCD,
                semantic_hash: 0x1234,
                ir_hash: 0,
                parse_ok: true,
                elab_ok: true,
                err_count: 0,
                warn_count: 0,
                info_count: 0,
                parse_ms: 5,
                elab_ms: 3,
                result_hash: 7,
                verified_at_ns: 0,
                checks: HashMap::new(),
            });
            db.add_symbol("counter".into(), "module".into(), path.clone());
            db.save().unwrap();
        }
        {
            let mut db = MicdDatabase::open(&root);
            assert_eq!(db.files.len(), 1);
            let meta = db.get_file_meta(&path).unwrap();
            assert_eq!(meta.content_hash, hash);
            assert!(db.has_valid_ast(&path, hash));
            assert_eq!(&*db.get_ast(&path, hash).unwrap(), &[1, 2, 3, 4][..]);
            assert!(!db.has_valid_ast(&path, 999));
            assert_eq!(
                db.get_preprocessed(&path, hash).unwrap().combined,
                "`line 1 \"test/counter.sv\"\nmodule c;\nendmodule"
            );
            assert_eq!(db.get_verify(hash).unwrap().result_hash, 7);
            assert!(db.symbols.locate("counter", "module").is_some());
            let id = db.snapshot("build1".into()).unwrap();
            assert_eq!(id, 1);
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            assert_eq!(db.snapshots, vec![1]);
            assert!(snapshot_path(&db.state_dir(), 1).exists());
            // Payload AST tersimpan sebagai objek CAS di objects/default/.
            assert!(db.object_path(hash, OBJ_AST).exists());
            assert!(db.object_path(hash, OBJ_PREPROC).exists());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rollback_restores_state() {
        let root = test_root("rollback");
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                path.clone(),
                111,
                vec![],
                FileStatus::Unchanged,
                0,
                10,
                vec![],
            );
            db.save().unwrap();
            let id = db.snapshot("s1".into()).unwrap();
            assert_eq!(id, 1);
            db.record_file(
                path.clone(),
                222,
                vec![],
                FileStatus::Recompiled,
                0,
                10,
                vec![],
            );
            db.save().unwrap();
            assert_eq!(db.get_file_meta(&path).unwrap().content_hash, 222);
            db.rollback(1).unwrap();
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            assert_eq!(db.get_file_meta(&path).unwrap().content_hash, 111);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Fase 0 (Kritik B): dirty_diag isolasi + zero-copy get_slice ──

    #[test]
    fn test_fase0_dirty_diag_tidak_menulis_metadata() {
        // set_diags saja → hanya dirty_diag, metadata.mdb tidak tersentuh.
        let root = test_root("fase0_diag");
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(path.clone(), 1, vec![], FileStatus::New, 0, 5, vec![]);
            db.save().unwrap();
            assert!(!db.dirty && !db.dirty_diag);
        }
        let meta_path = state_dir(&root, "default").join(DB_METADATA);
        let meta_before = std::fs::read(&meta_path).unwrap();
        {
            let mut db = MicdDatabase::open(&root);
            assert!(!db.dirty && !db.dirty_diag);
            db.set_diags(FileDiags {
                path: path.clone(),
                content_hash: 1,
                entries: vec![],
            });
            assert!(db.dirty_diag, "set_diags harus tandai dirty_diag");
            assert!(!db.dirty, "set_diags TIDAK boleh tandai dirty metadata");
            db.save().unwrap();
        }
        let meta_after = std::fs::read(&meta_path).unwrap();
        assert_eq!(
            meta_before, meta_after,
            "metadata.mdb tidak boleh berubah saat hanya diag berubah"
        );
        {
            let db = MicdDatabase::open(&root);
            assert!(db.get_diags(&path).is_some());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase0_get_slice_sama_dengan_get() {
        // Helper read_mdb_singleton/entries pakai get_slice zero-copy —
        // hasilnya harus identik dengan jalur get().
        let root = test_root("fase0_slice");
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(path.clone(), 77, vec![], FileStatus::New, 0, 5, vec![]);
            db.set_diags(FileDiags {
                path: path.clone(),
                content_hash: 77,
                entries: vec![],
            });
            let mut v = VerifyResult::fresh(77);
            v.ast_hash = 0xABCD;
            v.semantic_hash = 0x1234;
            v.parse_ok = true;
            db.set_verify(v);
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            // Load via helper baru (get_slice) — semua store terbaca.
            assert_eq!(db.files.len(), 1);
            assert!(db.get_diags(&path).is_some());
            assert!(db.reuse_verify(77, 0, 0).is_some());
            assert!(db.reuse_verify(999, 0xABCD, 0).is_some());
            assert!(db.reuse_verify(999, 0, 0x1234).is_some());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase0_preproc_ref_tanpa_clone_vec() {
        // get_preprocessed_ref / combined: tanpa clone timescale_segments.
        let root = test_root("fase0_preproc_ref");
        let path = PathBuf::from("a.sv");
        let entry = PreprocEntry {
            content_hash: 55,
            combined: "module a; endmodule".to_string(),
            timescale: Some(("1ns".to_string(), "1ps".to_string())),
            timescale_segments: vec![(3, ("1ns".to_string(), "1ps".to_string())); 16],
        };
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(path.clone(), 55, vec![], FileStatus::New, 0, 5, vec![]);
            db.cache_preprocessed(path.clone(), entry.clone());
            // Ref: pointer sama dengan isi cache, tanpa alokasi baru.
            let r = db.get_preprocessed_ref(&path, 55).unwrap();
            assert_eq!(r.combined, "module a; endmodule");
            assert_eq!(r.timescale_segments.len(), 16);
            assert_eq!(db.get_preprocessed_combined(&path, 55).unwrap(), "module a; endmodule");
            assert!(db.has_valid_preproc(&path, 55));
            assert!(!db.has_valid_preproc(&path, 999));
            assert!(db.get_preprocessed_ref(&path, 999).is_none());
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            // Setelah reload, ref tetap valid + combined sama.
            assert_eq!(db.get_preprocessed_combined(&path, 55).unwrap(), "module a; endmodule");
            // Jalur lama (clone) tetap kompatibel.
            assert_eq!(db.get_preprocessed(&path, 55).unwrap().combined, "module a; endmodule");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase0_rollback_menandai_semua_dirty_dan_rebuild_index() {
        // Rollback harus tandai graph/verify/symbol/diag + rebuild index.
        let root = test_root("fase0_rollback");
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(path.clone(), 100, vec![], FileStatus::New, 0, 5, vec![]);
            let mut v = VerifyResult::fresh(100);
            v.ast_hash = 0xAAA;
            v.semantic_hash = 0xBBB;
            v.parse_ok = true;
            db.set_verify(v);
            db.add_symbol("top".into(), "module".into(), path.clone());
            db.set_diags(FileDiags {
                path: path.clone(),
                content_hash: 100,
                entries: vec![],
            });
            db.save().unwrap();
            let id = db.snapshot("s1".into()).unwrap();
            assert_eq!(id, 1);
            // Ubah semua store setelah snapshot.
            db.record_file(path.clone(), 200, vec![], FileStatus::Recompiled, 0, 5, vec![]);
            db.save().unwrap();
            // Rollback → semua dirty flag terkait harus menyala.
            db.rollback(1).unwrap();
            assert!(db.dirty, "metadata berubah → dirty");
            assert!(db.dirty_graph, "graph di-restore → dirty_graph");
            assert!(db.dirty_verify, "verify di-restore → dirty_verify");
            assert!(db.dirty_symbol, "symbol di-restore → dirty_symbol");
            assert!(db.dirty_diag, "diag di-restore → dirty_diag");
            // Indeks verify dibangun ulang → reuse via ast/semantic jalan.
            assert!(db.reuse_verify(999, 0xAAA, 0).is_some(), "ast index rollback");
            assert!(db.reuse_verify(999, 0, 0xBBB).is_some(), "semantic index rollback");
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            assert_eq!(db.get_file_meta(&path).unwrap().content_hash, 100);
            assert!(db.reuse_verify(999, 0xAAA, 0).is_some(), "index persist setelah save");
            // Review Fase 0: graph/symbol/diag hasil rollback harus persist.
            assert!(
                db.symbols.locate("top", "module").is_some(),
                "symbol rollback harus persist"
            );
            assert!(
                db.get_diags(&path).is_some(),
                "diag rollback harus persist"
            );
            assert!(
                db.get_verify(100).is_some(),
                "verify rollback harus persist"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Fase 2 (Kritik A): mirror write-through + legacy freeze ──

    #[test]
    fn test_fase2_mirror_verify_state_dan_cache_sama() {
        // set_verify menulis state/ DAN cermin cache/verify (satu setter).
        use crate::micd::cache::pipeline::VerifyPayload;
        let root = test_root("fase2_verify");
        {
            let mut db = MicdDatabase::open(&root);
            // File terdaftar (wajib: tanpa metadata, reopen menganggap fresh).
            db.record_file(
                PathBuf::from("a.sv"),
                500,
                vec![],
                FileStatus::New,
                0,
                5,
                vec![],
            );
            let mut v = VerifyResult::fresh(500);
            v.parse_ok = true;
            v.elab_ok = true;
            v.err_count = 2;
            v.warn_count = 1;
            db.set_verify(v);
            db.save().unwrap();
        }
        {
            let mut db = MicdDatabase::open(&root);
            // State: ada.
            let v = db.get_verify(500).unwrap();
            assert!(v.parse_ok && v.elab_ok);
            assert_eq!(v.err_count, 2);
            // Cermin cache: ada + isi sama.
            let key = format!("{:016x}", 500u64);
            let bytes = db
                .cache_layer
                .as_mut()
                .and_then(|l| l.get(CacheCategory::Verify, &key))
                .expect("cermin cache/verify harus ada");
            let p: VerifyPayload = bincode::deserialize(&bytes).unwrap();
            assert!(p.parse_ok && p.elab_ok);
            assert_eq!((p.err_count, p.warn_count, p.info_count), (2, 1, 0));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase2_mirror_profile_last() {        // set_stats mencerminkan BuildProfile ke cache/profile "last".
        let root = test_root("fase2_profile");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                PathBuf::from("a.sv"),
                1,
                vec![],
                FileStatus::New,
                0,
                5,
                vec![],
            );
            let mut p = db.stats_db.next_profile();
            p.total_ms = 777;
            p.changed_files = 3;
            db.set_stats(p);
            db.save().unwrap();
        }
        {
            let mut db = MicdDatabase::open(&root);
            assert_eq!(db.stats_db.total_builds(), 1);
            let bytes = db
                .cache_layer
                .as_mut()
                .and_then(|l| l.get(CacheCategory::Profile, "last"))
                .expect("cermin cache/profile harus ada");
            let p: BuildProfile = bincode::deserialize(&bytes).unwrap();
            assert_eq!(p.total_ms, 777);
            assert_eq!(p.changed_files, 3);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase2_mirror_best_effort_tanpa_cache() {
        // Mirror tidak boleh panic/gagalkan state bila cache_layer None.
        let root = test_root("fase2_nocache");
        let mut db = MicdDatabase::open(&root);
        db.cache_layer = None;
        db.record_file(
            PathBuf::from("a.sv"),
            600,
            vec![],
            FileStatus::New,
            0,
            5,
            vec![],
        );
        let mut v = VerifyResult::fresh(600);
        v.parse_ok = true;
        db.set_verify(v);
        let mut p = db.stats_db.next_profile();
        p.total_ms = 11;
        db.set_stats(p);
        db.save().unwrap();
        assert!(db.get_verify(600).is_some(), "state tetap tersimpan");
        assert_eq!(db.stats_db.total_builds(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase2_sweep_store_mati_dan_migrate_idempoten() {
        // Store gaya lama di state/ disapu; open 2× hasil identik (frozen).
        let root = test_root("fase2_sweep");
        let pid = "default";
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(PathBuf::from("a.sv"), 9, vec![], FileStatus::New, 0, 5, vec![]);
            db.save().unwrap();
        }
        let st = state_dir(&root, pid);
        // Tanam sisa usang + tmp menggantung.
        std::fs::write(st.join(DB_AST), b"usang").unwrap();
        std::fs::write(st.join(DB_PREPROC), b"usang").unwrap();
        std::fs::write(st.join(DB_AST).with_extension("mdb.tmp"), b"tmp").unwrap();
        {
            let db = MicdDatabase::open(&root);
            assert!(!st.join(DB_AST).exists(), "ast.mdb usang disapu");
            assert!(!st.join(DB_PREPROC).exists(), "preproc.mdb usang disapu");
            assert!(!st.join(DB_AST).with_extension("mdb.tmp").exists());
            assert_eq!(db.files.len(), 1, "data valid tetap");
        }
        {
            // Idempoten: open lagi → sama, tidak ada yang berubah.
            let db2 = MicdDatabase::open(&root);
            assert_eq!(db2.files.len(), 1);
            assert!(!st.join(DB_AST).exists());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Fase 3 (Kritik C): invalidasi per-stage, fingerprint = statistik ──

    #[test]
    fn test_fase3_pid_stabil_lintas_rebuild() {
        // pid hanya dari base compiler — tidak memuat fingerprint binary.
        let pid1 = MicdDatabase::project_id(Path::new("/p"), &[PathBuf::from("a.sv")], &[], &[]);
        let pid2 = MicdDatabase::project_id(Path::new("/p"), &[PathBuf::from("a.sv")], &[], &[]);
        assert_eq!(pid1, pid2, "deterministik");
        assert!(
            !pid1.contains(&format!("{:016x}", binary_fingerprint())),
            "pid tidak boleh memuat fingerprint (dulu via pipeline_revision)"
        );
    }

    #[test]
    fn test_fase3_compiler_base_matches() {
        assert!(compiler_base_matches(COMPILER_VERSION));
        assert!(compiler_base_matches(&pipeline_revision()), "legacy suffix diterima");
        assert!(!compiler_base_matches("Mivon 0.0.0-p99"));
        assert!(!compiler_base_matches("acak-tanpa-arti-kecuali-base"));
        assert!(!compiler_base_matches(""));
        // Suffix bukan-hex / panjang salah ditolak (R1).
        assert!(!compiler_base_matches(&format!("{}-zzz", COMPILER_VERSION)));
        assert!(!compiler_base_matches(&format!("{}-", COMPILER_VERSION)));
        assert!(!compiler_base_matches(&format!("{}-123", COMPILER_VERSION)));
    }

    #[test]
    fn test_fase3_manifest_legacy_dan_bump() {
        // Manifest legacy (base+suffix, tanpa stage) tetap di-reuse;
        // bump -p (base beda) → wipe; stage beda → wipe.
        let root = test_root("fase3_compat");
        let st = state_dir(&root, "default");
        let path = PathBuf::from("a.sv");
        std::fs::create_dir_all(&st).unwrap();
        let write_manifest = |compiler: &str, stages: Vec<(String, u64)>| {
            let mut w = MdbWriter::new();
            w.put(
                KEY_SINGLETON,
                format::KIND_MANIFEST,
                bincode::serialize(&MetadataManifest {
                    paths: vec![path.clone()],
                    flags_hash: 0,
                    compiler_version: compiler.to_string(),
                    created_ns: 0,
                    schema_version: SCHEMA_VERSION,
                    stage_schemas: stages,
                })
                .unwrap(),
            );
            w.put(
                path_hash(&path),
                format::KIND_META,
                bincode::serialize(&FileMeta {
                    path: path.clone(),
                    content_hash: 7,
                    mtime_ns: 0,
                    size: 3,
                    status: FileStatus::Unchanged,
                    flags_hash: 0,
                    deps: vec![],
                    include_hashes: vec![],
                    compiled_at_ns: 0,
                    ast_format_version: AST_FORMAT_VERSION,
                })
                .unwrap(),
            );
            w.write_to(&st.join(DB_METADATA)).unwrap();
        };
        // Legacy: suffix fingerprint + stage current → reuse.
        write_manifest(&pipeline_revision(), current_stage_schemas());
        let db = MicdDatabase::open(&root);
        assert_eq!(db.files.len(), 1, "manifest legacy harus di-reuse (bukan wipe)");
        // Bump -p → wipe.
        write_manifest("Mivon 0.0.0-p99", vec![]);
        let db2 = MicdDatabase::open(&root);
        assert!(db2.files.is_empty(), "base beda harus wipe");
        // Stage beda → wipe.
        let mut stages = current_stage_schemas();
        stages[0].1 += 1000;
        write_manifest(COMPILER_VERSION, stages);
        let db3 = MicdDatabase::open(&root);
        assert!(db3.files.is_empty(), "stage beda harus wipe");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase3_stage_tersimpan_dan_registry_statistik() {
        let root = test_root("fase3_stats");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(PathBuf::from("a.sv"), 3, vec![], FileStatus::New, 0, 5, vec![]);
            db.save().unwrap();
        }
        // Manifest menyimpan stage_schemas current.
        let r = MdbReader::open(&state_dir(&root, "default").join(DB_METADATA)).unwrap();
        let m: MetadataManifest =
            bincode::deserialize(&r.get(KEY_SINGLETON).unwrap()).unwrap();
        assert_eq!(m.compiler_version, COMPILER_VERSION, "tanpa suffix fingerprint");
        assert_eq!(m.stage_schemas, current_stage_schemas());
        assert_eq!(m.schema_version, SCHEMA_VERSION);
        // Registry: fingerprint + revisi tercatat sebagai statistik.
        let map = read_registry(&root);
        let info = map.get("default").expect("pid tercatat");
        assert_eq!(info.compiler_version, COMPILER_VERSION);
        assert_eq!(info.binary_fingerprint, binary_fingerprint());
        // Reopen: reuse penuh (tidak wipe).
        let db2 = MicdDatabase::open(&root);
        assert_eq!(db2.files.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fase0_singleton_terkompresi_fallback_get() {        // Review Fase 0: satu-satunya store terkompresi adalah graph.mdb
        // (LZ4) — get_slice mengembalikan None, helper harus fallback ke get.
        use crate::micd::format::{Compression, MdbReader, MdbWriter, KIND_GRAPH};
        let dir = test_root("fase0_lz4");
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = FileGraph::new();
        g.set_deps(PathBuf::from("cpu.sv"), vec![PathBuf::from("uart.sv")]);
        g.rebuild();
        let mut w = MdbWriter::with_compression(Compression::Lz4);
        w.put(
            KEY_SINGLETON,
            KIND_GRAPH,
            bincode::serialize(&g).unwrap(),
        );
        w.write_to(&dir.join(DB_GRAPH)).unwrap();
        // Bukti premis: store terkompresi → get_slice None.
        let r = MdbReader::open(&dir.join(DB_GRAPH)).unwrap();
        assert!(r.get_slice(KEY_SINGLETON).is_none());
        // Helper tetap bisa baca via fallback get().
        let back: Option<FileGraph> = read_mdb_singleton(&dir, DB_GRAPH);
        assert!(back.is_some(), "fallback get untuk LZ4 harus berhasil");
        let affected = {
            let mut gg = back.unwrap();
            gg.affected(&[PathBuf::from("uart.sv")])
        };
        assert!(affected.contains(&PathBuf::from("cpu.sv")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_affected_via_graph() {
        let root = test_root("graph");
        let mut db = MicdDatabase::open(&root);
        db.graph
            .set_deps(PathBuf::from("cpu.sv"), vec![PathBuf::from("uart.sv")]);
        db.graph
            .set_deps(PathBuf::from("uart.sv"), vec![PathBuf::from("defines.svh")]);
        let affected = db.affected(&[PathBuf::from("defines.svh")]);
        assert!(affected.contains(&PathBuf::from("cpu.sv")));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Kritik 3 db.md: Versioned Schema ──

    #[test]
    fn test_schema_version_persisted() {
        let root = test_root("schema_persist");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                PathBuf::from("a.sv"),
                1,
                vec![],
                FileStatus::New,
                0,
                5,
                vec![],
            );
            db.save().unwrap();
            assert_eq!(db.schema_version, SCHEMA_VERSION);
        }
        {
            let db = MicdDatabase::open(&root);
            assert_eq!(db.schema_version, SCHEMA_VERSION);
            assert_eq!(db.files.len(), 1);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_schema_version_mismatch_rebuilds_empty() {
        let root = test_root("schema_mismatch");
        let st = default_state(&root);
        let objs = objects_dir(&root, "default");
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(path.clone(), 42, vec![], FileStatus::New, 0, 5, vec![]);
            db.cache_ast(path.clone(), 42, vec![9, 9]);
            db.add_symbol("a".into(), "module".into(), path.clone());
            db.set_verify(VerifyResult::fresh(42));
            db.save().unwrap();
        }
        // Palsukan skema lain: tulis ulang manifest dengan schema_version tua.
        {
            let mut w = MdbWriter::new();
            let manifest = MetadataManifest {
                paths: vec![path.clone()],
                flags_hash: 0,
                compiler_version: COMPILER_VERSION.into(),
                created_ns: 0,
                schema_version: SCHEMA_VERSION - 1,
                stage_schemas: Vec::new(),
            };
            w.put(
                KEY_SINGLETON,
                format::KIND_MANIFEST,
                bincode::serialize(&manifest).unwrap(),
            );
            w.write_to(&st.join(DB_METADATA)).unwrap();
        }
        // Buka → seluruh store dianggap tidak kompatibel → database kosong.
        let db = MicdDatabase::open(&root);
        assert_eq!(
            db.schema_version, SCHEMA_VERSION,
            "db baru memakai versi terkini"
        );
        assert!(db.files.is_empty(), "file lama tidak boleh di-load");
        assert!(
            !db.has_valid_ast(&path, 42),
            "AST lama tidak boleh di-reuse"
        );
        // Store lama harus DIBUANG deterministik dari disk (bukan dibiarkan
        // sebagai state campuran yang akan di-load run berikutnya).
        for name in [
            DB_GRAPH, DB_SYMBOL, DB_TYPE, DB_VERIFY, DB_PREPROC, DB_STATS,
        ] {
            assert!(
                !st.join(name).exists(),
                "store lama '{}' harus terhapus saat schema mismatch",
                name
            );
        }
        assert!(
            !objs.exists(),
            "objek CAS lama dihapus saat schema mismatch"
        );
        // Save berikutnya membangun database baru yang konsisten.
        let mut db = db;
        db.record_file(path.clone(), 42, vec![], FileStatus::New, 0, 5, vec![]);
        db.save().unwrap();
        let db2 = MicdDatabase::open(&root);
        assert_eq!(db2.schema_version, SCHEMA_VERSION);
        assert_eq!(db2.files.len(), 1);
        assert_eq!(db2.files.get(&path).unwrap().content_hash, 42);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Kritik 4 & 5 db.md: Transaction + Crash Recovery ──

    #[test]
    fn test_crash_recovery_discards_corrupt_store() {
        let root = test_root("crash");
        let st = default_state(&root);
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(path.clone(), 7, vec![], FileStatus::New, 0, 5, vec![]);
            db.save().unwrap();
        }
        // Simulasikan crash saat commit: journal tersisa + verify.mdb korup.
        let journal = st.join(DB_JOURNAL);
        write_journal(
            &journal,
            &[
                PathBuf::from(DB_METADATA),
                PathBuf::from(DB_VERIFY),
                PathBuf::from(DB_GRAPH),
            ],
        )
        .unwrap();
        std::fs::write(st.join(DB_VERIFY), vec![0xFF; 256]).unwrap();
        // graph.mdb dihapus total (seolah belum sempat rename).

        // Open → recovery: metadata valid dipertahankan, verify corrupt dibuang,
        // graph hilang dibuang. Journal dihapus.
        let db = MicdDatabase::open(&root);
        assert!(!journal.exists(), "journal harus dihapus recovery");
        assert_eq!(db.files.len(), 1, "store valid tetap di-load");
        assert!(db.verify.is_empty(), "store corrupt tidak di-load");
        assert!(db.graph.is_empty(), "store hilang tidak di-load");
        // Save berikutnya membangun ulang store yang hilang → db pulih total.
        let mut db = db;
        db.record_file(path.clone(), 7, vec![], FileStatus::Unchanged, 0, 5, vec![]);
        db.save().unwrap();
        let db2 = MicdDatabase::open(&root);
        assert_eq!(db2.files.len(), 1);
        assert!(!db2.state_dir().join(DB_JOURNAL).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_no_journal_left_after_normal_save() {
        let root = test_root("clean_save");
        let st = default_state(&root);
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                PathBuf::from("x.sv"),
                1,
                vec![],
                FileStatus::New,
                0,
                3,
                vec![],
            );
            db.save().unwrap();
        }
        assert!(
            !st.join(DB_JOURNAL).exists(),
            "save sukses → journal bersih"
        );
        // Tidak ada tmp yang menggantung.
        for name in [
            DB_METADATA,
            DB_GRAPH,
            DB_VERIFY,
            DB_AST,
            DB_PREPROC,
            DB_SYMBOL,
            DB_TYPE,
            DB_DIAG,
            DB_STATS,
        ] {
            assert!(!st.join(name).with_extension("mdb.tmp").exists());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_txn_single_file_failure_leaves_final_untouched() {
        // write_tmp gagal (dir tidak ada) → file final tidak boleh berubah.
        let root = test_root("txn_fail");
        let st = default_state(&root);
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                PathBuf::from("a.sv"),
                1,
                vec![],
                FileStatus::New,
                0,
                3,
                vec![],
            );
            db.save().unwrap();
        }
        let meta_before = std::fs::read(st.join(DB_METADATA)).unwrap();
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                PathBuf::from("a.sv"),
                2,
                vec![],
                FileStatus::Recompiled,
                0,
                3,
                vec![],
            );
            // Simulasikan kegagalan fase tmp: file penghalang di path parent
            // membuat create_dir_all / create file tmp gagal.
            let journal_path = st.join(DB_JOURNAL);
            let names = vec![PathBuf::from(DB_METADATA)];
            write_journal(&journal_path, &names).unwrap();
            std::fs::write(root.join("blocker"), b"x").unwrap();
            let err = format::write_tmp(&root.join("blocker").join(DB_METADATA), b"x");
            assert!(
                err.is_err(),
                "menulis ke path dengan parent file harus gagal"
            );
            // Recovery manual: buang journal, file final tetap versi lama.
            let _ = std::fs::remove_file(&journal_path);
        }
        let meta_after = std::fs::read(st.join(DB_METADATA)).unwrap();
        assert_eq!(
            meta_before, meta_after,
            "file final tidak boleh berubah saat gagal"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Kritik 1 db.md: Multi-level hash ──

    #[test]
    fn test_reuse_verify_by_ast_hash() {
        let root = test_root("reuse_ast");
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            let mut v = VerifyResult::fresh(100);
            v.ast_hash = 0xDEAD;
            v.semantic_hash = 0xBEEF;
            v.parse_ok = true;
            v.elab_ok = true;
            db.set_verify(v);
            db.record_file(
                path.clone(),
                100,
                vec![],
                FileStatus::Unchanged,
                0,
                5,
                vec![],
            );
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            // Content hash berubah (komentar ditambah), AST sama → reuse.
            let v = db.reuse_verify(999, 0xDEAD, 0).expect("reuse via ast_hash");
            assert_eq!(v.content_hash, 100);
            // Level semantic juga tersedia.
            let v2 = db
                .reuse_verify(999, 0x9999, 0xBEEF)
                .expect("reuse via semantic_hash");
            assert_eq!(v2.content_hash, 100);
            // Tidak ada level yang cocok → None.
            assert!(db.reuse_verify(999, 0x9999, 0x0000).is_none());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_reuse_verify_prefers_content_hash() {
        let root = test_root("reuse_content");
        let mut db = MicdDatabase::open(&root);
        let mut v = VerifyResult::fresh(100);
        v.ast_hash = 0xAA;
        v.parse_ok = true;
        v.elab_ok = true;
        db.set_verify(v);
        let got = db.reuse_verify(100, 0, 0).unwrap();
        assert_eq!(got.content_hash, 100, "content hash menang duluan");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Kritik 14 db.md: Statistics Database ──

    #[test]
    fn test_stats_db_persists_across_sessions() {
        let root = test_root("stats");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                PathBuf::from("a.sv"),
                1,
                vec![],
                FileStatus::New,
                0,
                3,
                vec![],
            );
            let mut p = db.stats_db.next_profile();
            p.total_ms = 1234;
            p.changed_files = 5;
            p.cache_hits = 3;
            p.cache_misses = 2;
            db.set_stats(p);
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            assert_eq!(db.stats_db.total_builds(), 1);
            let last = db.stats_db.last().unwrap();
            assert_eq!(last.total_ms, 1234);
            assert_eq!(last.cache_hits, 3);
            assert_eq!(db.stats_db.hit_rate_pct(), 60);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Stress tests ──

    #[test]
    fn test_stress_many_files_repeated_save_reload() {
        let root = test_root("stress_many");
        let n_files = 500;
        let paths: Vec<PathBuf> = (0..n_files)
            .map(|i| PathBuf::from(format!("mod_{:04}.sv", i)))
            .collect();

        // 10 siklus save/reload, tiap siklus tambah beberapa file baru.
        for cycle in 0..10 {
            let mut db = MicdDatabase::open(&root);
            #[allow(clippy::needless_range_loop)]
            for i in 0..n_files {
                let path = &paths[i];
                let hash = (i as u64) * 1000 + cycle as u64;
                db.record_file(
                    path.clone(),
                    hash,
                    vec![],
                    FileStatus::Unchanged,
                    0,
                    100,
                    vec![],
                );
                if i % 2 == 0 {
                    db.cache_ast(path.clone(), hash, vec![i as u8; 8]);
                }
            }
            db.save().unwrap();
            assert!(
                !db.state_dir().join(DB_JOURNAL).exists(),
                "journal harus bersih"
            );
        }

        let db = MicdDatabase::open(&root);
        assert_eq!(db.files.len(), n_files);
        assert_eq!(db.ast_cache.len(), n_files / 2);
        // Verify semua file masih terbaca.
        for (i, path) in paths.iter().take(n_files).enumerate() {
            let meta = db.get_file_meta(path).unwrap();
            assert_eq!(meta.content_hash, (i as u64) * 1000 + 9);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_stress_big_dependency_graph_affected() {
        let root = test_root("stress_graph");
        let mut db = MicdDatabase::open(&root);
        // Rantai 2000 file + 100 file depend pada node 0 → transitive closure.
        let n = 2000;
        for i in 0..n {
            let cur = PathBuf::from(format!("f{:04}.sv", i));
            let deps = if i > 0 {
                vec![PathBuf::from(format!("f{:04}.sv", i - 1))]
            } else {
                vec![]
            };
            db.graph.set_deps(cur, deps);
        }
        for i in 0..100 {
            db.graph.add_dep(
                PathBuf::from(format!("leaf{:03}.sv", i)),
                PathBuf::from("f0000.sv"),
            );
        }
        let affected = db.affected(&[PathBuf::from("f0000.sv")]);
        assert_eq!(affected.len(), n + 100, "transitive closure lengkap");
        // Reverse index dibangun ulang otomatis.
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_stress_verify_multi_level_matches() {
        // Banyak entry verify; pencarian semantic tidak memilih yang salah.
        let root = test_root("stress_verify");
        let mut db = MicdDatabase::open(&root);
        for i in 1..201u64 {
            let mut v = VerifyResult::fresh(i * 100);
            v.ast_hash = i * 1000 + 1;
            v.semantic_hash = i * 1000 + 2;
            v.parse_ok = true;
            v.elab_ok = true;
            db.set_verify(v);
        }
        let v = db.reuse_verify(999_999, 123_001, 0).unwrap();
        assert_eq!(v.content_hash, 12300);
        let v2 = db.reuse_verify(999_999, 999_999, 45_002).unwrap();
        assert_eq!(v2.content_hash, 4500);
        assert!(db.reuse_verify(0, 0, 0).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_semantic_index_persists_across_reload() {
        // Kritik E: verify_semantic_index harus persist dan O(1) lookup
        // setelah save/reload (bukan rebuild dari linear scan).
        let root = test_root("semantic_idx");
        {
            let mut db = MicdDatabase::open(&root);
            // Daftarkan file agar verify entry tidak di-compact GC.
            db.record_file(
                PathBuf::from("a.sv"),
                100,
                vec![],
                FileStatus::New,
                0,
                5,
                vec![],
            );
            let mut v = VerifyResult::fresh(100);
            v.ast_hash = 0xAAA;
            v.semantic_hash = 0xBBB;
            v.parse_ok = true;
            v.elab_ok = true;
            db.set_verify(v);
            db.save().unwrap();
        }
        {
            let db = MicdDatabase::open(&root);
            // Semantic index harus terisi dari load.
            assert!(
                db.verify_semantic_index.contains_key(&0xBBB),
                "semantic index harus ter-load dari disk"
            );
            // Lookup via semantic harus O(1) dan berhasil.
            let v = db
                .reuse_verify(999, 0x9999, 0xBBB)
                .expect("semantic index harus menemukan entry");
            assert_eq!(v.content_hash, 100);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Kritik 13 db.md: Snapshot DAG via MicdDatabase ──

    #[test]
    fn test_snapshot_dag_branch_and_merge() {
        let root = test_root("snap_dag");
        let mut db = MicdDatabase::open(&root);
        // Rantai: 1 → 2 → 3.
        let id1 = db.snapshot("root".into()).unwrap();
        assert_eq!(id1, 1);
        let id2 = db.snapshot("child1".into()).unwrap();
        let id3 = db.snapshot("child2".into()).unwrap();
        // Branch dari 1: snapshot 4 ber-parent 1.
        let id4 = db.snapshot_from(vec![id1], "branch".into()).unwrap();
        assert_eq!(id4, 4);
        // Merge: 5 ber-parent 3 dan 4.
        let id5 = db.snapshot_from(vec![id3, id4], "merge".into()).unwrap();
        assert_eq!(id5, 5);

        // Cek parent DAG. Snapshot tersimpan di state/<pid>/snapshots/.
        let st = db.state_dir();
        let s3 = read_snapshot(&st, id3).unwrap();
        assert_eq!(s3.parents, vec![id2]);
        let s5 = read_snapshot(&st, id5).unwrap();
        assert_eq!(s5.parents, vec![id3, id4], "merge punya dua parent");

        // History 5 mencakup semua.
        let mut hist = history_of(&st, id5);
        hist.sort_unstable();
        assert_eq!(hist, vec![1, 2, 3, 4, 5]);
        // Merge-base 3 dan 4 = 1.
        assert_eq!(merge_base(&st, id3, id4), Some(1));

        // Parent yang tidak ada → error.
        assert!(db.snapshot_from(vec![99], "invalid".into()).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_snapshot_prune_keeps_dag_intact() {
        let root = test_root("snap_prune");
        let mut db = MicdDatabase::open(&root);
        // Prune internal (private) — panggil via snapshot berulang lalu buktikan
        // yang tersisa tidak merusak keturunan. Maks 16 → buat 20 linear.
        for i in 0..20 {
            db.snapshot(format!("b{}", i)).unwrap();
        }
        assert!(db.snapshots.len() <= 16, "prune batasi jumlah snapshot");
        assert_eq!(
            *db.snapshots.first().unwrap(),
            5,
            "yang tertua dibuang (1-4)"
        );
        assert_eq!(*db.snapshots.last().unwrap(), 20, "terbaru dipertahankan");
        // Snapshot terakhir masih punya riwayat ke root baru (5).
        let last = *db.snapshots.last().unwrap();
        let st = db.state_dir();
        let hist = history_of(&st, last);
        assert!(hist.contains(&5), "garis keturunan ke root tetap ada");
        assert!(hist.contains(&20), "snapshot terakhir ada di riwayatnya");
        // Tidak ada referensi parent menggantung (parent yang hilang di-squash).
        let s20 = read_snapshot(&st, 20).unwrap();
        assert!(!s20.parents.contains(&1), "parent ter-squash tidak muncul");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Kritik 6 db.md: GC otomatis saat save ──

    #[test]
    fn test_gc_on_save_trims_cache() {
        let root = test_root("gc_save");
        let mut db = MicdDatabase::open(&root);
        db.gc_on_save = true;
        // Tambah AST dengan file terdaftar (compaction tidak akan salah
        // menghapusnya karena path dikenal).
        for i in 0..50u64 {
            let p = PathBuf::from(format!("f{:02}.sv", i));
            db.record_file(p.clone(), i, vec![], FileStatus::New, 0, 10_000, vec![]);
            db.ast_accessed.insert(p.clone(), now_ns());
            db.ast_cache.insert(p, (i, vec![0u8; 10_000].into()));
            db.ast_bytes += 10_000;
        }
        // GcConfig default budget 256MB tidak men-trigger untuk 500KB.
        // Pastikan save tetap sukses (GC idempoten, tidak menggagalkan save)
        // dan seluruh AST tetap ada (di bawah budget).
        db.save().unwrap();
        assert_eq!(db.ast_bytes, 50 * 10_000);
        assert_eq!(db.ast_cache.len(), 50);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── MICD cache per-project: prune file lintas-run ──

    #[test]
    fn test_prune_stale_removes_foreign_project_files() {
        let root = test_root("prune_stale");
        let mut db = MicdDatabase::open(&root);
        // Skenario: opentitan (3970 file) + counter.sv dari run lain menempel.
        for i in 0..3 {
            let p = PathBuf::from(format!("ot_{}.sv", i));
            db.record_file(p.clone(), i, vec![], FileStatus::Unchanged, 0, 5, vec![]);
            db.cache_ast(p.clone(), i, vec![0u8; 10]);
            db.add_symbol(format!("mod_{}", i), "module".into(), p.clone());
        }
        let foreign = PathBuf::from("test/counter.sv");
        db.record_file(
            foreign.clone(),
            99,
            vec![],
            FileStatus::Unchanged,
            0,
            5,
            vec![],
        );
        db.cache_ast(foreign.clone(), 99, vec![1u8; 10]);
        db.cache_preprocessed(
            foreign.clone(),
            PreprocEntry {
                content_hash: 99,
                combined: "module counter; endmodule".into(),
                timescale: None,
                timescale_segments: Vec::new(),
            },
        );
        db.set_diags(crate::micd::FileDiags {
            path: foreign.clone(),
            entries: vec![],
            content_hash: 99,
        });
        db.graph
            .set_deps(foreign.clone(), vec![PathBuf::from("defines.svh")]);

        assert_eq!(db.files.len(), 4);
        // Active project hanya ot_*.sv.
        let active: Vec<PathBuf> = (0..3)
            .map(|i| PathBuf::from(format!("ot_{}.sv", i)))
            .collect();
        let pruned = db.prune_stale(&active);
        assert_eq!(pruned, 1, "file project lain dibuang");
        assert_eq!(db.files.len(), 3);
        assert!(!db.files.contains_key(&foreign));
        assert!(!db.ast_cache.contains_key(&foreign));
        assert!(!db.preproc_cache.contains_key(&foreign));
        assert!(!db.diags.contains_key(&foreign));
        assert_eq!(db.ast_bytes, 30, "bytes foreign dibebaskan");
        assert_eq!(db.preproc_bytes, 0, "preproc foreign dibebaskan");
        // Graph tidak menyimpan foreign lagi.
        assert!(db.graph.deps_of(&foreign).is_empty());
        // File project aktif tetap utuh.
        assert!(db.files.contains_key(&PathBuf::from("ot_0.sv")));
        // Save setelah prune tidak error.
        db.save().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Kritik B: dirty_diag terpisah dari dirty metadata ──

    #[test]
    fn test_dirty_diag_does_not_rewrite_metadata() {
        // set_diags SAJA (tanpa record_file) → hanya diagnostics.mdb yang
        // ditulis, metadata.mdb TIDAK ikut tulis ulang.
        let root = test_root("dirty_diag");
        let st = default_state(&root);
        let path = PathBuf::from("a.sv");
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                path.clone(),
                42,
                vec![],
                FileStatus::New,
                0,
                5,
                vec![],
            );
            db.save().unwrap();
        }
        // Simpan checksum metadata.mdb awal.
        let meta_before = std::fs::read(st.join(DB_METADATA)).unwrap();
        {
            let mut db = MicdDatabase::open(&root);
            // Hanya set diagnostic — tidak menyentuh metadata.
            db.set_diags(crate::micd::FileDiags {
                path: path.clone(),
                entries: vec![crate::micd::DiagEntry::new(
                    10,
                    5,
                    crate::micd::DiagSeverity::Warning,
                    "unused signal".into(),
                    "W001".into(),
                )],
                content_hash: 42,
            });
            // dirty_diag harus true, dirty metadata harus false.
            assert!(db.dirty_diag, "dirty_diag harus true setelah set_diags");
            assert!(!db.dirty, "dirty metadata tidak boleh true dari set_diags");
            db.save().unwrap();
        }
        // Metadata.mdb harus TIDAK berubah (byte identik).
        let meta_after = std::fs::read(st.join(DB_METADATA)).unwrap();
        assert_eq!(
            meta_before, meta_after,
            "metadata.mdb tidak boleh tulis ulang saat hanya diagnostic berubah"
        );
        // diagnostics.mdb harus ada dan berisi data baru.
        assert!(st.join(DB_DIAG).exists(), "diagnostics.mdb harus ditulis");
        {
            let db = MicdDatabase::open(&root);
            let d = db.get_diags(&path).unwrap();
            assert_eq!(d.entries.len(), 1);
            assert_eq!(d.entries[0].message, "unused signal");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_dirty_diag_only_save_minimal() {
        // Hanya set_diags → save() tidak menulis metadata/graph/verify/symbol/
        // type/stats — hanya diagnostics.mdb.
        let root = test_root("dirty_diag_minimal");
        let st = default_state(&root);
        {
            let mut db = MicdDatabase::open(&root);
            db.record_file(
                PathBuf::from("a.sv"),
                1,
                vec![],
                FileStatus::New,
                0,
                3,
                vec![],
            );
            db.save().unwrap();
        }
        // Hapus diagnostics.mdb lama (bila ada) supaya bisa cek fresh write.
        let _ = std::fs::remove_file(st.join(DB_DIAG));
        {
            let mut db = MicdDatabase::open(&root);
            db.set_diags(crate::micd::FileDiags {
                path: PathBuf::from("a.sv"),
                entries: vec![],
                content_hash: 1,
            });
            db.save().unwrap();
        }
        // diagnostics.mdb harus ditulis.
        assert!(st.join(DB_DIAG).exists());
        // Tidak ada journal tersisa.
        assert!(!st.join(DB_JOURNAL).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
