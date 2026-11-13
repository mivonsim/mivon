//! IEEE 1735 IP encryption foundation — stage 1 of FEAT-13
//! (IP encryption IEEE 1735 — Critical, open).
//!
//! IEEE 1735 menstandarkan enkripsi IP RTL untuk perlindungan IP sensitif.
//! Implementasi nyata (enkripsi/dekripsi sebenarnya menurut skema 1735)
//! BELUM ada — stage 1 adalah struktur data + placeholder yang mencatat
//! gap ini, dengan tes yang mendokumentasikan statusnya.
//! Belum di-wire ke alur enkripsi IP nyata.

use mivon_core::error::SimError;

/// Konfigurasi enkripsi IP menurut IEEE 1735.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ieee1735Config {
    pub algorithm: EncryptionAlgorithm,
    pub key_id: String,
}

/// Algoritma enkripsi yang didukung (placeholder — hanya placeholder, belum
/// ada implementasi kriptografi sebenarnya).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    Aes128Gcm,
    Aes256Gcm,
    Placeholder,
}

/// Hasil enkripsi IP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedIp {
    pub encrypted_blob: Vec<u8>,
    pub key_id: String,
}

/// Enkripsi/dekripsi IP menurut IEEE 1735 (placeholder — belum diimplementasi
/// sebenarnya).
#[derive(Debug, Clone, Default)]
pub struct Ieee1735;

impl Ieee1735 {
    pub fn new() -> Self {
        Self
    }

    /// Enkripsi sebuah IP. Sekarang masih placeholder: mengembalikan
    /// `Err(SimError::runtime(...))` karena kriptografi 1735 belum ada.
    pub fn encrypt(&self, _plaintext: &[u8], _config: &Ieee1735Config)
        -> Result<EncryptedIp, SimError>
    {
        Err(SimError::runtime("IEEE 1735 encryption not yet implemented"))
    }

    /// Dekripsi sebuah IP. Placeholder yang sama.
    pub fn decrypt(&self, _ciphertext: &EncryptedIp)
        -> Result<Vec<u8>, SimError>
    {
        Err(SimError::runtime("IEEE 1735 decryption not yet implemented"))
    }

    /// Apakah enkripsi 1735 sebenarnya tersedia? Belum.
    pub fn is_supported(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ieee1735_not_yet_supported() {
        let e = Ieee1735::new();
        assert!(!e.is_supported());
    }

    #[test]
    fn encrypt_returns_not_implemented() {
        let e = Ieee1735::new();
        let cfg = Ieee1735Config {
            algorithm: EncryptionAlgorithm::Aes128Gcm,
            key_id: "key-1".into(),
        };
        let err = e.encrypt(b"plaintext", &cfg).unwrap_err();
        assert!(matches!(err, SimError::Runtime(_)));
    }

    #[test]
    fn decrypt_returns_not_implemented() {
        let e = Ieee1735::new();
        let err = e.decrypt(&EncryptedIp {
            encrypted_blob: vec![0, 1, 2],
            key_id: "key-1".into(),
        }).unwrap_err();
        assert!(matches!(err, SimError::Runtime(_)));
    }

    #[test]
    fn config_round_trip() {
        let cfg = Ieee1735Config {
            algorithm: EncryptionAlgorithm::Aes256Gcm,
            key_id: "k2".into(),
        };
        assert_eq!(cfg.key_id, "k2");
        assert!(matches!(cfg.algorithm, EncryptionAlgorithm::Aes256Gcm));
    }
}
