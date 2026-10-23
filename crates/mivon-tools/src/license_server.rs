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
    /// Batas absolut lease — TIDAK diperpanjang heartbeat. Tanpa ini klien
    /// yang terus heartbeat memegang lisensi selamanya.
    #[serde(default)]
    pub max_expires_at: u64,
}

/// License server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseServer {
    pools: HashMap<String, LicensePool>,
    leases: Vec<LicenseLease>,
    next_id: u32,
    heartbeat_timeout: u64,
    /// Durasi maksimum satu lease (detik), tidak diperpanjang heartbeat.
    #[serde(default = "default_lease_duration")]
    lease_duration: u64,
}

/// Default masa berlaku absolut lease: 8 jam.
fn default_lease_duration() -> u64 {
    8 * 3600
}

impl LicenseServer {
    pub fn new(heartbeat_timeout_secs: u64) -> Self {
        LicenseServer {
            pools: HashMap::new(),
            leases: Vec::new(),
            next_id: 1,
            heartbeat_timeout: heartbeat_timeout_secs,
            lease_duration: default_lease_duration(),
        }
    }

    /// Register a license pool.
    ///
    /// Bila pool sudah ada dan sedang dipakai, `available` dihitung ulang dari
    /// jumlah lease aktif — sebelumnya `init` ulang me-reset available ke
    /// `count` sehingga melebihi `total` dan bisa underflow di `pool_status`.
    pub fn register_pool(&mut self, feature: &str, count: u32) {
        self.expire_stale();
        let active = self.leases.iter().filter(|l| l.feature == feature).count() as u32;
        self.pools.insert(
            feature.to_string(),
            LicensePool {
                total: count,
                available: count.saturating_sub(active),
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
            expires_at: now + 3600, // 1 jam sejak heartbeat terakhir
            heartbeat: now,
            max_expires_at: now + self.lease_duration,
        });

        Ok(id)
    }

    /// Checkin (release) a license.
    pub fn checkin(&mut self, lease_id: &str) -> Result<(), String> {
        // Bersihkan lease basi dulu agar slot tidak dihitung dua kali.
        self.expire_stale();
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
    ///
    /// Lease yang sudah lewat `expires_at`/`max_expires_at` TIDAK dihidupkan
    /// kembali — sebelumnya heartbeat selalu menulis ulang `expires_at`
    /// sehingga lease mati bisa diperpanjang selamanya.
    pub fn heartbeat(&mut self, lease_id: &str) -> Result<(), String> {
        self.expire_stale();
        let now = now_secs();
        let lease = self
            .leases
            .iter_mut()
            .find(|l| l.id == lease_id)
            .ok_or_else(|| format!("lease {} not found", lease_id))?;
        if now > lease.expires_at || now > lease.max_expires_at {
            return Err(format!("lease {} expired", lease_id));
        }
        lease.heartbeat = now;
        lease.expires_at = (now + 3600).min(lease.max_expires_at);
        Ok(())
    }

    /// Bersihkan lease basi (dipakai perintah status sebelum membaca).
    pub fn refresh(&mut self) {
        self.expire_stale();
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
            let stale = now.saturating_sub(l.heartbeat) > timeout
                || now > l.expires_at
                // max_expires_at=0 pada DB lama → abaikan batas absolut.
                || (l.max_expires_at != 0 && now > l.max_expires_at);
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
            .map(|p| (p.total, p.available, p.total.saturating_sub(p.available)))
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
        // Temp unik per proses: nama temp tetap membuat dua penulis paralel
        // saling menimpa/menghapus file sebelum rename.
        let mut temp_name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| std::ffi::OsString::from("licenses.json"));
        temp_name.push(format!(".{}.tmp", std::process::id()));
        let temp = path.with_file_name(temp_name);
        std::fs::write(&temp, json)
            .map_err(|e| format!("gagal tulis {}: {e}", temp.display()))?;
        std::fs::rename(&temp, path)
            .map_err(|e| format!("gagal rename {}: {e}", path.display()))
    }

    /// Muat state server dari JSON + normalisasi.
    ///
    /// DB hasil edit tangan/restore bisa punya `next_id` lebih kecil dari ID
    /// terpakai (→ ID duplikat), `available` tidak konsisten, atau lease ID
    /// ganda. Semua dinormalisasi/ditolak di sini.
    pub fn load(path: &Path) -> Result<Self, String> {
        let src = std::fs::read_to_string(path)
            .map_err(|e| format!("gagal baca {}: {e}", path.display()))?;
        let mut server: Self = serde_json::from_str(&src)
            .map_err(|e| format!("license db {}: {e}", path.display()))?;
        server.normalize()?;
        Ok(server)
    }

    /// Validasi + sinkronkan state hasil load.
    fn normalize(&mut self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for l in &self.leases {
            if !seen.insert(l.id.clone()) {
                return Err(format!("license db: lease ID duplikat {}", l.id));
            }
        }
        // next_id harus melewati ID numerik terbesar.
        let max_id = self
            .leases
            .iter()
            .filter_map(|l| l.id.strip_prefix("L-"))
            .filter_map(|n| n.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_id + 1);
        // available dihitung ulang dari lease aktif.
        for pool in self.pools.values_mut() {
            let active = self
                .leases
                .iter()
                .filter(|l| l.feature == pool.feature)
                .count() as u32;
            pool.available = pool.total.saturating_sub(active);
        }
        Ok(())
    }

    /// Ubah timeout heartbeat (dipakai CLI --timeout pada DB yang sudah ada).
    pub fn set_heartbeat_timeout(&mut self, secs: u64) {
        self.heartbeat_timeout = secs;
    }

    /// Timeout heartbeat yang dipakai server (detik).
    pub fn heartbeat_timeout(&self) -> u64 {
        self.heartbeat_timeout
    }
}

/// Lock file sederhana untuk penulisan DB lisensi.
struct DbLock {
    path: std::path::PathBuf,
}

impl DbLock {
    fn acquire(path: &Path, timeout: std::time::Duration) -> Result<Self, String> {
        use std::io::Write;
        let start = std::time::Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut f) => {
                    let _ = f.write_all(b"lock");
                    return Ok(DbLock {
                        path: path.to_path_buf(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Lock basi (>60s) = proses pemegang mati.
                    let stale = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .map(|d| d.as_secs() > 60)
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    if start.elapsed() > timeout {
                        return Err(format!(
                            "lock {} masih dipegang proses lain",
                            path.display()
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => {
                    return Err(format!("gagal membuat lock {}: {e}", path.display()));
                }
            }
        }
    }
}

impl Drop for DbLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Jalankan `f` pada DB dengan lock eksklusif: load → mutate → save.
///
/// Tanpa lock, dua `checkout` paralel membaca state yang sama dan menulis
/// lease ID duplikat (atau saling menghapus file temp).
pub fn with_locked_db<T>(
    path: &Path,
    default_timeout: u64,
    f: impl FnOnce(&mut LicenseServer) -> Result<T, String>,
) -> Result<T, String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("gagal buat direktori {}: {e}", parent.display()))?;
        }
    }
    let mut lock_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("licenses.json"));
    lock_name.push(".lock");
    let lock = DbLock::acquire(&path.with_file_name(lock_name), std::time::Duration::from_secs(5))?;
    let mut server = if path.exists() {
        LicenseServer::load(path)?
    } else {
        LicenseServer::new(default_timeout)
    };
    let out = f(&mut server)?;
    server.save(path)?;
    drop(lock);
    Ok(out)
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
    fn heartbeat_rejects_expired_lease() {
        let mut server = LicenseServer::new(100_000);
        server.register_pool("sim", 1);
        let id = server.checkout("sim", "alice", "h").unwrap();
        // Paksa lease lewat masa berlaku absolut.
        server.leases[0].max_expires_at = now_secs().saturating_sub(1);
        assert!(server.heartbeat(&id).is_err(), "lease mati tidak boleh hidup lagi");
        assert!(server.active_leases().is_empty());
        assert_eq!(server.pool_status("sim").unwrap().1, 1);
    }

    #[test]
    fn reinit_keeps_active_leases() {
        let mut server = LicenseServer::new(300);
        server.register_pool("sim", 2);
        server.checkout("sim", "alice", "h").unwrap();
        server.register_pool("sim", 2);
        assert_eq!(server.pool_status("sim"), Some((2, 1, 1)));
        server.checkin("L-000001").unwrap();
        assert_eq!(server.pool_status("sim"), Some((2, 2, 0)));
    }

    #[test]
    fn load_normalizes_next_id_and_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("licenses.json");
        // DB hasil edit tangan: next_id=1 (bentrok) dan available salah.
        let json = r#"{"pools":{"sim":{"total":2,"available":2,"feature":"sim"}},
            "leases":[{"id":"L-000005","user":"a","host":"h","feature":"sim",
            "granted_at":1,"expires_at":9999999999,"heartbeat":9999999999}],
            "next_id":1,"heartbeat_timeout":300}"#;
        std::fs::write(&path, json).unwrap();
        let mut s = LicenseServer::load(&path).unwrap();
        assert_eq!(s.pool_status("sim"), Some((2, 1, 1)), "available dihitung ulang");
        let id = s.checkout("sim", "b", "h2").unwrap();
        assert_eq!(id, "L-000006", "next_id melewati ID terpakai");
    }

    #[test]
    fn load_rejects_duplicate_lease_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("licenses.json");
        let json = r#"{"pools":{},"leases":[
            {"id":"L-000001","user":"a","host":"h","feature":"x",
             "granted_at":1,"expires_at":9,"heartbeat":1},
            {"id":"L-000001","user":"b","host":"h","feature":"x",
             "granted_at":1,"expires_at":9,"heartbeat":1}],
            "next_id":2,"heartbeat_timeout":300}"#;
        std::fs::write(&path, json).unwrap();
        assert!(LicenseServer::load(&path).is_err());
    }

    #[test]
    fn locked_db_serializes_concurrent_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("licenses.json");
        with_locked_db(&path, 300, |s| {
            s.register_pool("sim", 1);
            Ok(())
        })
        .unwrap();

        let p1 = path.clone();
        let p2 = path.clone();
        let t1 = std::thread::spawn(move || {
            with_locked_db(&p1, 300, |s| s.checkout("sim", "a", "h")).is_ok()
        });
        let t2 = std::thread::spawn(move || {
            with_locked_db(&p2, 300, |s| s.checkout("sim", "b", "h")).is_ok()
        });
        let ok1 = t1.join().unwrap();
        let ok2 = t2.join().unwrap();
        assert!(ok1 ^ ok2, "tepat satu checkout berhasil (pool=1)");
        let s = LicenseServer::load(&path).unwrap();
        assert_eq!(s.active_leases().len(), 1, "tidak ada lease hilang/duplikat");
    }


    #[test]
    fn load_rejects_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("licenses.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(LicenseServer::load(&path).is_err());
    }
}
