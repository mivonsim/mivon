//! Graph snapshot immutable + reference project (node_cache.md §5).
//!
//! Graph TIDAK ditulis ulang sebagian: setiap publikasi membuat snapshot baru,
//! lalu `refs/<project-id>` dipindahkan secara atomik setelah seluruh node
//! selesai. Graph lama tetap immutable untuk recovery/rollback.

use crate::filesystem::{atomic_write, Layout};
use crate::format::{Decoder, Encoder, MAGIC_GRAPH, MAGIC_REF, MAX_ENTRIES};
use crate::identity::{ContentId, NodeIdentity};
use std::io;

/// Satu node di dalam snapshot graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEntry {
    pub node: NodeIdentity,
    pub stage: String,
    /// Hash 64-bit key komputasi (path/nama module/`last`).
    pub key_hash: u64,
}

/// Snapshot graph immutable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSnapshot {
    pub project: String,
    pub generation: u64,
    pub flags: u64,
    pub toolchain: String,
    /// Catatan snapshot (mis. `"build: 3 file(s) recompiled"`).
    pub note: String,
    /// Node index project (payload state: file meta, verify, simbol, …).
    pub index: Option<NodeIdentity>,
    pub entries: Vec<GraphEntry>,
}

impl GraphSnapshot {
    pub fn new(project: impl Into<String>, generation: u64, flags: u64, toolchain: impl Into<String>) -> Self {
        GraphSnapshot {
            project: project.into(),
            generation,
            flags,
            toolchain: toolchain.into(),
            note: String::new(),
            index: None,
            entries: Vec::new(),
        }
    }

    pub fn has_node(&self, id: &NodeIdentity) -> bool {
        self.entries.iter().any(|e| e.node == *id)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut e = Encoder::new(&MAGIC_GRAPH);
        e.text(&self.project);
        e.u64(self.generation);
        e.u64(self.flags);
        e.text(&self.toolchain);
        e.text(&self.note);
        e.u8(u8::from(self.index.is_some()));
        if let Some(index) = &self.index {
            e.id(&index.0);
        }
        e.u64(self.entries.len() as u64);
        for entry in &self.entries {
            e.id(&entry.node.0);
            e.text(&entry.stage);
            e.u64(entry.key_hash);
        }
        e.finish()
    }

    pub fn decode(data: &[u8]) -> Option<GraphSnapshot> {
        let mut d = Decoder::open(data, &MAGIC_GRAPH)?;
        let project = d.text()?;
        let generation = d.u64()?;
        let flags = d.u64()?;
        let toolchain = d.text()?;
        let note = d.text()?;
        let index = if d.u8()? != 0 {
            Some(NodeIdentity(d.id()?))
        } else {
            None
        };
        let n = d.u64()? as usize;
        if n > MAX_ENTRIES {
            return None;
        }
        let mut entries = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            entries.push(GraphEntry {
                node: NodeIdentity(d.id()?),
                stage: d.text()?,
                key_hash: d.u64()?,
            });
        }
        d.finish()?;
        Some(GraphSnapshot {
            project,
            generation,
            flags,
            toolchain,
            note,
            index,
            entries,
        })
    }
}

/// Reference project → graph terakhir yang berhasil dipublikasikan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRecord {
    pub project: String,
    pub graph: ContentId,
    pub generation: u64,
}

impl RefRecord {
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Encoder::new(&MAGIC_REF);
        e.text(&self.project);
        e.id(&self.graph);
        e.u64(self.generation);
        e.finish()
    }

    pub fn decode(data: &[u8]) -> Option<RefRecord> {
        let mut d = Decoder::open(data, &MAGIC_REF)?;
        let project = d.text()?;
        let graph = d.id()?;
        let generation = d.u64()?;
        d.finish()?;
        Some(RefRecord {
            project,
            graph,
            generation,
        })
    }
}

/// Store graph + reference.
pub struct GraphStore<'a> {
    layout: &'a Layout,
}

impl<'a> GraphStore<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        GraphStore { layout }
    }

    pub fn exists(&self, id: &ContentId) -> bool {
        self.layout.graph_path(id).is_file()
    }

    /// Baca graph; validasi checksum + id (content-addressed).
    pub fn read(&self, id: &ContentId) -> Option<GraphSnapshot> {
        let data = std::fs::read(self.layout.graph_path(id)).ok()?;
        if ContentId::of(&data) != *id {
            return None;
        }
        GraphSnapshot::decode(&data)
    }

    /// Publikasikan snapshot graph (immutable, content-addressed).
    pub fn write(&self, graph: &GraphSnapshot) -> io::Result<ContentId> {
        let bytes = graph.encode();
        let id = ContentId::of(&bytes);
        if !self.exists(&id) {
            atomic_write(&self.layout.graph_path(&id), &bytes)?;
        }
        Ok(id)
    }

    pub fn read_ref(&self, project: &str) -> Option<RefRecord> {
        let data = std::fs::read(self.layout.ref_path(project)).ok()?;
        let record = RefRecord::decode(&data)?;
        (record.project == project).then_some(record)
    }

    /// Pindahkan reference secara atomik ke graph baru.
    pub fn write_ref(&self, record: &RefRecord) -> io::Result<()> {
        atomic_write(&self.layout.ref_path(&record.project), &record.encode()).map(|_| ())
    }

    /// Graph yang dirujuk project (bila ada dan valid).
    pub fn current(&self, project: &str) -> Option<(ContentId, GraphSnapshot)> {
        let record = self.read_ref(project)?;
        let graph = self.read(&record.graph)?;
        Some((record.graph, graph))
    }

    pub fn remove_ref(&self, project: &str) -> io::Result<()> {
        match std::fs::remove_file(self.layout.ref_path(project)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Semua reference yang ada (untuk GC).
    pub fn refs(&self) -> Vec<RefRecord> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.layout.refs_dir()) else {
            return out;
        };
        for entry in entries.flatten() {
            if let Ok(data) = std::fs::read(entry.path()) {
                if let Some(record) = RefRecord::decode(&data) {
                    out.push(record);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;

    fn temp_layout(tag: &str) -> (Layout, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mivon-graph-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        (Layout::open(&root).unwrap(), root)
    }

    fn node(n: u8) -> NodeIdentity {
        crate::identity::fingerprint("parse", 1, &[n], &[], 0, "v1", "sv", 1)
    }

    #[test]
    fn graph_roundtrip_and_ref() {
        let (layout, root) = temp_layout("roundtrip");
        let store = GraphStore::new(&layout);
        let mut g = GraphSnapshot::new("pid", 1, 7, "v1");
        g.note = "build".into();
        g.entries.push(GraphEntry {
            node: node(1),
            stage: "parse".into(),
            key_hash: 42,
        });
        let id = store.write(&g).unwrap();
        assert_eq!(store.read(&id).unwrap(), g);
        store
            .write_ref(&RefRecord {
                project: "pid".into(),
                graph: id,
                generation: 1,
            })
            .unwrap();
        let (gid, current) = store.current("pid").unwrap();
        assert_eq!(gid, id);
        assert!(current.has_node(&node(1)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupted_graph_is_rejected() {
        let (layout, root) = temp_layout("corrupt");
        let store = GraphStore::new(&layout);
        let g = GraphSnapshot::new("pid", 1, 0, "v1");
        let id = store.write(&g).unwrap();
        let path = layout.graph_path(&id);
        let mut bytes = std::fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF;
        std::fs::write(&path, bytes).unwrap();
        assert!(store.read(&id).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
