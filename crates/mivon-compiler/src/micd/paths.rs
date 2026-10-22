//! Path & konstruktor `MicdDatabase`: root default/fallback, layout
//! Git-style (state/objects/locks), dan ProjectID.
use super::*;

impl MicdDatabase {
    /// Lokasi alternatif saat root default tidak bisa dipakai: `.mivon` adalah
    /// FILE (project file untuk `--filelist`/`-f`), bukan folder → MICD harus
    /// memakai folder terpisah agar tidak konflik.
    pub(super) fn fallback_root() -> PathBuf {
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
}
