//! snapshots/ — snapshot build (mirip commit git) untuk rollback.
//!
//! Setiap build yang berhasil menyimpan snapshot state (metadata + graph +
//! verify + symbol index) ke `snapshots/build-NNN` sebagai satu blob biner.
//! Rollback mengembalikan state tersebut, lalu seluruh artefak AST yang
//! cocok dengan metadata di-reuse pada build berikutnya.
//!
//! Sejak Kritik 13 db.md, snapshot membentuk **DAG** (bukan rantai linear):
//! setiap snapshot menyimpan `parents` (biasanya satu, bisa beberapa untuk
//! merge/multi-parent). `parent = []` adalah snapshot akar (build pertama).
//! Mirip commit Git: `snapshot_from(parents, ...)` membuat snapshot turunan
//! dari snapshot yang dipilih; `snapshot(...)` otomatis mengambil parent
//! terakhir. Linear tetap mungkin (tiap snapshot ber-parent satu).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::diag::FileDiags;
use super::graph::FileGraph;
use super::lock::acquire_write_lock;
use super::metadata::FileMeta;
use super::symbol::SymbolIndex;
use super::verify::now_ns;
use super::verify::VerifyResult;
use super::MicdDatabase;

/// Isi satu snapshot build.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// Nomor build (incrementing).
    pub id: u64,
    /// Waktu pembuatan (unix ns).
    pub created_ns: u64,
    /// Parent snapshot (DAG, Kritik 13). Kosong = snapshot akar.
    /// Umumnya 1 (linear); bisa >1 untuk merge (branch digabung).
    pub parents: Vec<u64>,
    /// Metadata seluruh file.
    pub files: Vec<FileMeta>,
    /// Dependency graph.
    pub graph: FileGraph,
    /// Verification cache.
    pub verify: Vec<(u64, VerifyResult)>,
    /// Index simbol.
    pub symbols: SymbolIndex,
    /// Diagnostic per file.
    pub diags: Vec<FileDiags>,
    /// Hash flags kompilasi saat snapshot.
    pub flags_hash: u64,
    /// Pesan ringkas (opsional).
    pub note: String,
}

impl Snapshot {
    pub fn root() -> Self {
        Snapshot::default()
    }

    /// Snapshot yang punya parent `parent`.
    pub fn with_parent(parent: u64) -> Self {
        Snapshot {
            parents: vec![parent],
            ..Snapshot::default()
        }
    }
}

/// Nama file snapshot untuk sebuah build id.
pub fn snapshot_path(root: &Path, id: u64) -> PathBuf {
    root.join("snapshots").join(format!("build-{:03}", id))
}

/// List snapshot id yang ada di root (scan direktori snapshots/).
pub fn list_snapshots(root: &Path) -> Vec<u64> {
    let dir = root.join("snapshots");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(rest) = name.strip_prefix("build-") {
            if let Ok(id) = rest.parse::<u64>() {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable();
    ids
}

/// Baca satu snapshot dari disk. `None` bila tidak ada / corrupt.
pub fn read_snapshot(root: &Path, id: u64) -> Option<Snapshot> {
    let path = snapshot_path(root, id);
    let bytes = std::fs::read(&path).ok()?;
    bincode::deserialize(&bytes).ok()
}

/// Ambil `id` terbesar dari daftar yang tersedia (atau 0 bila kosong).
pub fn last_snapshot_id(ids: &[u64]) -> u64 {
    ids.iter().copied().max().unwrap_or(0)
}

/// Parent snapshot (DAG, Kritik 13). `None` bila snapshot tidak ada.
pub fn parents_of(root: &Path, id: u64) -> Option<Vec<u64>> {
    read_snapshot(root, id).map(|s| s.parents)
}

/// Seluruh riwayat (ancestor chain) dari snapshot `id` ke akar, DFS.
/// Termasuk `id` itu sendiri di posisi pertama. Menghasilkan DAG, bukan
/// rantai — bila ada merge, kedua cabang ikut.
pub fn history_of(root: &Path, id: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![id];
    while let Some(cur) = stack.pop() {
        if !visited.insert(cur) {
            continue;
        }
        out.push(cur);
        if let Some(parents) = parents_of(root, cur) {
            for p in parents {
                if !visited.contains(&p) {
                    stack.push(p);
                }
            }
        }
    }
    out
}

/// Common ancestor terdekat (merge-base) dari dua snapshot. `None` bila
/// tidak ada yang berbagi garis keturunan.
pub fn merge_base(root: &Path, a: u64, b: u64) -> Option<u64> {
    let hist_a: std::collections::HashSet<u64> = history_of(root, a).into_iter().collect();
    let mut stack = vec![b];
    let mut visited = std::collections::HashSet::new();
    while let Some(cur) = stack.pop() {
        if !visited.insert(cur) {
            continue;
        }
        if hist_a.contains(&cur) {
            return Some(cur);
        }
        if let Some(parents) = parents_of(root, cur) {
            for p in parents {
                if !visited.contains(&p) {
                    stack.push(p);
                }
            }
        }
    }
    None
}

// ─── Operasi snapshot/rollback pada MicdDatabase ───
//
// 1 file = 1 tanggung jawab: manajemen snapshot (buat, prune, rollback)
// tinggal di sini — mod.rs hanya facade pembuka database.

impl MicdDatabase {
    /// Buat snapshot build baru (mirip commit). Menyimpan state saat ini.
    /// Parent otomatis = snapshot terakhir (linear default). Untuk membuat
    /// DAG bercabang/merge, gunakan [`MicdDatabase::snapshot_from`].
    pub fn snapshot(&mut self, note: String) -> io::Result<u64> {
        let parent = last_snapshot_id(&self.snapshots);
        let parents = if parent == 0 {
            Vec::new()
        } else {
            vec![parent]
        };
        self.snapshot_from(parents, note)
    }

    /// Buat snapshot dengan parent eksplisit (DAG, Kritik 13 db.md).
    /// `parents = []` → snapshot akar; `parents = [a]` → turunan a (linear);
    /// `parents = [a, b]` → merge (dua cabang digabung, mirip Git).
    /// Semua parent harus sudah ada; id otomatis = max(id)+1.
    pub fn snapshot_from(&mut self, parents: Vec<u64>, note: String) -> io::Result<u64> {
        for p in &parents {
            if !self.snapshots.contains(p) {
                return Err(io::Error::other(format!(
                    "MICD snapshot: parent {} belum ada",
                    p
                )));
            }
        }
        let _lock = acquire_write_lock(
            &self.lock_path(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        let id = last_snapshot_id(&self.snapshots) + 1;
        let snap = Snapshot {
            id,
            created_ns: now_ns(),
            parents,
            files: self.files.values().cloned().collect(),
            graph: self.graph.clone(),
            verify: self.verify.iter().map(|(k, v)| (*k, v.clone())).collect(),
            symbols: self.symbols.clone(),
            diags: self.diags.values().cloned().collect(),
            flags_hash: self.flags_hash,
            note,
        };
        let path = snapshot_path(&self.state_dir(), id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(&snap).map_err(io::Error::other)?;
        std::fs::write(&path, bytes)?;
        self.snapshots.push(id);
        // Pertahankan maksimal 16 snapshot. Buang yang TIDAK menjadi parent
        // snapshot lain (leaf tertua) — bukan asal buang terdepan, agar DAG
        // tidak kehilangan merge-base.
        self.prune_snapshots(16);
        Ok(id)
    }

    /// Buang snapshot berlebih (maks `keep`) tanpa merusak DAG.
    /// Selalu buang yang TERTUA (id terkecil) agar snapshot terbaru tetap
    /// ada (build terakhir yang paling berguna untuk rollback). Untuk menjaga
    /// DAG tetap utuh, parent yang dibuang di-rewrite dari keturunannya
    /// (squash) sehingga tidak ada referensi menggantung.
    ///
    /// Atomik: semua snapshot yang di-squash ditulis ke temp dulu, dan file
    /// hanya di-commit (rename) bila SELURUH serialisasi berhasil. Gagal
    /// di tengah → tidak ada snapshot yang berubah, DAG tetap konsisten.
    /// Gagal serialize tidak pernah menulis data kosong.
    fn prune_snapshots(&mut self, keep: usize) {
        while self.snapshots.len() > keep {
            let oldest = *self.snapshots.first().unwrap();
            // Kumpulkan rewrite terhadap isi yang sudah stable (copy hasil
            // squash ke memori), bukan menulis satu per satu ke disk.
            let mut rewrites: Vec<(PathBuf, Vec<u8>)> = Vec::new();
            for id in &self.snapshots {
                if *id == oldest {
                    continue;
                }
                if let Some(mut s) = read_snapshot(&self.state_dir(), *id) {
                    if s.parents.contains(&oldest) {
                        s.parents.retain(|p| *p != oldest);
                        let bytes = match bincode::serialize(&s) {
                            Ok(b) => b,
                            Err(_) => return, // gagal serialize → batal, DAG utuh
                        };
                        rewrites.push((snapshot_path(&self.state_dir(), *id), bytes));
                    }
                }
            }
            // Fase komit: tulis temp semua, lalu rename semua (atomik per file).
            for (path, bytes) in &rewrites {
                if let Some(parent) = path.parent() {
                    if std::fs::create_dir_all(parent).is_err() {
                        return;
                    }
                }
                if super::format::write_tmp(path, bytes).is_err() {
                    return;
                }
            }
            for (path, _) in &rewrites {
                let _ = super::format::commit_tmp(path);
            }
            let _ = std::fs::remove_file(snapshot_path(&self.state_dir(), oldest));
            self.snapshots.retain(|x| *x != oldest);
        }
    }

    /// Rollback ke snapshot `id`: restore metadata + graph + verify + symbols.
    /// AST cache dibiarkan (objek yang tidak cocok diabaikan otomatis).
    pub fn rollback(&mut self, id: u64) -> io::Result<()> {
        let _lock = acquire_write_lock(
            &self.lock_path(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        let path = snapshot_path(&self.state_dir(), id);
        let bytes = std::fs::read(&path)?;
        let snap: Snapshot = bincode::deserialize(&bytes).map_err(io::Error::other)?;
        self.files = snap
            .files
            .into_iter()
            .map(|m| (m.path.clone(), m))
            .collect();
        self.graph = snap.graph;
        self.verify = snap.verify.into_iter().collect();
        // Fase 0: rollback mengganti verify map — indeks turunan harus
        // dibangun ulang, kalau tidak reuse_verify() gagal walau entry ada.
        self.verify_ast_index.clear();
        self.verify_semantic_index.clear();
        self.verify_accessed.clear();
        for v in self.verify.values() {
            if v.ast_hash != 0 {
                self.verify_ast_index.insert(v.ast_hash, v.content_hash);
            }
            if v.semantic_hash != 0 {
                self.verify_semantic_index
                    .insert(v.semantic_hash, v.content_hash);
            }
            self.verify_accessed.insert(v.content_hash, v.verified_at_ns);
        }
        self.symbols = snap.symbols;
        self.diags = snap
            .diags
            .into_iter()
            .map(|d| (d.path.clone(), d))
            .collect();
        self.flags_hash = snap.flags_hash;
        // Fase 0 (Kritik B): rollback mengubah metadata + graph + verify +
        // symbol + diag — tandai SEMUA store terkait, bukan hanya dirty
        // metadata. Sebelumnya hanya dirty=true sehingga graph/verify/diag
        // hasil rollback hilang pada save() berikutnya.
        self.dirty = true;
        self.dirty_graph = true;
        self.dirty_verify = true;
        self.dirty_symbol = true;
        self.dirty_diag = true;
        Ok(())
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_roundtrip() {
        let mut s = Snapshot {
            id: 1,
            ..Default::default()
        };
        s.files.push(FileMeta {
            path: PathBuf::from("a.sv"),
            content_hash: 5,
            mtime_ns: 0,
            size: 10,
            status: super::super::metadata::FileStatus::Unchanged,
            flags_hash: 0,
            deps: vec![],
            include_hashes: vec![],
            compiled_at_ns: 0,
            ast_format_version: 1,
        });
        let bytes = bincode::serialize(&s).unwrap();
        let s2: Snapshot = bincode::deserialize(&bytes).unwrap();
        assert_eq!(s2.files[0].content_hash, 5);
    }

    // ── Kritik 13 db.md: Snapshot DAG ──

    fn dag_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mivon_micd_snap_dag_{}_{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("snapshots")).unwrap();
        dir
    }

    fn write_snap(root: &Path, id: u64, parents: Vec<u64>) {
        let s = Snapshot {
            id,
            parents,
            ..Snapshot::default()
        };
        std::fs::write(snapshot_path(root, id), bincode::serialize(&s).unwrap()).unwrap();
    }

    #[test]
    fn test_history_and_parents() {
        let root = dag_root("history");
        // DAG: 1 → 2 → 4, dan 1 → 3 → 4 (merge).
        write_snap(&root, 1, vec![]);
        write_snap(&root, 2, vec![1]);
        write_snap(&root, 3, vec![1]);
        write_snap(&root, 4, vec![2, 3]);

        assert_eq!(parents_of(&root, 4).unwrap(), vec![2, 3]);
        assert_eq!(parents_of(&root, 1).unwrap(), Vec::<u64>::new());

        let mut hist = history_of(&root, 4);
        hist.sort_unstable();
        assert_eq!(hist, vec![1, 2, 3, 4], "riwayat mencakup kedua cabang");

        // Merge-base 4 dan 2 = 2.
        assert_eq!(merge_base(&root, 4, 2), Some(2));
        // Merge-base 4 dan 3 = 3.
        assert_eq!(merge_base(&root, 4, 3), Some(3));
        // Merge-base 2 dan 3 = 1 (common ancestor).
        assert_eq!(merge_base(&root, 2, 3), Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_read_missing_returns_none() {
        let root = dag_root("missing");
        assert!(read_snapshot(&root, 99).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
