//! ENT-18: Parallel Multi-User Simulation Server.
//!
//! Mengelola sesi simulasi konkuren untuk banyak user: kuota per user,
//! kapasitas running, antrean FIFO, dan eksekusi lewat *runner* yang
//! di-inject pemanggil (`set_runner`). Tanpa runner, server hanya menjadi
//! registri sesi (status tetap Running) — pemisahan ini menjaga modul ini
//! bebas dari ketergantungan simulator.
//!
//! Urutan lock yang WAJIB dipertahankan: `sessions` → `queue`. Tidak ada
//! jalur yang mengambil `queue` lalu `sessions` (mencegah ABBA deadlock).
//! Lock memakai `unwrap_or_else(|e| e.into_inner())` supaya panic di satu
//! sesi tidak mem-poison seluruh server.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Simulation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSession {
    pub id: String,
    pub user: String,
    pub status: SessionStatus,
    pub source_files: Vec<String>,
    pub max_time: u64,
    pub elapsed_cycles: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionStatus {
    Queued,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

/// User quota.
#[derive(Debug, Clone)]
pub struct UserQuota {
    pub max_concurrent_sessions: usize,
    pub max_memory_mb: u64,
    pub max_time_secs: u64,
    pub current_sessions: usize,
}

impl Default for UserQuota {
    fn default() -> Self {
        UserQuota {
            max_concurrent_sessions: 4,
            max_memory_mb: 2048,
            max_time_secs: 300,
            current_sessions: 0,
        }
    }
}

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Kapasitas sesi yang boleh berjalan bersamaan (sisanya antre).
    pub max_total_sessions: usize,
    /// Batas keras jumlah sesi yang disimpan (riwayat) — submit berikutnya
    /// ditolak dengan `SubmitError::Capacity` agar memori/payload tidak
    /// tumbuh tanpa batas.
    pub max_sessions_total: usize,
    pub max_workers: usize,
    pub session_timeout_secs: u64,
    pub user_quotas: HashMap<String, UserQuota>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            max_total_sessions: 32,
            max_sessions_total: 512,
            max_workers: 8,
            session_timeout_secs: 600,
            user_quotas: HashMap::new(),
        }
    }
}

/// Error submit — dipetakan ke status HTTP oleh pemanggil
/// (kuota → 429, kapasitas riwayat → 503).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    Quota { user: String, active: usize, max: usize },
    Capacity { total: usize, max: usize },
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Quota { user, active, max } => {
                write!(f, "user {} has {} sessions (max {})", user, active, max)
            }
            SubmitError::Capacity { total, max } => {
                write!(f, "server menyimpan {} sesi (max {})", total, max)
            }
        }
    }
}

impl std::error::Error for SubmitError {}

/// Error cancel — dipetakan ke 404/409.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelError {
    NotFound,
    NotCancellable,
}

impl std::fmt::Display for CancelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CancelError::NotFound => write!(f, "session not found"),
            CancelError::NotCancellable => write!(f, "session not in cancellable state"),
        }
    }
}

/// Fungsi eksekusi sesi: (files, max_time) → jumlah cycle yang dijalankan.
pub type SessionRunner = Arc<dyn Fn(&[String], u64) -> Result<u64, String> + Send + Sync>;

/// State bersama — dipisah dari `SimServer` agar worker thread bisa memegang
/// `Arc` tanpa meminjam server.
struct Store {
    sessions: Mutex<HashMap<String, SimSession>>,
    queue: Mutex<Vec<String>>,
    runner: Mutex<Option<SessionRunner>>,
    next_id: AtomicU64,
}

impl Store {
    fn sessions(&self) -> MutexGuard<'_, HashMap<String, SimSession>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn queue(&self) -> MutexGuard<'_, Vec<String>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Simulation server — manages concurrent sessions.
pub struct SimServer {
    config: ServerConfig,
    store: Arc<Store>,
    start_time: Instant,
}

impl SimServer {
    pub fn new(config: ServerConfig) -> Self {
        SimServer {
            config,
            store: Arc::new(Store {
                sessions: Mutex::new(HashMap::new()),
                queue: Mutex::new(Vec::new()),
                runner: Mutex::new(None),
                next_id: AtomicU64::new(0),
            }),
            start_time: Instant::now(),
        }
    }

    /// Pasang runner eksekusi. Tanpa runner, sesi tetap Running (mode
    /// registri) karena tidak ada yang bisa menjalankannya.
    pub fn set_runner(&self, runner: SessionRunner) {
        *self
            .store
            .runner
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(runner);
    }

    /// Submit a new simulation session.
    ///
    /// ID dihitung dari counter atomik (tidak pernah dipakai ulang walau
    /// sesi lama dihapus), dan status + antrean diubah di dalam lock yang
    /// sama supaya tidak ada sesi Queued yang tidak tercatat di antrean.
    pub fn submit(
        &self,
        user: &str,
        files: Vec<String>,
        max_time: u64,
    ) -> Result<String, SubmitError> {
        let mut sessions = self.store.sessions();

        if sessions.len() >= self.config.max_sessions_total {
            return Err(SubmitError::Capacity {
                total: sessions.len(),
                max: self.config.max_sessions_total,
            });
        }

        let user_sessions = sessions
            .values()
            .filter(|s| {
                s.user == user && matches!(s.status, SessionStatus::Running | SessionStatus::Queued)
            })
            .count();
        let max_concurrent = self
            .config
            .user_quotas
            .get(user)
            .map(|q| q.max_concurrent_sessions)
            .unwrap_or(4);
        if user_sessions >= max_concurrent {
            return Err(SubmitError::Quota {
                user: user.to_string(),
                active: user_sessions,
                max: max_concurrent,
            });
        }

        // Kapasitas hanya menghitung sesi yang benar-benar berjalan; sesi
        // antrean tidak boleh menahan slot (kalau tidak, ambang tak pernah
        // turun dan semua submit berikutnya ikut antre).
        let running = sessions
            .values()
            .filter(|s| matches!(s.status, SessionStatus::Running))
            .count();
        let queued = running >= self.config.max_total_sessions;

        let id = format!(
            "sim_{}_{}",
            user,
            self.store.next_id.fetch_add(1, Ordering::SeqCst) + 1
        );
        let session = SimSession {
            id: id.clone(),
            user: user.to_string(),
            status: if queued {
                SessionStatus::Queued
            } else {
                SessionStatus::Running
            },
            source_files: files.clone(),
            max_time,
            elapsed_cycles: 0,
            created_at: now_secs(),
        };
        sessions.insert(id.clone(), session);
        if queued {
            self.store.queue().push(id.clone());
        }
        drop(sessions);

        if !queued {
            start_worker(self.store.clone(), self.config.clone(), id.clone(), files, max_time);
        }
        Ok(id)
    }

    /// Get session status.
    pub fn get_session(&self, id: &str) -> Option<SimSession> {
        self.store.sessions().get(id).cloned()
    }

    /// Cancel a session.
    ///
    /// Sesi yang dibatalkan dikeluarkan dari antrean; slot yang bebas
    /// langsung dipakai untuk mempromosikan sesi antrean berikutnya.
    pub fn cancel(&self, id: &str) -> Result<(), CancelError> {
        {
            let mut sessions = self.store.sessions();
            let session = sessions.get_mut(id).ok_or(CancelError::NotFound)?;
            match session.status {
                SessionStatus::Running | SessionStatus::Queued => {
                    session.status = SessionStatus::Cancelled;
                }
                _ => return Err(CancelError::NotCancellable),
            }
        }
        self.store.queue().retain(|q| q != id);
        promote(&self.store, &self.config);
        Ok(())
    }

    /// List sessions for a user.
    pub fn list_user_sessions(&self, user: &str) -> Vec<SimSession> {
        sort_sessions(
            self.store
                .sessions()
                .values()
                .filter(|s| s.user == user)
                .cloned()
                .collect(),
        )
    }

    /// List all active sessions.
    pub fn list_active(&self) -> Vec<SimSession> {
        sort_sessions(
            self.store
                .sessions()
                .values()
                .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Queued))
                .cloned()
                .collect(),
        )
    }

    /// Semua sesi (termasuk completed/failed/cancelled), urut created_at + id.
    pub fn list_all(&self) -> Vec<SimSession> {
        sort_sessions(self.store.sessions().values().cloned().collect())
    }

    /// Queue depth — jumlah sesi berstatus Queued (sumber kebenaran adalah
    /// status sesi, bukan isi vektor antrean yang bisa mengandung entri basi).
    pub fn queue_depth(&self) -> usize {
        self.store
            .sessions()
            .values()
            .filter(|s| matches!(s.status, SessionStatus::Queued))
            .count()
    }

    /// Summary.
    pub fn summary(&self) -> String {
        let sessions = self.store.sessions();
        let running = sessions
            .values()
            .filter(|s| matches!(s.status, SessionStatus::Running))
            .count();
        let queued = sessions
            .values()
            .filter(|s| matches!(s.status, SessionStatus::Queued))
            .count();
        let completed = sessions
            .values()
            .filter(|s| matches!(s.status, SessionStatus::Completed))
            .count();
        let uptime = self.start_time.elapsed().as_secs();
        format!(
            "SimServer: {} sessions ({} running, {} queued, {} completed), uptime {}s",
            sessions.len(),
            running,
            queued,
            completed,
            uptime,
        )
    }
}

/// Jalankan runner untuk satu sesi di thread terpisah.
fn start_worker(
    store: Arc<Store>,
    config: ServerConfig,
    id: String,
    files: Vec<String>,
    max_time: u64,
) {
    let runner = store
        .runner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some(runner) = runner else {
        return;
    };
    std::thread::spawn(move || {
        let outcome = runner(&files, max_time);
        finish_session(&store, &config, &id, outcome);
    });
}

/// Simpan hasil eksekusi. Sesi yang sudah dibatalkan tidak ditimpa.
fn finish_session(
    store: &Arc<Store>,
    config: &ServerConfig,
    id: &str,
    outcome: Result<u64, String>,
) {
    let mut freed = false;
    {
        let mut sessions = store.sessions();
        if let Some(s) = sessions.get_mut(id) {
            if matches!(s.status, SessionStatus::Running) {
                match outcome {
                    Ok(cycles) => {
                        s.elapsed_cycles = cycles;
                        s.status = SessionStatus::Completed;
                    }
                    Err(e) => s.status = SessionStatus::Failed(e),
                }
                freed = true;
            }
        }
    }
    if freed {
        promote(store, config);
    }
}

/// Promosikan sesi antrean selama masih ada slot running.
fn promote(store: &Arc<Store>, config: &ServerConfig) {
    loop {
        let next = {
            let mut sessions = store.sessions();
            let running = sessions
                .values()
                .filter(|s| matches!(s.status, SessionStatus::Running))
                .count();
            if running >= config.max_total_sessions {
                return;
            }
            let mut queue = store.queue();
            loop {
                let Some(id) = queue.first().cloned() else {
                    return;
                };
                queue.remove(0);
                match sessions.get_mut(&id) {
                    Some(s) if matches!(s.status, SessionStatus::Queued) => {
                        s.status = SessionStatus::Running;
                        break Some((id, s.source_files.clone(), s.max_time));
                    }
                    // Entri basi (sesi dibatalkan) — lewati.
                    _ => continue,
                }
            }
        };
        match next {
            Some((id, files, max_time)) => {
                start_worker(store.clone(), config.clone(), id, files, max_time)
            }
            None => return,
        }
    }
}

/// Urutkan sesi: created_at, lalu nomor urut pada ID (bukan leksikografis,
/// agar sim_x_2 tampil sebelum sim_x_10).
fn sort_sessions(mut sessions: Vec<SimSession>) -> Vec<SimSession> {
    sessions.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| id_seq(&a.id).cmp(&id_seq(&b.id)))
            .then_with(|| a.id.cmp(&b.id))
    });
    sessions
}

fn id_seq(id: &str) -> u64 {
    id.rsplit('_')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn wait_for<F: Fn() -> bool>(cond: F) -> bool {
        for _ in 0..200 {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        cond()
    }

    #[test]
    fn test_submit_session() {
        let server = SimServer::new(ServerConfig::default());
        let id = server
            .submit("alice", vec!["counter.sv".into()], 1000)
            .unwrap();
        assert!(id.starts_with("sim_alice_"));
        assert_eq!(server.list_active().len(), 1);
    }

    #[test]
    fn test_user_quota() {
        let server = SimServer::new(ServerConfig::default());
        for _ in 0..4 {
            server.submit("alice", vec![], 100).unwrap();
        }
        assert_eq!(
            server.submit("alice", vec![], 100),
            Err(SubmitError::Quota {
                user: "alice".into(),
                active: 4,
                max: 4
            })
        );
    }

    #[test]
    fn test_cancel() {
        let server = SimServer::new(ServerConfig::default());
        let id = server.submit("alice", vec![], 100).unwrap();
        server.cancel(&id).unwrap();
        let session = server.get_session(&id).unwrap();
        assert!(matches!(session.status, SessionStatus::Cancelled));
    }

    #[test]
    fn test_list_user_sessions() {
        let server = SimServer::new(ServerConfig::default());
        server.submit("alice", vec![], 100).unwrap();
        server.submit("alice", vec![], 100).unwrap();
        server.submit("bob", vec![], 100).unwrap();
        assert_eq!(server.list_user_sessions("alice").len(), 2);
        assert_eq!(server.list_user_sessions("bob").len(), 1);
    }

    #[test]
    fn test_summary() {
        let server = SimServer::new(ServerConfig::default());
        let s = server.summary();
        assert!(s.contains("SimServer"));
    }

    #[test]
    fn test_submit_ids_unique_under_concurrency() {
        let server = Arc::new(SimServer::new(ServerConfig::default()));
        let mut handles = Vec::new();
        for t in 0..8 {
            let s = server.clone();
            handles.push(std::thread::spawn(move || {
                let user = format!("user{}", t);
                (0..4)
                    .map(|_| s.submit(&user, vec![], 100).unwrap())
                    .collect::<Vec<_>>()
            }));
        }
        let mut ids: Vec<String> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "ID sesi harus unik");
    }

    #[test]
    fn test_cancel_queued_removes_from_queue() {
        let cfg = ServerConfig {
            max_total_sessions: 1,
            ..Default::default()
        };
        let server = SimServer::new(cfg);
        server.submit("alice", vec![], 100).unwrap();
        let queued = server.submit("bob", vec![], 100).unwrap();
        assert_eq!(server.queue_depth(), 1);
        server.cancel(&queued).unwrap();
        assert_eq!(server.queue_depth(), 0);
    }

    #[test]
    fn test_list_all_includes_cancelled() {
        let server = SimServer::new(ServerConfig::default());
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("alice", vec![], 100).unwrap();
        server.cancel(&a).unwrap();
        let all = server.list_all();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|s| s.id == a));
        assert!(all.iter().any(|s| s.id == b));
    }

    #[test]
    fn runner_completes_session_with_cycles() {
        let server = SimServer::new(ServerConfig::default());
        server.set_runner(Arc::new(|_files, max_time| Ok(max_time.min(10))));
        let id = server.submit("alice", vec!["a.sv".into()], 500).unwrap();
        assert!(wait_for(|| matches!(
            server.get_session(&id).map(|s| s.status),
            Some(SessionStatus::Completed)
        )));
        let s = server.get_session(&id).unwrap();
        assert_eq!(s.elapsed_cycles, 10);
        assert_eq!(server.queue_depth(), 0);
    }

    #[test]
    fn runner_failure_marks_failed() {
        let server = SimServer::new(ServerConfig::default());
        server.set_runner(Arc::new(|_files, _max_time| Err("boom".to_string())));
        let id = server.submit("alice", vec![], 100).unwrap();
        assert!(wait_for(|| matches!(
            server.get_session(&id).map(|s| s.status),
            Some(SessionStatus::Failed(_))
        )));
        match server.get_session(&id).unwrap().status {
            SessionStatus::Failed(e) => assert_eq!(e, "boom"),
            other => panic!("status tak terduga: {:?}", other),
        }
    }

    #[test]
    fn queued_session_is_promoted_when_slot_frees() {
        let (tx, rx) = mpsc::channel::<()>();
        let rx = Arc::new(Mutex::new(rx));
        let server = SimServer::new(ServerConfig {
            max_total_sessions: 1,
            ..Default::default()
        });
        server.set_runner(Arc::new(move |_files, _max_time| {
            let _ = rx.lock().unwrap().recv_timeout(Duration::from_secs(2));
            Ok(1)
        }));
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("bob", vec![], 100).unwrap();
        assert_eq!(server.queue_depth(), 1);
        assert!(matches!(
            server.get_session(&b).unwrap().status,
            SessionStatus::Queued
        ));
        // Lepas worker pertama → B harus dipromosikan lalu selesai.
        let _ = tx.send(());
        let _ = tx.send(());
        assert!(wait_for(|| {
            matches!(
                server.get_session(&a).map(|s| s.status),
                Some(SessionStatus::Completed)
            ) && matches!(
                server.get_session(&b).map(|s| s.status),
                Some(SessionStatus::Completed)
            )
        }));
        assert_eq!(server.queue_depth(), 0);
    }

    #[test]
    fn cancelling_running_session_promotes_queue() {
        let server = SimServer::new(ServerConfig {
            max_total_sessions: 1,
            ..Default::default()
        });
        server.set_runner(Arc::new(|_files, _max_time| {
            std::thread::sleep(Duration::from_millis(80));
            Ok(2)
        }));
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("bob", vec![], 100).unwrap();
        server.cancel(&a).unwrap();
        // Worker A masih berjalan; hasilnya tidak boleh menimpa Cancelled.
        assert!(wait_for(|| {
            matches!(
                server.get_session(&b).map(|s| s.status),
                Some(SessionStatus::Completed)
            )
        }));
        assert!(matches!(
            server.get_session(&a).unwrap().status,
            SessionStatus::Cancelled
        ));
    }

    #[test]
    fn history_cap_rejects_new_session() {
        let server = SimServer::new(ServerConfig {
            max_sessions_total: 2,
            ..Default::default()
        });
        server.submit("alice", vec![], 100).unwrap();
        server.submit("bob", vec![], 100).unwrap();
        assert_eq!(
            server.submit("carol", vec![], 100),
            Err(SubmitError::Capacity { total: 2, max: 2 })
        );
    }

    #[test]
    fn sort_orders_by_numeric_suffix() {
        let mk = |id: &str| SimSession {
            id: id.to_string(),
            user: "u".into(),
            status: SessionStatus::Running,
            source_files: vec![],
            max_time: 1,
            elapsed_cycles: 0,
            created_at: 7,
        };
        let out = sort_sessions(vec![mk("sim_u_10"), mk("sim_u_2"), mk("sim_u_1")]);
        let ids: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["sim_u_1", "sim_u_2", "sim_u_10"]);
    }
}
