//! Resolver node: cache hit/miss + validasi artefak (node_cache.md §5-§7).
//!
//! Lookup langsung dari identitas komputasi ke path manifest (tanpa index
//! database). Hit hanya bila SELURUH lapisan validasi lolos.

use crate::filesystem::Layout;
use crate::identity::{ContentId, NodeFingerprint, NodeIdentity};
use crate::manifest::{ManifestStore, NodeManifest};
use crate::object::ObjectStore;

/// Node yang berhasil di-resolve beserta manifest-nya.
#[derive(Debug, Clone)]
pub struct ResolvedNode {
    pub manifest: NodeManifest,
}

impl ResolvedNode {
    /// Ambil artefak output dengan jenis tertentu, memverifikasi ukuran +
    /// checksum content-addressed.
    pub fn bytes(&self, layout: &Layout, kind: &str) -> Option<Vec<u8>> {
        let out = self.manifest.output(kind)?;
        let store = ObjectStore::new(layout);
        if !store.has_valid(&out.content, out.size) {
            return None;
        }
        store.get(&out.content)
    }
}

/// Resolver: dari fingerprint ke node valid.
pub struct NodeResolver<'a> {
    layout: &'a Layout,
}

impl<'a> NodeResolver<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        NodeResolver { layout }
    }

    /// Identitas yang akan dipakai untuk fingerprint ini.
    pub fn identity(&self, fp: &NodeFingerprint<'_>) -> NodeIdentity {
        fp.id()
    }

    /// Cari node valid. `None` = MISS (hitung ulang).
    pub fn resolve(&self, fp: &NodeFingerprint<'_>) -> Option<ResolvedNode> {
        self.resolve_id(&fp.id(), fp)
    }

    /// Resolve berdasarkan node id + fingerprint yang diharapkan.
    pub fn resolve_id(
        &self,
        id: &NodeIdentity,
        fp: &NodeFingerprint<'_>,
    ) -> Option<ResolvedNode> {
        let manifest = ManifestStore::new(self.layout).read(id)?;
        if !self.valid_manifest(&manifest, fp) {
            return None;
        }
        let objects = ObjectStore::new(self.layout);
        for out in &manifest.outputs {
            if !objects.has_valid(&out.content, out.size) {
                return None;
            }
        }
        Some(ResolvedNode { manifest })
    }

    /// Lapisan 3+4: implementasi stage cocok dan artefak output valid.
    pub fn valid_manifest(&self, m: &NodeManifest, fp: &NodeFingerprint<'_>) -> bool {
        m.complete
            && m.stage == fp.stage
            && m.stage_version == fp.stage_version
            && m.key == fp.key
            && m.config == fp.config
            && m.toolchain == fp.toolchain
            && m.target == fp.target
            && m.schema == fp.schema
            && m.inputs == fp.inputs
            && m.dependencies == fp.dependencies
    }

    /// Apakah artefak dengan content id ini ada dan valid.
    pub fn artifact_ok(&self, id: &ContentId, size: u64) -> bool {
        ObjectStore::new(self.layout).has_valid(id, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{unique_suffix, Layout};
    use crate::identity::fingerprint;
    use crate::manifest::{publish, OutputRef};
    use crate::object::ObjectStore;

    fn temp_layout(tag: &str) -> (Layout, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mivon-resolve-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        (Layout::open(&root).unwrap(), root)
    }

    fn publish_node(layout: &Layout, stage: &str, key: &[u8], payload: &[u8]) -> NodeIdentity {
        let objects = ObjectStore::new(layout);
        let (content, size, _) = objects.put(payload).unwrap();
        let id = fingerprint(stage, 1, key, &[], 0, "v1", "sv", 1);
        let mut manifest = NodeManifest::new(id, stage, 1, key)
            .with_toolchain("v1")
            .with_target("sv")
            .with_schema(1)
            .with_output(OutputRef {
                kind: "blob".into(),
                content,
                size,
                schema: 1,
            });
        manifest.complete = true;
        publish(layout, &manifest).unwrap();
        id
    }

    #[test]
    fn hit_and_miss_are_identity_based() {
        let (layout, root) = temp_layout("hit");
        let id = publish_node(&layout, "parse", b"a.sv", b"ast");
        let resolver = NodeResolver::new(&layout);

        let fp = NodeFingerprint {
            stage: "parse",
            stage_version: 1,
            key: b"a.sv",
            inputs: &[],
            dependencies: &[],
            config: 0,
            toolchain: "v1",
            target: "sv",
            schema: 1,
        };
        let hit = resolver.resolve(&fp).expect("hit");
        assert_eq!(hit.manifest.node, id);
        assert_eq!(hit.bytes(&layout, "blob").as_deref(), Some(&b"ast"[..]));

        // Versi stage beda → MISS.
        let fp2 = NodeFingerprint {
            stage_version: 2,
            ..fp
        };
        assert!(resolver.resolve(&fp2).is_none());

        // Config beda → MISS.
        let fp3 = NodeFingerprint { config: 9, ..fp };
        assert!(resolver.resolve(&fp3).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_artifact_forces_miss() {
        let (layout, root) = temp_layout("artifact");
        publish_node(&layout, "parse", b"a.sv", b"ast");
        let resolver = NodeResolver::new(&layout);
        let fp = NodeFingerprint {
            stage: "parse",
            stage_version: 1,
            key: b"a.sv",
            inputs: &[],
            dependencies: &[],
            config: 0,
            toolchain: "v1",
            target: "sv",
            schema: 1,
        };
        assert!(resolver.resolve(&fp).is_some());
        // Hapus artefak → manifest ada tapi tidak boleh hit.
        let store = ObjectStore::new(&layout);
        let manifest = ManifestStore::new(&layout).read(&fp.id()).unwrap();
        std::fs::remove_file(store.path(&manifest.outputs[0].content)).unwrap();
        assert!(resolver.resolve(&fp).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn incomplete_manifest_is_not_reused() {
        let (layout, root) = temp_layout("incomplete");
        let id = publish_node(&layout, "parse", b"a.sv", b"ast");
        let store = ManifestStore::new(&layout);
        let mut m = store.read(&id).unwrap();
        m.complete = false;
        store.write(&m).unwrap();
        let resolver = NodeResolver::new(&layout);
        let fp = NodeFingerprint {
            stage: "parse",
            stage_version: 1,
            key: b"a.sv",
            inputs: &[],
            dependencies: &[],
            config: 0,
            toolchain: "v1",
            target: "sv",
            schema: 1,
        };
        assert!(resolver.resolve(&fp).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
