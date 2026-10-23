//! FEAT-12: License Server — floating license management.
//!
//! Manages concurrent license checkout/checkin across multiple users.
//! Tracks license pool, heartbeat timeout, and lease expiration.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// License pool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicensePool {
    pub total: u32,
    pub available: u32,
    pub feature: String,
}

/// Active license lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseLease {
    pub id: String,
    pub user: String,
    pub host: String,
    pub feature: String,
    pub granted_at: u64,
    pub expires_at: u64,
    pub heartbeat: u64,
}

/// License server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseServer {
    pools: HashMap<String, LicensePool>,
    leases: Vec<LicenseLease>,
    next_id: u32,
    heartbeat_timeout: u64,
}

impl LicenseServer {
    pub fn new(heartbeat_timeout_secs: u64) -> Self {
        LicenseServer {
            pools: HashMap::new(),
            leases: Vec::new(),
            next_id: 1,
            heartbeat_timeout: heartbeat_timeout_secs,
        }
    }

    /// Register a license pool.
    pub fn register_pool(&mut self, feature: &str, count: u32) {
        self.pools.insert(
            feature.to_string(),
            LicensePool {
                total: count,
                available: count,
                feature: feature.to_string(),
            },
        );
    }

    /// Checkout a license. Returns lease ID or error.
    pub fn checkout(&mut self, feature: &str, user: &str, host: &str) -> Result<String, String> {
        // Expire stale leases first
        self.expire_stale();

        let pool = self
            .pools
            .get_mut(feature)
            .ok_or_else(|| format!("unknown feature: {}", feature))?;

        if pool.available == 0 {
            return Err(format!("no licenses available for {}", feature));
        }

        pool.available -= 1;
        let now = now_secs();
        let id = format!("L-{:06}", self.next_id);
        self.next_id += 1;

        self.leases.push(LicenseLease {
            id: id.clone(),
            user: user.to_string(),
            host: host.to_string(),
            feature: feature.to_string(),
            granted_at: now,
            expires_at: now + 3600, // 1 hour default
            heartbeat: now,
        });

        Ok(id)
    }

    /// Checkin (release) a license.
    pub fn checkin(&mut self, lease_id: &str) -> Result<(), String> {
        let idx = self
            .leases
            .iter()
            .position(|l| l.id == lease_id)
            .ok_or_else(|| format!("lease {} not found", lease_id))?;
        let lease = self.leases.remove(idx);
        if let Some(pool) = self.pools.get_mut(&lease.feature) {
            pool.available += 1;
        }
        Ok(())
    }

    /// Send heartbeat to keep lease alive.
    pub fn heartbeat(&mut self, lease_id: &str) -> Result<(), String> {
        let lease = self
            .leases
            .iter_mut()
            .find(|l| l.id == lease_id)
            .ok_or_else(|| format!("lease {} not found", lease_id))?;
        let now = now_secs();
        lease.heartbeat = now;
        lease.expires_at = now + 3600;
        Ok(())
    }

    /// Expire stale leases (heartbeat timeout) DAN lease yang sudah lewat
    /// `expires_at`. Tanpa cek expires_at, lease bisa hidup selamanya selama
    /// heartbeat terus dikirim melewati masa berlaku yang diberikan.
    fn expire_stale(&mut self) {
        self.expire_stale_at(now_secs());
    }

    /// Varian dengan waktu eksplisit (untuk test deterministik).
    fn expire_stale_at(&mut self, now: u64) {
        let timeout = self.heartbeat_timeout;
        self.leases.retain(|l| {
            let stale =
                now.saturating_sub(l.heartbeat) > timeout || now > l.expires_at;
            if stale {
                // Return license to pool
                // Can't return here since we're in retain, handle after
                false
            } else {
                true
            }
        });
        // Recompute available counts
        for pool in self.pools.values_mut() {
            pool.available = pool.total;
        }
        for l in &self.leases {
            if let Some(pool) = self.pools.get_mut(&l.feature) {
                pool.available = pool.available.saturating_sub(1);
            }
        }
    }

    /// List active leases.
    pub fn active_leases(&self) -> Vec<&LicenseLease> {
        self.leases.iter().collect()
    }

    /// Get pool status.
    pub fn pool_status(&self, feature: &str) -> Option<(u32, u32, u32)> {
        self.pools
            .get(feature)
            .map(|p| (p.total, p.available, p.total - p.available))
    }

    /// Summary.
    pub fn summary(&self) -> String {
        let pools: Vec<String> = self
            .pools
            .iter()
            .map(|(f, p)| format!("{}: {}/{} available", f, p.available, p.total))
            .collect();
        format!(
            "LicenseServer: {} pools, {} active leases [{}]",
            self.pools.len(),
            self.leases.len(),
            pools.join(", ")
        )
    }

    /// Simpan state server (JSON, atomik, membuat direktori induk).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("gagal buat direktori {}: {e}", parent.display()))?;
            }
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let mut temp_name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| std::ffi::OsString::from("licenses.json"));
        temp_name.push(".tmp");
        let temp = path.with_file_name(temp_name);
        std::fs::write(&temp, json)
            .map_err(|e| format!("gagal tulis {}: {e}", temp.display()))?;
        std::fs::rename(&temp, path)
            .map_err(|e| format!("gagal rename {}: {e}", path.display()))
    }

    /// Muat state server dari JSON.
    pub fn load(path: &Path) -> Result<Self, String> {
        let src = std::fs::read_to_string(path)
            .map_err(|e| format!("gagal baca {}: {e}", path.display()))?;
        serde_json::from_str(&src).map_err(|e| format!("license db {}: {e}", path.display()))
    }

    /// Timeout heartbeat yang dipakai server (detik).
    pub fn heartbeat_timeout(&self) -> u64 {
        self.heartbeat_timeout
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkout_checkin() {
        let mut server = LicenseServer::new(300);
        server.register_pool("mivon-sim", 2);
        let id1 = server.checkout("mivon-sim", "alice", "host1").unwrap();
        let _id2 = server.checkout("mivon-sim", "bob", "host2").unwrap();
        assert!(server.checkout("mivon-sim", "carol", "host3").is_err());
        server.checkin(&id1).unwrap();
        assert!(server.checkout("mivon-sim", "carol", "host3").is_ok());
    }

    #[test]
    fn test_heartbeat() {
        let mut server = LicenseServer::new(300);
        server.register_pool("mivon-sim", 1);
        let id = server.checkout("mivon-sim", "alice", "host1").unwrap();
        assert!(server.heartbeat(&id).is_ok());
    }

    #[test]
    fn test_summary() {
        let mut server = LicenseServer::new(300);
        server.register_pool("mivon-sim", 4);
        server.register_pool("mivon-formal", 2);
        let s = server.summary();
        assert!(s.contains("2 pools"));
    }

    #[test]
    fn test_pool_status() {
        let mut server = LicenseServer::new(300);
        server.register_pool("mivon-sim", 3);
        let _id = server.checkout("mivon-sim", "alice", "h").unwrap();
        let (total, avail, in_use) = server.pool_status("mivon-sim").unwrap();
        assert_eq!(total, 3);
        assert_eq!(avail, 2);
        assert_eq!(in_use, 1);
    }

    #[test]
    fn lease_expires_after_expires_at_even_with_heartbeat_timeout_long() {
        let mut server = LicenseServer::new(100_000);
        server.register_pool("sim", 1);
        let id = server.checkout("sim", "alice", "h").unwrap();
        let now = now_secs();
        // Sebelum expires_at: lease hidup.
        server.expire_stale_at(now + 10);
        assert_eq!(server.active_leases().len(), 1);
        assert_eq!(server.pool_status("sim").unwrap().1, 0);
        // Melewati expires_at (1 jam): lease dibuang, lisensi kembali.
        server.expire_stale_at(now + 4000);
        assert!(server.active_leases().is_empty(), "lease {id} harus expired");
        assert_eq!(server.pool_status("sim").unwrap().1, 1);
    }

    #[test]
    fn stale_heartbeat_expires_lease() {
        let mut server = LicenseServer::new(60);
        server.register_pool("sim", 1);
        server.checkout("sim", "alice", "h").unwrap();
        server.expire_stale_at(now_secs() + 61);
        assert!(server.active_leases().is_empty());
        assert_eq!(server.pool_status("sim").unwrap().1, 1);
    }

    #[test]
    fn save_load_roundtrip_keeps_leases_and_next_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/licenses.json");
        let mut server = LicenseServer::new(300);
        server.register_pool("sim", 2);
        let id = server.checkout("sim", "alice", "h").unwrap();
        server.save(&path).unwrap();
        let mut loaded = LicenseServer::load(&path).unwrap();
        assert_eq!(loaded.active_leases().len(), 1);
        assert_eq!(loaded.pool_status("sim"), Some((2, 1, 1)));
        assert_eq!(loaded.heartbeat_timeout(), 300);
        let next = loaded.checkout("sim", "bob", "h2").unwrap();
        assert_ne!(next, id, "next_id tidak reset setelah load");
        assert_eq!(next, "L-000002");
        assert!(!dir.path().join("nested/licenses.json.tmp").exists());
    }

    #[test]
    fn load_rejects_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("licenses.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(LicenseServer::load(&path).is_err());
    }
}
