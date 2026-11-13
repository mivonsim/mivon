//! Transaksi & atomic commit (node_cache.md §9).
//!
//! Urutan publikasi:
//! 1. hitung output, 2. tulis object (atomic), 3. tulis manifest (atomic),
//! 4. tulis graph snapshot (atomic), 5. catat journal, 6. pindahkan reference,
//! 7. hapus journal.
//!
//! Journal ditulis SEBELUM reference dipindahkan; recovery memakai journal
//! untuk memastikan crash tidak menghasilkan reference ke graph setengah jadi.

use crate::filesystem::{atomic_write, unique_suffix, Layout};
use crate::format::{Decoder, Encoder, MAGIC_TXN};
use crate::graph::{GraphStore, RefRecord};
use crate::identity::ContentId;
use std::io;
use std::path::PathBuf;

/// Isi journal transaksi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journal {
    pub id: String,
    pub project: String,
    pub graph: ContentId,
    pub generation: u64,
}

impl Journal {
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Encoder::new(&MAGIC_TXN);
        e.text(&self.id);
        e.text(&self.project);
        e.id(&self.graph);
        e.u64(self.generation);
        e.finish()
    }

    pub fn decode(data: &[u8]) -> Option<Journal> {
        let mut d = Decoder::open(data, &MAGIC_TXN)?;
        let id = d.text()?;
        let project = d.text()?;
        let graph = d.id()?;
        let generation = d.u64()?;
        d.finish()?;
        Some(Journal {
            id,
            project,
            graph,
            generation,
        })
    }
}

/// Transaksi publikasi graph.
pub struct Transaction {
    layout: Layout,
    journal: Journal,
    path: PathBuf,
    committed: bool,
}

impl Transaction {
    /// Mulai transaksi: tulis journal durable sebelum reference dipindahkan.
    pub fn begin(
        layout: &Layout,
        project: &str,
        graph: ContentId,
        generation: u64,
    ) -> io::Result<Transaction> {
        let id = format!("{}-{}", project, unique_suffix());
        let journal = Journal {
            id: id.clone(),
            project: project.to_string(),
            graph,
            generation,
        };
        let path = layout.transaction_path(&id);
        atomic_write(&path, &journal.encode())?;
        Ok(Transaction {
            layout: layout.clone(),
            journal,
            path,
            committed: false,
        })
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Selesaikan transaksi: pindahkan reference secara atomik lalu hapus
    /// journal.
    pub fn commit(mut self) -> io::Result<()> {
        GraphStore::new(&self.layout).write_ref(&RefRecord {
            project: self.journal.project.clone(),
            graph: self.journal.graph,
            generation: self.journal.generation,
        })?;
        let _ = std::fs::remove_file(&self.path);
        self.committed = true;
        Ok(())
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.committed {
            // Abort: journal dibuang; reference TIDAK disentuh sehingga tetap
            // menunjuk graph lama yang valid.
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Journal yang belum selesai (untuk recovery).
pub fn pending(layout: &Layout) -> Vec<(PathBuf, Journal)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(layout.transactions_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        if let Some(journal) = Journal::decode(&data) {
            out.push((path, journal));
        } else {
            // Journal rusak: bukan commit valid → aman dihapus.
            let _ = std::fs::remove_file(&path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::Layout;
    use crate::graph::{GraphSnapshot, RefRecord};

    fn temp_layout(tag: &str) -> (Layout, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mivon-txn-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        (Layout::open(&root).unwrap(), root)
    }

    #[test]
    fn commit_moves_reference_and_clears_journal() {
        let (layout, root) = temp_layout("commit");
        let graphs = GraphStore::new(&layout);
        let graph = graphs
            .write(&GraphSnapshot::new("pid", 1, 0, "v1"))
            .unwrap();
        let txn = Transaction::begin(&layout, "pid", graph, 1).unwrap();
        assert_eq!(pending(&layout).len(), 1);
        txn.commit().unwrap();
        assert!(pending(&layout).is_empty());
        assert_eq!(
            graphs.read_ref("pid").unwrap(),
            RefRecord {
                project: "pid".into(),
                graph,
                generation: 1
            }
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn abort_leaves_old_reference() {
        let (layout, root) = temp_layout("abort");
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
        let new = graphs
            .write(&GraphSnapshot::new("pid", 2, 0, "v1"))
            .unwrap();
        {
            let _txn = Transaction::begin(&layout, "pid", new, 2).unwrap();
            // drop tanpa commit
        }
        assert_eq!(graphs.read_ref("pid").unwrap().graph, old);
        assert!(pending(&layout).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
