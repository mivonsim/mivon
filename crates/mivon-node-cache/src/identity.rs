//! Identitas komputasi node_cache.
//!
//! Dua hash yang SENGAJA dipisah (node_cache.md §3):
//!
//! * [`ContentId`] — identitas isi artefak: `BLAKE3(artifact_bytes)`.
//!   Dua artefak dengan isi identik berbagi identitas yang sama, walau berasal
//!   dari project berbeda.
//! * [`NodeIdentity`] — identitas proses komputasi: stage + versi stage +
//!   input + dependency + konfigurasi + toolchain + target + skema output.
//!
//! Node menunjuk ke object; object tidak tahu node mana yang memakainya.

use std::fmt;

/// Panjang hash (BLAKE3-256).
pub const HASH_LEN: usize = 32;

/// Algoritma hash yang dipakai format penyimpanan.
pub const HASH_ALGO: &str = "blake3-256";

/// Identitas isi artefak (content hash).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentId(pub [u8; HASH_LEN]);

impl ContentId {
    /// Hash isi artefak.
    pub fn of(bytes: &[u8]) -> Self {
        ContentId(*blake3::hash(bytes).as_bytes())
    }

    /// Content id dari hex 64 karakter.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != HASH_LEN * 2 {
            return None;
        }
        let mut out = [0u8; HASH_LEN];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = hex_val(chunk[0])?;
            let lo = hex_val(chunk[1])?;
            out[i] = (hi << 4) | lo;
        }
        Some(ContentId(out))
    }

    /// Representasi hex 64 karakter (nama file object/node).
    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(HASH_LEN * 2);
        for b in self.0 {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        s
    }

    pub fn as_bytes(&self) -> &[u8; HASH_LEN] {
        &self.0
    }

    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|b| *b == 0)
    }

    /// Prefix hex untuk logging.
    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }
}

impl fmt::Debug for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentId({})", self.short())
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Identitas proses komputasi (node id).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeIdentity(pub ContentId);

impl NodeIdentity {
    pub fn as_content(&self) -> ContentId {
        self.0
    }

    pub fn to_hex(self) -> String {
        self.0.to_hex()
    }

    pub fn short(&self) -> String {
        self.0.short()
    }
}

impl fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeIdentity({})", self.short())
    }
}

impl fmt::Display for NodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_hex())
    }
}

/// Input yang menentukan identitas satu node (node_cache.md §3B):
///
/// ```text
/// NODE_ID = BLAKE3(
///     stage_identity + stage_version + input_content_ids
///     + dependency_output_ids + configuration_id
///     + toolchain_id + target_id + output_schema
/// )
/// ```
#[derive(Clone, Copy, Debug)]
pub struct NodeFingerprint<'a> {
    pub stage: &'a str,
    pub stage_version: u32,
    pub key: &'a [u8],
    pub inputs: &'a [ContentId],
    pub dependencies: &'a [(NodeIdentity, ContentId)],
    pub config: u64,
    pub toolchain: &'a str,
    pub target: &'a str,
    pub schema: u32,
}

impl<'a> NodeFingerprint<'a> {
    /// Hitung node id deterministik dari fingerprint.
    ///
    /// Setiap field diberi panjang eksplisit (length-prefixed) agar batas
    /// antar-field tidak ambigu (`"ab"+"c"` != `"a"+"bc"`).
    pub fn id(&self) -> NodeIdentity {
        let mut h = blake3::Hasher::new();
        h.update(b"mivon-node\x00\x01");
        put_str(&mut h, self.stage);
        h.update(&self.stage_version.to_le_bytes());
        put_bytes(&mut h, self.key);
        h.update(&(self.inputs.len() as u64).to_le_bytes());
        for id in self.inputs {
            h.update(id.as_bytes());
        }
        h.update(&(self.dependencies.len() as u64).to_le_bytes());
        for (node, out) in self.dependencies {
            h.update(node.0.as_bytes());
            h.update(out.as_bytes());
        }
        h.update(&self.config.to_le_bytes());
        put_str(&mut h, self.toolchain);
        put_str(&mut h, self.target);
        h.update(&self.schema.to_le_bytes());
        NodeIdentity(ContentId(*h.finalize().as_bytes()))
    }

    /// Hash 64-bit dari key — dipakai graph snapshot untuk lookup cepat tanpa
    /// menyimpan key mentah di graph.
    pub fn key_hash(&self) -> u64 {
        let mut h = blake3::Hasher::new();
        h.update(b"key\0");
        h.update(self.key);
        u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap())
    }
}

/// Fingerprint untuk node tanpa input/dependency eksplisit.
pub fn fingerprint(
    stage: &str,
    stage_version: u32,
    key: &[u8],
    inputs: &[ContentId],
    config: u64,
    toolchain: &str,
    target: &str,
    schema: u32,
) -> NodeIdentity {
    NodeFingerprint {
        stage,
        stage_version,
        key,
        inputs,
        dependencies: &[],
        config,
        toolchain,
        target,
        schema,
    }
    .id()
}

fn put_bytes(h: &mut blake3::Hasher, data: &[u8]) {
    h.update(&(data.len() as u64).to_le_bytes());
    h.update(data);
}

fn put_str(h: &mut blake3::Hasher, s: &str) {
    put_bytes(h, s.as_bytes());
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_id_is_content_addressed() {
        let a = ContentId::of(b"hello");
        let b = ContentId::of(b"hello");
        let c = ContentId::of(b"hello!");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(ContentId::from_hex(&a.to_hex()), Some(a));
    }

    #[test]
    fn content_id_rejects_bad_hex() {
        assert!(ContentId::from_hex("zz").is_none());
        assert!(ContentId::from_hex("").is_none());
    }

    #[test]
    fn node_id_depends_on_every_field() {
        let base = fingerprint("parse", 1, b"a.sv", &[], 0, "v1", "sv", 1);
        assert_eq!(base, fingerprint("parse", 1, b"a.sv", &[], 0, "v1", "sv", 1));
        assert_ne!(base, fingerprint("parse", 2, b"a.sv", &[], 0, "v1", "sv", 1));
        assert_ne!(base, fingerprint("elab", 1, b"a.sv", &[], 0, "v1", "sv", 1));
        assert_ne!(base, fingerprint("parse", 1, b"b.sv", &[], 0, "v1", "sv", 1));
        assert_ne!(base, fingerprint("parse", 1, b"a.sv", &[], 7, "v1", "sv", 1));
        assert_ne!(base, fingerprint("parse", 1, b"a.sv", &[], 0, "v2", "sv", 1));
        assert_ne!(base, fingerprint("parse", 1, b"a.sv", &[], 0, "v1", "vhdl", 1));
        assert_ne!(base, fingerprint("parse", 1, b"a.sv", &[], 0, "v1", "sv", 2));
        assert_ne!(
            base,
            fingerprint(
                "parse",
                1,
                b"a.sv",
                &[ContentId::of(b"x")],
                0,
                "v1",
                "sv",
                1
            )
        );
    }

    #[test]
    fn fingerprint_fields_are_unambiguous() {
        let a = fingerprint("ab", 1, b"c", &[], 0, "t", "g", 1);
        let b = fingerprint("a", 1, b"bc", &[], 0, "t", "g", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn key_hash_is_stable() {
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
        assert_eq!(fp.key_hash(), fp.key_hash());
    }
}
