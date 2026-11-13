//! Garbage collection mark-and-sweep (node_cache.md §9D).
//!
//! 1. Baca seluruh reference project.
//! 2. Tandai graph + node yang masih dirujuk.
//! 3. Tandai seluruh artefak turunannya.
//! 4. Hapus object/node/graph yang tidak dirujuk setelah memastikan tidak ada
//!    transaksi yang belum selesai.
//!
//! Graph lama dipertahankan selama masih dirujuk `refs/` (rollback), tetapi
//! graph yatim (mis. hasil commit yang gagal) dibersihkan.

use crate::filesystem::{walk_sharded, Layout};
use crate::graph::GraphStore;
use crate::identity::{ContentId, NodeIdentity};
use crate::manifest::ManifestStore;
use crate::object::ObjectStore;
use crate::transaction;
use std::collections::HashSet;
use std::io;

/// Statistik hasil GC.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcStats {
    pub objects_removed: usize,
    pub bytes_freed: u64,
    pub nodes_removed: usize,
    pub graphs_removed: usize,
    pub tmp_removed: usize,
    pub stale_locks_removed: usize,
    /// GC dilewati karena ada transaksi aktif yang belum selesai.
    pub skipped_pending_transactions: usize,
}

/// Jalankan GC. Aman dipanggil kapan pun; transaksi yang belum selesai membuat
/// GC menunda pembersihan agar tidak menghapus artefak di tengah commit.
pub fn collect(layout: &Layout) -> io::Result<GcStats> {
    let mut stats = GcStats::default();

    let pending = transaction::pending(layout);
    if !pending.is_empty() {
        stats.skipped_pending_transactions = pending.len();
        return Ok(stats);
    }

    let graphs = GraphStore::new(layout);
    let manifests = ManifestStore::new(layout);
    let objects = ObjectStore::new(layout);

    // 1-3: mark.
    let mut live_graphs: HashSet<ContentId> = HashSet::new();
    let mut live_nodes: HashSet<NodeIdentity> = HashSet::new();
    let mut live_objects: HashSet<ContentId> = HashSet::new();

    for record in graphs.refs() {
        live_graphs.insert(record.graph);
        if let Some(graph) = graphs.read(&record.graph) {
            for entry in &graph.entries {
                live_nodes.insert(entry.node);
                if let Some(manifest) = manifests.read(&entry.node) {
                    for out in &manifest.outputs {
                        live_objects.insert(out.content);
                    }
                }
            }
        }
    }

    // 4: sweep.
    let mut node_victims: Vec<std::path::PathBuf> = Vec::new();
    walk_sharded(&layout.nodes_dir(), |path, name| {
        let Some(id) = ContentId::from_hex(name) else {
            if name.starts_with(".tmp-") {
                let _ = std::fs::remove_file(path);
            }
            return;
        };
        if !live_nodes.contains(&NodeIdentity(id)) {
            node_victims.push(path.to_path_buf());
        }
    });
    for path in node_victims {
        if std::fs::remove_file(&path).is_ok() {
            stats.nodes_removed += 1;
        }
    }

    let mut graph_victims: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(layout.graphs_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(id) = ContentId::from_hex(&name) else {
                continue;
            };
            if !live_graphs.contains(&id) {
                graph_victims.push(entry.path());
            }
        }
    }
    for path in graph_victims {
        if std::fs::remove_file(&path).is_ok() {
            stats.graphs_removed += 1;
        }
    }

    let (removed, bytes) = objects.sweep(&live_objects);
    stats.objects_removed = removed;
    stats.bytes_freed = bytes;

    if let Ok(entries) = std::fs::read_dir(layout.tmp_dir()) {
        for entry in entries.flatten() {
            if std::fs::remove_file(entry.path()).is_ok() {
                stats.tmp_removed += 1;
            }
        }
    }

    // Lock basi (>1 jam) — worker yang crash meninggalkan `.lock`.
    if let Ok(entries) = std::fs::read_dir(layout.locks_dir()) {
        let now = std::time::SystemTime::now();
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| now.duration_since(t).ok())
                .map(|age| age.as_secs() > 3600)
                .unwrap_or(false);
            if stale && std::fs::remove_file(entry.path()).is_ok() {
                stats.stale_locks_removed += 1;
            }
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;
    use crate::graph::{GraphEntry, GraphSnapshot, RefRecord};
    use crate::identity::fingerprint;
    use crate::manifest::{publish, NodeManifest, OutputRef};

    fn temp_layout(tag: &str) -> (Layout, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mivon-gc-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        (Layout::open(&root).unwrap(), root)
    }

    fn publish_node(layout: &Layout, key: &[u8], payload: &[u8]) -> NodeIdentity {
        let objects = ObjectStore::new(layout);
        let (content, size, _) = objects.put(payload).unwrap();
        let id = fingerprint("parse", 1, key, &[], 0, "v1", "sv", 1);
        let mut m = NodeManifest::new(id, "parse", 1, key);
        m.outputs.push(OutputRef {
            kind: "blob".into(),
            content,
            size,
            schema: 1,
        });
        publish(layout, &m).unwrap();
        id
    }

    #[test]
    fn gc_removes_only_unreferenced() {
        let (layout, root) = temp_layout("sweep");
        let graphs = GraphStore::new(&layout);
        let live = publish_node(&layout, b"live.sv", b"live");
        let dead = publish_node(&layout, b"dead.sv", b"dead");
        let mut g = GraphSnapshot::new("pid", 1, 0, "v1");
        g.entries.push(GraphEntry {
            node: live,
            stage: "parse".into(),
            key_hash: 1,
        });
        let graph = graphs.write(&g).unwrap();
        graphs
            .write_ref(&RefRecord {
                project: "pid".into(),
                graph,
                generation: 1,
            })
            .unwrap();

        let stats = collect(&layout).unwrap();
        assert_eq!(stats.nodes_removed, 1);
        assert_eq!(stats.objects_removed, 1);
        assert!(ManifestStore::new(&layout).read(&live).is_some());
        assert!(ManifestStore::new(&layout).read(&dead).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_skips_when_transaction_pending() {
        let (layout, root) = temp_layout("pending");
        let graphs = GraphStore::new(&layout);
        let graph = graphs
            .write(&GraphSnapshot::new("pid", 1, 0, "v1"))
            .unwrap();
        let txn = transaction::Transaction::begin(&layout, "pid", graph, 1).unwrap();
        std::mem::forget(txn);
        let stats = collect(&layout).unwrap();
        assert_eq!(stats.skipped_pending_transactions, 1);
        assert_eq!(stats.objects_removed, 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
