//! IEEE 1735 IP encryption — stage 2 of FEAT-13
//! (IP encryption IEEE 1735 — Critical, in progress).
//!
//! IEEE 1735 menstandarkan enkripsi IP RTL untuk perlindungan IP sensitif.
//! Stage 1: struktur data + placeholder. Stage 2: enkripsi/dekripsi
//! AES-128-GCM nyata (feature `crypto`).
//!
//! Enkripsi 1735 sebenarnya menggunakan skema yang lebih kompleks
//! (key wrapping, envelope encryption), namun Stage 2 ini mengimplementasikan
//! AES-128-GCM sebagai primitif inti yang dapat diekspansi ke skema 1735
//! penuh di tahap lanjutan.

use mivon_core::error::SimError;

#[cfg(feature = "crypto")]
use aes_gcm::{Aes128Gcm, Key, KeyInit, Nonce};
#[cfg(feature = "crypto")]
use aead::Aead;
#[cfg(feature = "crypto")]
use rand::rngs::OsRng;
#[cfg(feature = "crypto")]
use rand::RngCore;

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

    /// Enkripsi sebuah IP dengan AES-128-GCM (feature `crypto`).
    ///
    /// Tanpa fitur `crypto`, mengembalikan `Err` (placeholder stage 1).
    /// Dengan fitur `crypto`, menghasilkan ciphertext yang bisa didekripsi
    /// oleh `decrypt()` (round-trip aman).
    pub fn encrypt(
        &self,
        plaintext: &[u8],
        config: &Ieee1735Config,
    ) -> Result<EncryptedIp, SimError> {
        #[cfg(feature = "crypto")]
        {
            // Untuk key_id pendek, pad dengan nol hingga 16 byte.
            let mut key_bytes = [0u8; 16];
            let src = config.key_id.as_bytes();
            let len = src.len().min(16);
            key_bytes[..len].copy_from_slice(&src[..len]);
            let key = Key::<Aes128Gcm>::try_from(key_bytes.as_slice())
                .expect("invalid key length for AES-128");

            let cipher = Aes128Gcm::new(&key);
            let mut nonce_bytes = [0u8; 12];
            OsRng
                .try_fill_bytes(&mut nonce_bytes)
                .map_err(|e| SimError::runtime(format!("RNG gagal: {e}")))?;
            let nonce = Nonce::try_from(&nonce_bytes[..])
                .expect("invalid nonce length");

            let ciphertext = cipher
                .encrypt(&nonce, plaintext)
                .map_err(|e| SimError::runtime(format!("enkripsi gagal: {e}")))?;

            // Simpan nonce (12 byte) di awal encrypted_blob agar decrypt
            // bisa mengekstraknya: nonce || ciphertext || tag
            let mut encrypted_blob = nonce_bytes.to_vec();
            encrypted_blob.extend_from_slice(&ciphertext);

            Ok(EncryptedIp {
                encrypted_blob,
                key_id: config.key_id.clone(),
            })
        }
        #[cfg(not(feature = "crypto"))]
        {
            let _ = plaintext;
            let _ = config;
            Err(SimError::runtime(
                "IEEE 1735 encryption not yet implemented (aktifkan fitur 'crypto')",
            ))
        }
    }

    /// Dekripsi sebuah IP dengan AES-128-GCM (feature `crypto`).
    ///
    /// Tanpa fitur `crypto`, mengembalikan `Err` (placeholder stage 1).
    pub fn decrypt(&self, ciphertext: &EncryptedIp) -> Result<Vec<u8>, SimError> {
        #[cfg(feature = "crypto")]
        {
            let mut key_bytes = [0u8; 16];
            let src = ciphertext.key_id.as_bytes();
            let len = src.len().min(16);
            key_bytes[..len].copy_from_slice(&src[..len]);
            let key = Key::<Aes128Gcm>::try_from(key_bytes.as_slice())
                .expect("invalid key length for AES-128");

            let cipher = Aes128Gcm::new(&key);
            // Nonce adalah 12 byte awal ciphertext (GCM nonce disisipkan).
            if ciphertext.encrypted_blob.len() < 12 {
                return Err(SimError::runtime("ciphertext terlalu pendek"));
            }
            let nonce = Nonce::try_from(&ciphertext.encrypted_blob[..12])
                .expect("invalid nonce length");
            let ct = &ciphertext.encrypted_blob[12..];

            let plaintext = cipher
                .decrypt(&nonce, ct)
                .map_err(|e| SimError::runtime(format!("dekripsi gagal: {e}")))?;
            Ok(plaintext)
        }
        #[cfg(not(feature = "crypto"))]
        {
            let _ = ciphertext;
            Err(SimError::runtime(
                "IEEE 1735 decryption not yet implemented (aktifkan fitur 'crypto')",
            ))
        }
    }

    /// Apakah enkripsi 1735 sebenarnya tersedia? Hanya true bila fitur `crypto` aktif.
    pub fn is_supported(&self) -> bool {
        #[cfg(feature = "crypto")]
        {
            true
        }
        #[cfg(not(feature = "crypto"))]
        {
            false
        }
    }

    /// Daftar algoritma yang dikenali (semua masih placeholder — belum ada
    /// implementasi kriptografi sebenarnya).
    pub fn supported_algorithms(&self) -> Vec<EncryptionAlgorithm> {
        vec![
            EncryptionAlgorithm::Aes128Gcm,
            EncryptionAlgorithm::Aes256Gcm,
            EncryptionAlgorithm::Placeholder,
        ]
    }

    /// Konfigurasi default (placeholder, tidak aman untuk produksi).
    pub fn default_config() -> Ieee1735Config {
        #[cfg(feature = "crypto")]
        {
            Ieee1735Config {
                algorithm: EncryptionAlgorithm::Aes128Gcm,
                key_id: "mivon-default-key".into(),
            }
        }
        #[cfg(not(feature = "crypto"))]
        {
            Ieee1735Config {
                algorithm: EncryptionAlgorithm::Placeholder,
                key_id: String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "crypto"))]
    fn ieee1735_not_yet_supported() {
        let e = Ieee1735::new();
        assert!(!e.is_supported());
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn ieee1735_supported_with_crypto() {
        let e = Ieee1735::new();
        assert!(e.is_supported());
    }

    #[test]
    #[cfg(not(feature = "crypto"))]
    fn encrypt_returns_not_implemented() {
        let e = Ieee1735::new();
        let cfg = Ieee1735Config {
            algorithm: EncryptionAlgorithm::Aes128Gcm,
            key_id: "key-1".into(),
        };
        let err = e.encrypt(b"plaintext", &cfg).unwrap_err();
        assert!(matches!(err, SimError::Runtime(_)));
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn encrypt_decrypt_roundtrip() {
        let e = Ieee1735::new();
        let cfg = Ieee1735Config {
            algorithm: EncryptionAlgorithm::Aes128Gcm,
            key_id: "test-key-1234".into(),
        };
        let plaintext = b"Hello, IEEE 1735!";
        let encrypted = e.encrypt(plaintext, &cfg).expect("enkripsi harus sukses");
        let decrypted = e.decrypt(&encrypted).expect("dekripsi harus sukses");
        assert_eq!(decrypted.as_slice(), plaintext);
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn encrypt_decrypt_different_keys_fail() {
        let e = Ieee1735::new();
        let cfg1 = Ieee1735Config {
            algorithm: EncryptionAlgorithm::Aes128Gcm,
            key_id: "key-alpha".into(),
        };
        let cfg2 = Ieee1735Config {
            algorithm: EncryptionAlgorithm::Aes128Gcm,
            key_id: "key-beta".into(),
        };
        let plaintext = b"secret data";
        let encrypted = e.encrypt(plaintext, &cfg1).expect("enkripsi harus sukses");
        // Dekripsi dengan kunci berbeda harus gagal
        let err = e
            .decrypt(&EncryptedIp {
                encrypted_blob: encrypted.encrypted_blob.clone(),
                key_id: cfg2.key_id.clone(),
            })
            .unwrap_err();
        assert!(matches!(err, SimError::Runtime(_)));
    }

    #[test]
    #[cfg(not(feature = "crypto"))]
    fn decrypt_returns_not_implemented() {
        let e = Ieee1735::new();
        let err = e.decrypt(&EncryptedIp {
            encrypted_blob: vec![0, 1, 2],
            key_id: "key-1".into(),
        }).unwrap_err();
        assert!(matches!(err, SimError::Runtime(_)));
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn decrypt_short_ciphertext_fails() {
        let e = Ieee1735::new();
        let err = e
            .decrypt(&EncryptedIp {
                encrypted_blob: vec![1, 2, 3],
                key_id: "key-1".into(),
            })
            .unwrap_err();
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

    #[test]
    #[cfg(not(feature = "crypto"))]
    fn supported_algorithms_lists_known() {
        let e = Ieee1735::new();
        let algs = e.supported_algorithms();
        assert_eq!(algs.len(), 3);
        assert!(algs.contains(&EncryptionAlgorithm::Aes128Gcm));
        assert!(algs.contains(&EncryptionAlgorithm::Aes256Gcm));
        assert!(algs.contains(&EncryptionAlgorithm::Placeholder));
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn supported_algorithms_lists_known_crypto() {
        let e = Ieee1735::new();
        let algs = e.supported_algorithms();
        assert_eq!(algs.len(), 3);
        assert!(algs.contains(&EncryptionAlgorithm::Aes128Gcm));
        assert!(algs.contains(&EncryptionAlgorithm::Aes256Gcm));
        assert!(algs.contains(&EncryptionAlgorithm::Placeholder));
    }

    #[test]
    #[cfg(not(feature = "crypto"))]
    fn default_config_is_placeholder() {
        let e = Ieee1735::new();
        let cfg = Ieee1735::default_config();
        assert!(matches!(cfg.algorithm, EncryptionAlgorithm::Placeholder));
        assert!(cfg.key_id.is_empty());
        assert!(!e.is_supported());
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn default_config_is_aes128gcm() {
        let e = Ieee1735::new();
        let cfg = Ieee1735::default_config();
        assert!(matches!(cfg.algorithm, EncryptionAlgorithm::Aes128Gcm));
        assert_eq!(cfg.key_id, "mivon-default-key");
        assert!(e.is_supported());
    }
}
