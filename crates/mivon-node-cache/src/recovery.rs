//! Recovery setelah crash (node_cache.md §9C).
//!
//! Aturan:
//! * artefak sementara (`tmp/`, `.tmp-*`) dibersihkan;
//! * journal yang graph-nya sudah dirujuk = commit selesai → journal dibuang;
//! * journal yang graph-nya lengkap tapi reference belum dipindahkan →
//!   commit diselesaikan (idempoten);
//! * journal yang graph-nya tidak lengkap → dibuang, reference lama tetap.

use crate::filesystem::Layout;
use crate::graph::GraphStore;
use crate::manifest::ManifestStore;
use crate::object::ObjectStore;
use crate::transaction;

/// Ringkasan hasil recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Journal yang commit-nya diselesaikan.
    pub journals_completed: usize,
    /// Journal yang dibuang (tidak lengkap).
    pub journals_discarded: usize,
    /// File sementara yang dibersihkan.
    pub tmp_removed: usize,
}

impl RecoveryReport {
    pub fn is_empty(&self) -> bool {
        self.journals_completed == 0 && self.journals_discarded == 0 && self.tmp_removed == 0
    }
}

/// Jalankan recovery idempoten pada sebuah layout.
pub fn recover(layout: &Layout) -> RecoveryReport {
    let mut report = RecoveryReport::default();
    let graphs = GraphStore::new(layout);
    let manifests = ManifestStore::new(layout);
    let objects = ObjectStore::new(layout);

    for (path, journal) in transaction::pending(layout) {
        let ref_matches = graphs
            .read_ref(&journal.project)
            .map(|r| r.graph == journal.graph)
            .unwrap_or(false);
        if ref_matches {
            // Commit sudah selesai; journal tersisa karena crash saat unlink.
            let _ = std::fs::remove_file(&path);
            continue;
        }

        let graph_ok = graphs.read(&journal.graph).is_some_and(|g| {
            g.entries.iter().all(|entry| {
                manifests.read(&entry.node).is_some_and(|m| {
                    m.outputs
                        .iter()
                        .all(|out| objects.has_valid(&out.content, out.size))
                })
            })
        });

        if graph_ok {
            let ok = graphs.write_ref(&crate::graph::RefRecord {
                project: journal.project.clone(),
                graph: journal.graph,
                generation: journal.generation,
            });
            if ok.is_ok() {
                report.journals_completed += 1;
                let _ = std::fs::remove_file(&path);
                continue;
            }
        }

        report.journals_discarded += 1;
        let _ = std::fs::remove_file(&path);
    }

    // Bersihkan file tmp (artefak yang tidak pernah di-rename).
    if let Ok(entries) = std::fs::read_dir(layout.tmp_dir()) {
        for entry in entries.flatten() {
            if std::fs::remove_file(entry.path()).is_ok() {
                report.tmp_removed += 1;
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;
    use crate::graph::{GraphSnapshot, RefRecord};
    use crate::identity::{fingerprint, NodeIdentity};
    use crate::manifest::{publish, NodeManifest, OutputRef};
    use crate::object::ObjectStore;

    fn temp_layout(tag: &str) -> (Layout, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mivon-recovery-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        (Layout::open(&root).unwrap(), root)
    }

    fn node_with_artifact(layout: &Layout) -> (NodeIdentity, crate::identity::ContentId) {
        let objects = ObjectStore::new(layout);
        let (content, size, _) = objects.put(b"artifact").unwrap();
        let id = fingerprint("parse", 1, b"a.sv", &[], 0, "v1", "sv", 1);
        let mut m = NodeManifest::new(id, "parse", 1, b"a.sv");
        m.complete = true;
        m.outputs.push(OutputRef {
            kind: "blob".into(),
            content,
            size,
            schema: 1,
        });
        publish(layout, &m).unwrap();
        (id, content)
    }

    #[test]
    fn incomplete_journal_is_discarded_and_old_ref_kept() {
        let (layout, root) = temp_layout("incomplete");
        let graphs = GraphStore::new(&layout);
        let old = graphs
            .write(&GraphSnapshot::new("pid", 1, 0, "v1"))
            .unwrap();
        graphs
            .write_ref(&RefRecord {
                project: "pid".into(),
                graph: old,
                generation: 1,
            })
            .unwrap();
        let bogus = crate::identity::ContentId::of(b"no-such-graph");
        let _txn = transaction::Transaction::begin(&layout, "pid", bogus, 2).unwrap();
        // Simulasikan crash: journal ditulis, commit tidak.
        std::mem::forget(_txn);

        let report = recover(&layout);
        assert_eq!(report.journals_discarded, 1);
        assert_eq!(graphs.read_ref("pid").unwrap().graph, old);
        assert!(transaction::pending(&layout).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn complete_journal_finishes_commit() {
        let (layout, root) = temp_layout("complete");
        let graphs = GraphStore::new(&layout);
        let (node, content) = node_with_artifact(&layout);
        let mut g = GraphSnapshot::new("pid", 1, 0, "v1");
        g.entries.push(crate::graph::GraphEntry {
            node,
            stage: "parse".into(),
            key_hash: 1,
        });
        let graph = graphs.write(&g).unwrap();
        let _txn = transaction::Transaction::begin(&layout, "pid", graph, 1).unwrap();
        std::mem::forget(_txn);

        let report = recover(&layout);
        assert_eq!(report.journals_completed, 1);
        let record = graphs.read_ref("pid").unwrap();
        assert_eq!(record.graph, graph);
        assert_eq!(record.generation, 1);
        let _ = content;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tmp_dir_is_cleaned() {
        let (layout, root) = temp_layout("tmp");
        std::fs::write(layout.tmp_dir().join("leftover"), b"x").unwrap();
        let report = recover(&layout);
        assert_eq!(report.tmp_removed, 1);
        assert!(recover(&layout).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
