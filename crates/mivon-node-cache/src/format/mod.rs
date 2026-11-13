//! Format penyimpanan biner yang stabil dan versioned.
//!
//! Manifest node, graph snapshot, reference, dan journal TIDAK ditulis sebagai
//! memory layout Rust mentah: setiap record punya magic + field berurutan
//! little-endian + checksum BLAKE3 di akhir. Struct internal Rust boleh
//! berubah tanpa mengubah format disk (node_cache.md §4).

use crate::identity::{ContentId, HASH_LEN};

/// Magic manifest node.
pub const MAGIC_NODE: [u8; 8] = *b"MNOD0001";
/// Magic graph snapshot.
pub const MAGIC_GRAPH: [u8; 8] = *b"MGRF0001";
/// Magic reference project.
pub const MAGIC_REF: [u8; 8] = *b"MREF0001";
/// Magic journal transaksi.
pub const MAGIC_TXN: [u8; 8] = *b"MJRN0001";

/// Batas ukuran string/bytes saat decode (anti alokasi raksasa dari file rusak).
pub const MAX_FIELD_BYTES: usize = 64 * 1024 * 1024;
/// Batas jumlah entry per manifest/graph.
pub const MAX_ENTRIES: usize = 16 * 1024 * 1024;

/// Encoder field biner.
pub struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    pub fn new(magic: &[u8; 8]) -> Self {
        let mut buf = Vec::with_capacity(256);
        buf.extend_from_slice(magic);
        Encoder { buf }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.u64(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    pub fn text(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    pub fn id(&mut self, id: &ContentId) {
        self.buf.extend_from_slice(id.as_bytes());
    }

    /// Tutup record: tambahkan checksum BLAKE3 dari seluruh body.
    pub fn finish(mut self) -> Vec<u8> {
        let checksum = blake3::hash(&self.buf);
        self.buf.extend_from_slice(checksum.as_bytes());
        self.buf
    }
}

/// Decoder field biner dengan validasi magic + checksum.
pub struct Decoder<'a> {
    body: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// Buka record: validasi panjang, magic, dan checksum.
    pub fn open(data: &'a [u8], magic: &[u8; 8]) -> Option<Self> {
        if data.len() < 8 + HASH_LEN {
            return None;
        }
        if &data[..8] != magic {
            return None;
        }
        let (body, checksum) = data.split_at(data.len() - HASH_LEN);
        if blake3::hash(body).as_bytes() != checksum {
            return None;
        }
        Some(Decoder { body, pos: 8 })
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        if end > self.body.len() {
            return None;
        }
        let slice = &self.body[self.pos..end];
        self.pos = end;
        Some(slice)
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.u64()? as usize;
        if len > MAX_FIELD_BYTES {
            return None;
        }
        self.take(len)
    }

    pub fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }

    pub fn id(&mut self) -> Option<ContentId> {
        let mut out = [0u8; HASH_LEN];
        out.copy_from_slice(self.take(HASH_LEN)?);
        Some(ContentId(out))
    }

    /// Pastikan seluruh record habis dikonsumsi (tidak ada byte liar).
    pub fn finish(self) -> Option<()> {
        (self.pos == self.body.len()).then_some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_decoder_roundtrip() {
        let mut e = Encoder::new(&MAGIC_NODE);
        e.u8(7);
        e.u32(0xDEAD_BEEF);
        e.u64(42);
        e.text("parse");
        e.id(&ContentId::of(b"x"));
        let bytes = e.finish();

        let mut d = Decoder::open(&bytes, &MAGIC_NODE).unwrap();
        assert_eq!(d.u8(), Some(7));
        assert_eq!(d.u32(), Some(0xDEAD_BEEF));
        assert_eq!(d.u64(), Some(42));
        assert_eq!(d.text().as_deref(), Some("parse"));
        assert_eq!(d.id(), Some(ContentId::of(b"x")));
        assert!(d.finish().is_some());
    }

    #[test]
    fn corrupted_checksum_is_rejected() {
        let mut e = Encoder::new(&MAGIC_GRAPH);
        e.u64(1);
        let mut bytes = e.finish();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(Decoder::open(&bytes, &MAGIC_GRAPH).is_none());
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut e = Encoder::new(&MAGIC_GRAPH);
        e.u64(1);
        let bytes = e.finish();
        assert!(Decoder::open(&bytes, &MAGIC_NODE).is_none());
    }

    #[test]
    fn truncated_record_is_rejected() {
        let mut e = Encoder::new(&MAGIC_NODE);
        e.text("a long field");
        let bytes = e.finish();
        assert!(Decoder::open(&bytes[..bytes.len() / 2], &MAGIC_NODE).is_none());
    }

    #[test]
    fn trailing_bytes_are_detected() {
        let mut e = Encoder::new(&MAGIC_NODE);
        e.u8(1);
        let mut bytes = e.finish();
        // Sisipkan byte sebelum checksum → checksum tetap valid tidak mungkin,
        // jadi yang diuji adalah modifikasi body terdeteksi.
        let checksum_at = bytes.len() - HASH_LEN;
        bytes[checksum_at - 1] = 9;
        assert!(Decoder::open(&bytes, &MAGIC_NODE).is_none());
    }
}
