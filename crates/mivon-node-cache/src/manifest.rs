//! Manifest node: metadata immutable yang menjelaskan input, dependency, dan
//! output satu komputasi (node_cache.md §4).
//!
//! Manifest tidak pernah dipublikasikan sebelum seluruh artefak outputnya
//! tersedia (lihat `transaction.rs`).

use crate::filesystem::{atomic_write, Layout};
use crate::format::{Decoder, Encoder, MAGIC_NODE, MAX_ENTRIES};
use crate::identity::{ContentId, NodeIdentity};
use std::io;
use std::path::Path;

/// Referensi satu artefak output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRef {
    /// Jenis artefak (mis. `ast`, `preproc`, `ir`, `blob:lint`).
    pub kind: String,
    pub content: ContentId,
    pub size: u64,
    /// Skema payload (versi bincode/format) — artefak beda skema ditolak.
    pub schema: u32,
}

/// Manifest satu node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeManifest {
    pub node: NodeIdentity,
    pub stage: String,
    pub stage_version: u32,
    /// Key komputasi (path file, nama module, `"last"`, …).
    pub key: Vec<u8>,
    pub config: u64,
    pub toolchain: String,
    pub target: String,
    pub schema: u32,
    pub inputs: Vec<ContentId>,
    pub dependencies: Vec<(NodeIdentity, ContentId)>,
    pub outputs: Vec<OutputRef>,
    /// `false` = komputasi belum selesai/commit tidak lengkap → tidak boleh hit.
    pub complete: bool,
}

impl NodeManifest {
    pub fn new(
        node: NodeIdentity,
        stage: impl Into<String>,
        stage_version: u32,
        key: impl Into<Vec<u8>>,
    ) -> Self {
        NodeManifest {
            node,
            stage: stage.into(),
            stage_version,
            key: key.into(),
            config: 0,
            toolchain: String::new(),
            target: String::new(),
            schema: 0,
            inputs: Vec::new(),
            dependencies: Vec::new(),
            outputs: Vec::new(),
            complete: true,
        }
    }

    pub fn with_config(mut self, config: u64) -> Self {
        self.config = config;
        self
    }

    pub fn with_toolchain(mut self, toolchain: impl Into<String>) -> Self {
        self.toolchain = toolchain.into();
        self
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = target.into();
        self
    }

    pub fn with_schema(mut self, schema: u32) -> Self {
        self.schema = schema;
        self
    }

    pub fn with_inputs(mut self, inputs: Vec<ContentId>) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_dependencies(mut self, deps: Vec<(NodeIdentity, ContentId)>) -> Self {
        self.dependencies = deps;
        self
    }

    pub fn with_output(mut self, output: OutputRef) -> Self {
        self.outputs.push(output);
        self
    }

    /// Output dengan jenis tertentu.
    pub fn output(&self, kind: &str) -> Option<&OutputRef> {
        self.outputs.iter().find(|o| o.kind == kind)
    }

    /// Encode ke format biner stabil.
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Encoder::new(&MAGIC_NODE);
        e.id(&self.node.0);
        e.text(&self.stage);
        e.u32(self.stage_version);
        e.bytes(&self.key);
        e.u64(self.config);
        e.text(&self.toolchain);
        e.text(&self.target);
        e.u32(self.schema);
        e.u64(self.inputs.len() as u64);
        for id in &self.inputs {
            e.id(id);
        }
        e.u64(self.dependencies.len() as u64);
        for (node, out) in &self.dependencies {
            e.id(&node.0);
            e.id(out);
        }
        e.u64(self.outputs.len() as u64);
        for out in &self.outputs {
            e.text(&out.kind);
            e.id(&out.content);
            e.u64(out.size);
            e.u32(out.schema);
        }
        e.u8(u8::from(self.complete));
        e.finish()
    }

    /// Decode dari format biner. `None` bila rusak/tidak konsisten.
    pub fn decode(data: &[u8]) -> Option<NodeManifest> {
        let mut d = Decoder::open(data, &MAGIC_NODE)?;
        let node = NodeIdentity(d.id()?);
        let stage = d.text()?;
        let stage_version = d.u32()?;
        let key = d.bytes()?.to_vec();
        let config = d.u64()?;
        let toolchain = d.text()?;
        let target = d.text()?;
        let schema = d.u32()?;
        let n_inputs = d.u64()? as usize;
        if n_inputs > MAX_ENTRIES {
            return None;
        }
        let mut inputs = Vec::with_capacity(n_inputs.min(1024));
        for _ in 0..n_inputs {
            inputs.push(d.id()?);
        }
        let n_deps = d.u64()? as usize;
        if n_deps > MAX_ENTRIES {
            return None;
        }
        let mut dependencies = Vec::with_capacity(n_deps.min(1024));
        for _ in 0..n_deps {
            dependencies.push((NodeIdentity(d.id()?), d.id()?));
        }
        let n_out = d.u64()? as usize;
        if n_out > MAX_ENTRIES {
            return None;
        }
        let mut outputs = Vec::with_capacity(n_out.min(1024));
        for _ in 0..n_out {
            outputs.push(OutputRef {
                kind: d.text()?,
                content: d.id()?,
                size: d.u64()?,
                schema: d.u32()?,
            });
        }
        let complete = d.u8()? != 0;
        d.finish()?;
        Some(NodeManifest {
            node,
            stage,
            stage_version,
            key,
            config,
            toolchain,
            target,
            schema,
            inputs,
            dependencies,
            outputs,
            complete,
        })
    }
}

/// Store manifest di `<root>/nodes/`.
pub struct ManifestStore<'a> {
    layout: &'a Layout,
}

impl<'a> ManifestStore<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        ManifestStore { layout }
    }

    pub fn exists(&self, id: &NodeIdentity) -> bool {
        self.layout.node_path(id).is_file()
    }

    /// Baca manifest; validasi checksum dan konsistensi node id.
    pub fn read(&self, id: &NodeIdentity) -> Option<NodeManifest> {
        let data = std::fs::read(self.layout.node_path(id)).ok()?;
        let m = NodeManifest::decode(&data)?;
        // Node id di manifest wajib sama dengan nama file — cegah file nyasar.
        (m.node == *id).then_some(m)
    }

    /// Publikasikan manifest secara atomik.
    pub fn write(&self, manifest: &NodeManifest) -> io::Result<usize> {
        atomic_write(
            &self.layout.node_path(&manifest.node),
            &manifest.encode(),
        )
    }

    /// Path fisik manifest (untuk GC).
    pub fn path(&self, id: &NodeIdentity) -> std::path::PathBuf {
        self.layout.node_path(id)
    }

    /// Semua manifest (dipakai recovery/GC).
    pub fn iter(&self) -> Vec<NodeManifest> {
        let mut out = Vec::new();
        crate::filesystem::walk_sharded(&self.layout.nodes_dir(), |path, name| {
            if let Some(id) = ContentId::from_hex(name) {
                if let Some(m) = self.read(&NodeIdentity(id)) {
                    out.push(m);
                }
            } else {
                // File sampah non-hash di nodes/ — biarkan GC yang membersihkan.
                let _ = path;
            }
        });
        out
    }

    pub fn path_for_key(&self, id: &NodeIdentity) -> std::path::PathBuf {
        self.layout.node_path(id)
    }
}

/// Tulis manifest lengkap dengan artefak output yang dijamin sudah ada.
///
/// Manifest tidak boleh dipublikasikan bila artefak output belum tersedia
/// (node_cache.md §9B). Caller wajib menulis object lebih dulu; fungsi ini
/// memverifikasi keberadaan file object.
pub fn publish(layout: &Layout, manifest: &NodeManifest) -> io::Result<()> {
    for out in &manifest.outputs {
        let path = layout.object_path(&out.content);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() == out.size => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "artefak {} ({}) belum tersedia — manifest tidak dipublikasikan",
                        out.kind,
                        out.content.short()
                    ),
                ));
            }
        }
    }
    ManifestStore::new(layout).write(manifest).map(|_| ())
}

/// Baca manifest dari path mentah (helper recovery).
pub fn read_file(path: &Path) -> Option<NodeManifest> {
    NodeManifest::decode(&std::fs::read(path).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::fingerprint;
    use std::path::PathBuf;

    fn sample() -> NodeManifest {
        let node = fingerprint("parse", 1, b"a.sv", &[ContentId::of(b"src")], 7, "v1", "sv", 1);
        NodeManifest::new(node, "parse", 1, b"a.sv")
            .with_config(7)
            .with_toolchain("v1")
            .with_target("sv")
            .with_schema(1)
            .with_inputs(vec![ContentId::of(b"src")])
            .with_output(OutputRef {
                kind: "ast".into(),
                content: ContentId::of(b"ast-bytes"),
                size: 9,
                schema: 1,
            })
    }

    #[test]
    fn manifest_roundtrip_all_fields() {
        let m = sample();
        let bytes = m.encode();
        let m2 = NodeManifest::decode(&bytes).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn manifest_corruption_rejected() {
        let mut bytes = sample().encode();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xAA;
        assert!(NodeManifest::decode(&bytes).is_none());
    }

    #[test]
    fn output_lookup() {
        let m = sample();
        assert!(m.output("ast").is_some());
        assert!(m.output("ir").is_none());
    }

    fn temp_layout(tag: &str) -> (Layout, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mivon-manifest-{}-{}-{}",
            tag,
            std::process::id(),
            crate::filesystem::unique_suffix()
        ));
        (Layout::open(&root).unwrap(), root)
    }

    #[test]
    fn publish_requires_artifact_first() {
        let (layout, root) = temp_layout("publish");
        let m = sample();
        let err = publish(&layout, &m).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

        let out = m.outputs[0].clone();
        atomic_write(&layout.object_path(&out.content), &vec![0u8; out.size as usize]).unwrap();
        publish(&layout, &m).unwrap();
        let store = ManifestStore::new(&layout);
        assert_eq!(store.read(&m.node).unwrap().node, m.node);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mismatched_node_id_is_rejected() {
        let (layout, root) = temp_layout("id");
        let m = sample();
        let mut wrong = m.clone();
        wrong.node = fingerprint("parse", 1, b"other", &[], 0, "", "", 0);
        atomic_write(&layout.node_path(&m.node), &wrong.encode()).unwrap();
        assert!(ManifestStore::new(&layout).read(&m.node).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
