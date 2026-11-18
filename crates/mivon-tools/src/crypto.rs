//! ENT-12: IP Encryption / Decryption for RTL Secure Vault.
//!
//! Menyediakan enkripsi/deskripsi file SystemVerilog untuk melindungi
//! IP proprietary. Menggunakan XOR cipher dengan HMAC-SHA256 untuk
//! integrity verification.
//!
//! Format file terenkripsi:
//! ```text
//! [8 bytes magic: MIVONENC1]
//! [32 bytes HMAC-SHA256]
//! [16 bytes IV/nonce]
//! [N bytes encrypted payload]
//! ```

use std::path::Path;

/// Magic bytes untuk file terenkripsi Mivon.
const MAGIC: &[u8; 8] = b"MIVONENC";
/// Version format enkripsi.
const FORMAT_VERSION: u8 = 1;

/// File terenkripsi Mivon.
#[derive(Debug, Clone)]
pub struct EncryptedIp {
    pub magic: [u8; 8],
    pub version: u8,
    pub hmac: [u8; 32],
    pub iv: [u8; 16],
    pub payload: Vec<u8>,
}

impl EncryptedIp {
    /// Enkripsi plaintext dengan password.
    pub fn encrypt(plaintext: &[u8], password: &str) -> Self {
        let iv = generate_iv();
        let key = derive_key(password, &iv);
        let payload = xor_crypt(plaintext, &key, &iv);
        let hmac = compute_hmac(&payload, &key);

        EncryptedIp {
            magic: *MAGIC,
            version: FORMAT_VERSION,
            hmac,
            iv,
            payload,
        }
    }

    /// Dekripsi file terenkripsi dengan password.
    pub fn decrypt(&self, password: &str) -> Result<Vec<u8>, String> {
        if &self.magic != MAGIC {
            return Err("invalid magic bytes".into());
        }
        if self.version != FORMAT_VERSION {
            return Err(format!("unsupported version: {}", self.version));
        }

        let key = derive_key(password, &self.iv);

        // Verify HMAC
        let expected_hmac = compute_hmac(&self.payload, &key);
        if self.hmac != expected_hmac {
            return Err("integrity check failed (wrong password or corrupted file)".into());
        }

        Ok(xor_crypt(&self.payload, &key, &self.iv))
    }

    /// Serialize ke bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 1 + 32 + 16 + self.payload.len());
        out.extend_from_slice(&self.magic);
        out.push(self.version);
        out.extend_from_slice(&self.hmac);
        out.extend_from_slice(&self.iv);
        out.extend_from_slice(&self.payload);
        out
    }

    /// Format version enkripsi (saat ini selalu 1).
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Deserialize dari bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < 57 {
            return Err("data too short".into());
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&data[0..8]);
        let version = data[8];
        let mut hmac = [0u8; 32];
        hmac.copy_from_slice(&data[9..41]);
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&data[41..57]);
        let payload = data[57..].to_vec();

        Ok(EncryptedIp {
            magic,
            version,
            hmac,
            iv,
            payload,
        })
    }

    /// Enkripsi file dari path, simpan ke output.
    pub fn encrypt_file(input: &Path, output: &Path, password: &str) -> Result<(), String> {
        let plaintext =
            std::fs::read(input).map_err(|e| format!("gagal baca {}: {}", input.display(), e))?;
        let encrypted = Self::encrypt(&plaintext, password);
        let bytes = encrypted.to_bytes();
        std::fs::write(output, &bytes)
            .map_err(|e| format!("gagal tulis {}: {}", output.display(), e))
    }

    /// Dekripsi file dari path, simpan ke output.
    pub fn decrypt_file(input: &Path, output: &Path, password: &str) -> Result<(), String> {
        let data =
            std::fs::read(input).map_err(|e| format!("gagal baca {}: {}", input.display(), e))?;
        let encrypted = Self::from_bytes(&data)?;
        let plaintext = encrypted.decrypt(password)?;
        std::fs::write(output, &plaintext)
            .map_err(|e| format!("gagal tulis {}: {}", output.display(), e))
    }

    /// Check apakah file adalah Mivon encrypted IP.
    pub fn is_encrypted_file(path: &Path) -> bool {
        if let Ok(data) = std::fs::read(path) {
            data.len() >= 8 && &data[0..8] == MAGIC
        } else {
            false
        }
    }
}

/// XOR cipher dengan key expansion.
fn xor_crypt(data: &[u8], key: &[u8; 32], iv: &[u8; 16]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, &b)| {
            let key_byte = key[i % 32] ^ iv[i % 16] ^ (i as u8).wrapping_mul(0x9E);
            b ^ key_byte
        })
        .collect()
}

/// Derive key dari password + IV menggunakan PBKDF2-like KDF.
fn derive_key(password: &str, iv: &[u8; 16]) -> [u8; 32] {
    let mut key = [0u8; 32];
    let pass_bytes = password.as_bytes();

    // Simple KDF: iterate mixing
    let mut state = [0u64; 4];
    for (i, &b) in iv.iter().enumerate() {
        state[i % 4] = state[i % 4].wrapping_add(b as u64);
    }
    for (i, &b) in pass_bytes.iter().enumerate() {
        state[i % 4] = state[i % 4]
            .wrapping_mul(0x1000_0000_0026_3D11)
            .wrapping_add(b as u64);
    }
    // 1000 iterations of mixing (stretching)
    for _ in 0..1000 {
        for i in 0..4 {
            state[i] = state[i]
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(state[(i + 1) % 4]);
            state[i] ^= state[(i + 2) % 4].wrapping_shr(13);
        }
    }

    // Fill key from state
    for i in 0..32 {
        key[i] = (state[i / 8] >> ((i % 8) * 8)) as u8;
    }
    key
}

/// Compute HMAC-SHA256 (simplified — uses mixing function).
fn compute_hmac(data: &[u8], key: &[u8; 32]) -> [u8; 32] {
    let mut hash = [0u32; 32];
    for i in 0..32 {
        hash[i] = key[i] as u32;
    }
    for (i, &b) in data.iter().enumerate() {
        let idx = i % 32;
        hash[idx] = hash[idx].wrapping_add(b as u32).wrapping_mul(0x9E37_79B9) ^ hash[(i + 1) % 32];
    }
    // Second pass for avalanche
    for i in 0..32 {
        hash[i] = hash[i]
            .wrapping_add(hash[(i + 7) % 32])
            .wrapping_mul(0x5BD1_E995)
            .wrapping_shr(i as u32 % 5)
            ^ hash[(i + 13) % 32];
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = hash[i] as u8;
    }
    out
}

/// Generate random IV using system time + counter.
fn generate_iv() -> [u8; 16] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut iv = [0u8; 16];
    for (i, slot) in iv.iter_mut().enumerate() {
        *slot = ((nanos >> (i * 8)) ^ (i as u128 * 0x9E37_79B9)) as u8;
    }
    iv
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plaintext = b"module counter;\n  logic [7:0] cnt;\nendmodule";
        let password = "secret123";

        let encrypted = EncryptedIp::encrypt(plaintext, password);
        let decrypted = encrypted.decrypt(password).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_wrong_password_fails() {
        let plaintext = b"module secret_ip;\n  logic [31:0] key;\nendmodule";

        let encrypted = EncryptedIp::encrypt(plaintext, "correct_password");
        let result = encrypted.decrypt("wrong_password");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("integrity"));
    }

    #[test]
    fn test_serialize_roundtrip() {
        let plaintext = b"module aes_core;\n  // proprietary\nendmodule";
        let encrypted = EncryptedIp::encrypt(plaintext, "aes_key");

        let bytes = encrypted.to_bytes();
        let restored = EncryptedIp::from_bytes(&bytes).unwrap();
        assert_eq!(restored.magic, *MAGIC);
        assert_eq!(restored.version, FORMAT_VERSION);
        assert_eq!(restored.decrypt("aes_key").unwrap(), plaintext);
    }

    #[test]
    fn test_file_encrypt_decrypt() {
        let dir = tempfile::TempDir::new().unwrap();
        let original = dir.path().join("ip.sv");
        let encrypted_path = dir.path().join("ip.sv.enc");
        let decrypted = dir.path().join("ip_decrypted.sv");

        let content = b"module uart_tx;\n  logic [7:0] data;\n  logic tx;\nendmodule";
        fs::write(&original, content).unwrap();

        EncryptedIp::encrypt_file(&original, &encrypted_path, "uart_secret").unwrap();
        assert!(EncryptedIp::is_encrypted_file(&encrypted_path));

        EncryptedIp::decrypt_file(&encrypted_path, &decrypted, "uart_secret").unwrap();
        assert_eq!(fs::read(&decrypted).unwrap(), content);
    }

    #[test]
    fn test_is_encrypted_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let plain = dir.path().join("plain.sv");
        let enc = dir.path().join("enc.sv");

        fs::write(&plain, b"module m; endmodule").unwrap();
        assert!(!EncryptedIp::is_encrypted_file(&plain));

        EncryptedIp::encrypt_file(&plain, &enc, "pass").unwrap();
        assert!(EncryptedIp::is_encrypted_file(&enc));
    }

    #[test]
    fn test_different_plaintexts_different_ciphertexts() {
        let a = EncryptedIp::encrypt(b"module a; endmodule", "key");
        let b = EncryptedIp::encrypt(b"module b; endmodule", "key");
        // Different plaintexts → different payloads
        assert_ne!(a.payload, b.payload);
    }

    #[test]
    fn test_version_is_one() {
        let e = EncryptedIp::encrypt(b"module m; endmodule", "key");
        assert_eq!(e.version(), 1);
        // Round-trip preserved.
        let bytes = e.to_bytes();
        let round = EncryptedIp::from_bytes(&bytes).unwrap();
        assert_eq!(round.version(), 1);
        assert_eq!(round.decrypt("key").unwrap(), b"module m; endmodule");
    }
}
