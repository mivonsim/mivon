//! ENT-18: Parallel Multi-User Simulation Server.
//!
//! Mengelola sesi simulasi konkuren untuk banyak user: kuota per user,
//! kapasitas worker, antrean FIFO, dan eksekusi lewat *runner* yang
//! di-inject pemanggil (`set_runner`). Runner wajib dipasang sebelum submit —
//! tanpa runner, submit ditolak (`SubmitError::NoRunner`) agar tidak ada sesi
//! Running yang tak pernah selesai.
//!
//! Urutan lock yang WAJIB dipertahankan: `sessions` → `queue`. Tidak ada
//! jalur yang mengambil `queue` lalu `sessions` (mencegah ABBA deadlock).
//! Lock memakai `unwrap_or_else(|e| e.into_inner())` supaya panic di satu
//! sesi tidak mem-poison seluruh server.
//!
//! Slot eksekusi dihitung dengan `Store::active` (AtomicUsize) dan direservasi
//! secara atomik SEBELUM worker di-spawn, sehingga `max_workers` benar-benar
//! membatasi simulasi yang berjalan bersamaan (bukan hanya status Running).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    /// Waktu simulasi akhir yang dicapai runner (satuan waktu simulator,
    /// bukan jumlah clock cycle).
    pub elapsed_time: u64,
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
    /// Batas jumlah sesi yang disimpan (riwayat). Saat penuh, sesi terminal
    /// tertua dipangkas; bila semua masih aktif → `SubmitError::Capacity`.
    pub max_sessions_total: usize,
    /// Batas worker paralel (efektif: min(max_workers, max_total_sessions)).
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

impl ServerConfig {
    /// Jumlah worker yang boleh berjalan bersamaan.
    fn slots(&self) -> usize {
        self.max_workers.max(1).min(self.max_total_sessions.max(1))
    }
}

/// Error submit — dipetakan ke status HTTP oleh pemanggil
/// (kuota → 429, kapasitas → 503, tanpa runner → 503).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    Quota { user: String, active: usize, max: usize },
    Capacity { total: usize, max: usize },
    NoRunner,
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
            SubmitError::NoRunner => write!(f, "server belum punya runner eksekusi"),
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

/// Fungsi eksekusi sesi: (files, max_time) → waktu simulasi akhir.
pub type SessionRunner = Arc<dyn Fn(&[String], u64) -> Result<u64, String> + Send + Sync>;

/// State bersama — dipisah dari `SimServer` agar worker thread bisa memegang
/// `Arc` tanpa meminjam server.
struct Store {
    sessions: Mutex<HashMap<String, SimSession>>,
    queue: Mutex<Vec<String>>,
    runner: Mutex<Option<SessionRunner>>,
    next_id: AtomicU64,
    /// Slot worker yang sedang dipakai (direservasi sebelum spawn).
    active: AtomicUsize,
}

impl Store {
    fn sessions(&self) -> MutexGuard<'_, HashMap<String, SimSession>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn queue(&self) -> MutexGuard<'_, Vec<String>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reservasi satu slot worker (atomik). `false` bila slot habis.
    fn try_acquire_slot(&self, max: usize) -> bool {
        let mut cur = self.active.load(Ordering::SeqCst);
        loop {
            if cur >= max {
                return false;
            }
            match self
                .active
                .compare_exchange_weak(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(observed) => cur = observed,
            }
        }
    }

    fn release_slot(&self) {
        // Guard underflow: release hanya bila > 0. Di jalur normal 1 acquire
        // selalu berpasangan dengan 1 release, tapi ini mencegah kebocoran
        // slot berubah jadi DoS total bila ada path tak seimbang.
        if self.active.load(Ordering::SeqCst) > 0 {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
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
                active: AtomicUsize::new(0),
            }),
            start_time: Instant::now(),
        }
    }

    /// Pasang runner eksekusi (wajib sebelum submit).
    pub fn set_runner(&self, runner: SessionRunner) {
        *self
            .store
            .runner
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(runner);
    }

    /// True bila runner sudah terpasang.
    pub fn has_runner(&self) -> bool {
        self.store
            .runner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Submit a new simulation session.
    ///
    /// Sesi selalu masuk antrean dulu lalu dipromosikan lewat `promote()`,
    /// sehingga tidak ada sesi baru yang menyalip sesi lama (FIFO) dan slot
    /// worker tidak pernah terlampaui.
    pub fn submit(
        &self,
        user: &str,
        files: Vec<String>,
        max_time: u64,
    ) -> Result<String, SubmitError> {
        if !self.has_runner() {
            return Err(SubmitError::NoRunner);
        }

        let mut sessions = self.store.sessions();

        if sessions.len() >= self.config.max_sessions_total
            && !evict_oldest_terminal(&mut sessions)
        {
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

        let id = format!(
            "sim_{}_{}",
            user,
            self.store.next_id.fetch_add(1, Ordering::SeqCst) + 1
        );
        let session = SimSession {
            id: id.clone(),
            user: user.to_string(),
            status: SessionStatus::Queued,
            source_files: files,
            max_time,
            elapsed_time: 0,
            created_at: now_secs(),
        };
        sessions.insert(id.clone(), session);
        // Push antrean masih di dalam guard sessions → status Queued dan
        // entri antrean selalu konsisten.
        self.store.queue().push(id.clone());
        drop(sessions);

        promote(&self.store, &self.config);
        Ok(id)
    }

    /// Get session status.
    pub fn get_session(&self, id: &str) -> Option<SimSession> {
        self.store.sessions().get(id).cloned()
    }

    /// Cancel a session.
    ///
    /// Sesi yang dibatalkan dikeluarkan dari antrean. Worker yang sedang
    /// berjalan tidak bisa diinterupsi, jadi slot barunya baru dipakai
    /// setelah worker itu benar-benar selesai (lihat `active`).
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

    /// Jumlah worker yang sedang berjalan.
    pub fn active_workers(&self) -> usize {
        self.store.active.load(Ordering::SeqCst)
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

/// Pangkas sesi terminal tertua (completed/failed/cancelled). Return false
/// bila semua sesi masih aktif (tidak ada yang bisa dipangkas).
fn evict_oldest_terminal(sessions: &mut HashMap<String, SimSession>) -> bool {
    let oldest = sessions
        .values()
        .filter(|s| {
            matches!(
                s.status,
                SessionStatus::Completed
                    | SessionStatus::Failed(_)
                    | SessionStatus::Cancelled
            )
        })
        .min_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| id_seq(&a.id).cmp(&id_seq(&b.id)))
        })
        .map(|s| s.id.clone());
    match oldest {
        Some(id) => {
            sessions.remove(&id);
            true
        }
        None => false,
    }
}

/// Jalankan runner untuk satu sesi di thread terpisah. Slot HARUS sudah
/// direservasi pemanggil; fungsi ini melepasnya saat worker selesai.
fn start_worker(
    store: Arc<Store>,
    config: ServerConfig,
    id: String,
    files: Vec<String>,
    max_time: u64,
) -> bool {
    let runner = store
        .runner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some(runner) = runner else {
        return false;
    };
    // Pakai Builder::spawn (bukan std::thread::spawn) agar kegagalan pembuatan
    // thread oleh OS ditangkap sebagai Err, bukan panic di caller. Bila spawn
    // gagal, kembalikan false agar pemanggil (promote) melepas slot & mengembalikan
    // status sesi ke Queued (tidak macet Running tanpa worker).
    let handle = std::thread::Builder::new()
        .name(format!("sim-worker-{}", id))
        .spawn(move || {
            // Panic di runner (parser/elaborator atas input klien) tidak boleh
            // membuat sesi Running selamanya.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runner(&files, max_time)
            }))
            .unwrap_or_else(|_| Err("runner panic".to_string()));
            store.release_slot();
            finish_session(&store, &config, &id, outcome);
        });
    match handle {
        Ok(_) => true,
        Err(_) => false,
    }
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
                    Ok(elapsed) => {
                        s.elapsed_time = elapsed;
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

/// Promosikan sesi antrean selama masih ada slot worker bebas (FIFO).
fn promote(store: &Arc<Store>, config: &ServerConfig) {
    loop {
        let next = {
            let mut sessions = store.sessions();
            let mut queue = store.queue();
            loop {
                let Some(id) = queue.first().cloned() else {
                    return;
                };
                queue.remove(0);
                let promote_now = matches!(
                    sessions.get(&id).map(|s| &s.status),
                    Some(SessionStatus::Queued)
                );
                if !promote_now {
                    // Entri basi (sesi dibatalkan) — lewati.
                    continue;
                }
                if !store.try_acquire_slot(config.slots()) {
                    // Tidak ada slot: kembalikan ke depan antrean.
                    queue.insert(0, id);
                    return;
                }
                match sessions.get_mut(&id) {
                    Some(s) => {
                        s.status = SessionStatus::Running;
                        break Some((id, s.source_files.clone(), s.max_time));
                    }
                    None => {
                        store.release_slot();
                        continue;
                    }
                }
            }
        };
        match next {
            Some((id, files, max_time)) => {
                if !start_worker(store.clone(), config.clone(), id.clone(), files, max_time) {
                    // Runner hilang di tengah jalan — kembalikan slot & status.
                    store.release_slot();
                    let mut sessions = store.sessions();
                    if let Some(s) = sessions.get_mut(&id) {
                        if matches!(s.status, SessionStatus::Running) {
                            s.status = SessionStatus::Queued;
                            store.queue().insert(0, id);
                        }
                    }
                    return;
                }
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
        for _ in 0..300 {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        cond()
    }

    /// Runner yang menahan sesi sampai sender di-drop (agar status Running
    /// deterministik pada test).
    fn held_runner() -> (SessionRunner, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel::<()>();
        let rx = Arc::new(Mutex::new(rx));
        let runner: SessionRunner = Arc::new(move |_files, _max_time| {
            let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
            match guard.recv() {
                Ok(()) => Ok(1),
                Err(_) => Err("released".to_string()),
            }
        });
        (runner, tx)
    }

    fn server_with_runner(config: ServerConfig) -> (SimServer, mpsc::Sender<()>) {
        let server = SimServer::new(config);
        let (runner, tx) = held_runner();
        server.set_runner(runner);
        (server, tx)
    }

    #[test]
    fn test_submit_session() {
        let (server, _tx) = server_with_runner(ServerConfig::default());
        let id = server
            .submit("alice", vec!["counter.sv".into()], 1000)
            .unwrap();
        assert!(id.starts_with("sim_alice_"));
        assert_eq!(server.list_active().len(), 1);
        assert!(matches!(
            server.get_session(&id).unwrap().status,
            SessionStatus::Running
        ));
    }

    #[test]
    fn submit_without_runner_is_rejected() {
        let server = SimServer::new(ServerConfig::default());
        assert_eq!(
            server.submit("alice", vec![], 100),
            Err(SubmitError::NoRunner)
        );
    }

    #[test]
    fn test_user_quota() {
        let (server, _tx) = server_with_runner(ServerConfig::default());
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
        let (server, _tx) = server_with_runner(ServerConfig::default());
        let id = server.submit("alice", vec![], 100).unwrap();
        server.cancel(&id).unwrap();
        let session = server.get_session(&id).unwrap();
        assert!(matches!(session.status, SessionStatus::Cancelled));
    }

    #[test]
    fn test_list_user_sessions() {
        let (server, _tx) = server_with_runner(ServerConfig::default());
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
        let (server, _tx) = server_with_runner(ServerConfig::default());
        let server = Arc::new(server);
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
        let (server, _tx) = server_with_runner(ServerConfig {
            max_total_sessions: 1,
            max_workers: 1,
            ..Default::default()
        });
        server.submit("alice", vec![], 100).unwrap();
        let queued = server.submit("bob", vec![], 100).unwrap();
        assert_eq!(server.queue_depth(), 1);
        server.cancel(&queued).unwrap();
        assert_eq!(server.queue_depth(), 0);
    }

    #[test]
    fn test_list_all_includes_cancelled() {
        let (server, _tx) = server_with_runner(ServerConfig::default());
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("alice", vec![], 100).unwrap();
        server.cancel(&a).unwrap();
        let all = server.list_all();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|s| s.id == a));
        assert!(all.iter().any(|s| s.id == b));
    }

    #[test]
    fn runner_completes_session_with_time() {
        let server = SimServer::new(ServerConfig::default());
        server.set_runner(Arc::new(|_files, max_time| Ok(max_time.min(10))));
        let id = server.submit("alice", vec!["a.sv".into()], 500).unwrap();
        assert!(wait_for(|| matches!(
            server.get_session(&id).map(|s| s.status),
            Some(SessionStatus::Completed)
        )));
        let s = server.get_session(&id).unwrap();
        assert_eq!(s.elapsed_time, 10);
        assert_eq!(server.queue_depth(), 0);
        assert_eq!(server.active_workers(), 0);
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
        assert_eq!(server.active_workers(), 0);
    }

    #[test]
    fn runner_panic_marks_failed_and_frees_slot() {
        let server = SimServer::new(ServerConfig {
            max_workers: 1,
            ..Default::default()
        });
        server.set_runner(Arc::new(|_files, _max_time| panic!("engine rusak")));
        let id = server.submit("alice", vec![], 100).unwrap();
        assert!(wait_for(|| matches!(
            server.get_session(&id).map(|s| s.status),
            Some(SessionStatus::Failed(_))
        )));
        assert_eq!(server.active_workers(), 0);
        // Slot bebas lagi → submit berikutnya tetap bisa jalan.
        let next = server.submit("bob", vec![], 100).unwrap();
        assert!(wait_for(|| matches!(
            server.get_session(&next).map(|s| s.status),
            Some(SessionStatus::Failed(_))
        )));
    }

    #[test]
    fn max_workers_limits_parallel_execution() {
        let (server, _tx) = server_with_runner(ServerConfig {
            max_total_sessions: 8,
            max_workers: 1,
            ..Default::default()
        });
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("bob", vec![], 100).unwrap();
        assert_eq!(server.active_workers(), 1);
        assert!(matches!(
            server.get_session(&a).unwrap().status,
            SessionStatus::Running
        ));
        assert!(matches!(
            server.get_session(&b).unwrap().status,
            SessionStatus::Queued
        ));
        assert_eq!(server.queue_depth(), 1);
    }

    #[test]
    fn queued_session_is_promoted_when_slot_frees() {
        let (tx, rx) = mpsc::channel::<()>();
        let rx = Arc::new(Mutex::new(rx));
        let server = SimServer::new(ServerConfig {
            max_total_sessions: 1,
            max_workers: 1,
            ..Default::default()
        });
        server.set_runner(Arc::new(move |_files, _max_time| {
            let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
            guard.recv().map(|_| 1).map_err(|e| e.to_string())
        }));
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("bob", vec![], 100).unwrap();
        assert_eq!(server.queue_depth(), 1);
        let _ = tx.send(());
        assert!(wait_for(|| {
            matches!(
                server.get_session(&a).map(|s| s.status),
                Some(SessionStatus::Completed)
            )
        }));
        // B dipromosikan setelah slot A benar-benar bebas.
        let _ = tx.send(());
        assert!(wait_for(|| {
            matches!(
                server.get_session(&b).map(|s| s.status),
                Some(SessionStatus::Completed)
            )
        }));
        assert_eq!(server.queue_depth(), 0);
    }

    #[test]
    fn cancelling_running_session_does_not_oversubscribe() {
        let (server, _tx) = server_with_runner(ServerConfig {
            max_total_sessions: 1,
            max_workers: 1,
            ..Default::default()
        });
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("bob", vec![], 100).unwrap();
        server.cancel(&a).unwrap();
        // Worker A belum berhenti → slot masih dipakai, B tetap antre.
        assert_eq!(server.active_workers(), 1);
        assert!(matches!(
            server.get_session(&b).unwrap().status,
            SessionStatus::Queued
        ));
        assert!(matches!(
            server.get_session(&a).unwrap().status,
            SessionStatus::Cancelled
        ));
    }

    #[test]
    fn new_submit_does_not_overtake_queue() {
        let (server, _tx) = server_with_runner(ServerConfig {
            max_total_sessions: 2,
            max_workers: 2,
            ..Default::default()
        });
        let _a = server.submit("alice", vec![], 100).unwrap();
        let _b = server.submit("bob", vec![], 100).unwrap();
        let c = server.submit("carol", vec![], 100).unwrap();
        // C antre karena slot penuh.
        assert!(matches!(
            server.get_session(&c).unwrap().status,
            SessionStatus::Queued
        ));
        // Bebaskan satu slot: yang naik harus C (FIFO), bukan submit baru.
        let d = server.submit("dave", vec![], 100).unwrap();
        assert!(matches!(
            server.get_session(&d).unwrap().status,
            SessionStatus::Queued
        ));
        assert_eq!(server.queue_depth(), 2);
    }

    #[test]
    fn history_cap_evicts_oldest_terminal() {
        let server = SimServer::new(ServerConfig {
            max_sessions_total: 2,
            ..Default::default()
        });
        server.set_runner(Arc::new(|_files, _max_time| Ok(1)));
        let a = server.submit("alice", vec![], 100).unwrap();
        let b = server.submit("bob", vec![], 100).unwrap();
        assert!(wait_for(|| {
            matches!(
                server.get_session(&a).map(|s| s.status),
                Some(SessionStatus::Completed)
            ) && matches!(
                server.get_session(&b).map(|s| s.status),
                Some(SessionStatus::Completed)
            )
        }));
        // Riwayat penuh (2) tetapi keduanya terminal → yang tertua dipangkas.
        let c = server.submit("carol", vec![], 100).unwrap();
        assert_eq!(server.list_all().len(), 2);
        assert!(server.get_session(&a).is_none(), "sesi tertua dipangkas");
        assert!(server.get_session(&c).is_some());
    }

    #[test]
    fn history_cap_rejects_when_all_active() {
        let (server, _tx) = server_with_runner(ServerConfig {
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
            elapsed_time: 0,
            created_at: 7,
        };
        let out = sort_sessions(vec![mk("sim_u_10"), mk("sim_u_2"), mk("sim_u_1")]);
        let ids: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["sim_u_1", "sim_u_2", "sim_u_10"]);
    }
}
