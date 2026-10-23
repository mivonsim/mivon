//! ENT-18: Parallel Multi-User Simulation Server.
//!
//! Manages multiple concurrent simulation sessions for different users.
//! Each session runs independently with resource quotas.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
    pub max_total_sessions: usize,
    pub max_workers: usize,
    pub session_timeout_secs: u64,
    pub user_quotas: HashMap<String, UserQuota>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            max_total_sessions: 32,
            max_workers: 8,
            session_timeout_secs: 600,
            user_quotas: HashMap::new(),
        }
    }
}

/// Simulation server — manages concurrent sessions.
pub struct SimServer {
    config: ServerConfig,
    sessions: Arc<Mutex<HashMap<String, SimSession>>>,
    queue: Arc<Mutex<Vec<String>>>,
    start_time: Instant,
}

impl SimServer {
    pub fn new(config: ServerConfig) -> Self {
        SimServer {
            config,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            queue: Arc::new(Mutex::new(Vec::new())),
            start_time: Instant::now(),
        }
    }

    /// Submit a new simulation session.
    ///
    /// ID dihitung di dalam lock yang sama dengan insert — dua submit paralel
    /// tidak boleh mendapat ID yang sama (dashboard memakai ID untuk cancel).
    pub fn submit(&self, user: &str, files: Vec<String>, max_time: u64) -> Result<String, String> {
        let mut sessions = self.sessions.lock().unwrap();
        let user_sessions: usize = sessions
            .values()
            .filter(|s| {
                s.user == user && matches!(s.status, SessionStatus::Running | SessionStatus::Queued)
            })
            .count();

        // Check user quota
        let quota = self.config.user_quotas.get(user);
        let max_concurrent = quota.map(|q| q.max_concurrent_sessions).unwrap_or(4);
        if user_sessions >= max_concurrent {
            return Err(format!(
                "user {} has {} sessions (max {})",
                user, user_sessions, max_concurrent
            ));
        }

        // Check total capacity: penuh → antri (bukan ditolak)
        let total_active: usize = sessions
            .values()
            .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Queued))
            .count();
        let queued = total_active >= self.config.max_total_sessions;

        let id = format!("sim_{}_{}", user, next_id(&sessions));
        let session = SimSession {
            id: id.clone(),
            user: user.to_string(),
            status: if queued {
                SessionStatus::Queued
            } else {
                SessionStatus::Running
            },
            source_files: files,
            max_time,
            elapsed_cycles: 0,
            created_at: now_secs(),
        };
        sessions.insert(id.clone(), session);
        drop(sessions);

        if queued {
            self.queue.lock().unwrap().push(id.clone());
        }
        Ok(id)
    }

    /// Get session status.
    pub fn get_session(&self, id: &str) -> Option<SimSession> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    /// Cancel a session.
    ///
    /// Sesi yang dibatalkan juga dikeluarkan dari antrean agar queue_depth
    /// tidak menghitung sesi mati.
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        {
            let mut sessions = self.sessions.lock().unwrap();
            let session = sessions.get_mut(id).ok_or("session not found")?;
            match session.status {
                SessionStatus::Running | SessionStatus::Queued => {
                    session.status = SessionStatus::Cancelled;
                }
                _ => return Err("session not in cancellable state".into()),
            }
        }
        self.queue.lock().unwrap().retain(|q| q != id);
        Ok(())
    }

    /// List sessions for a user.
    pub fn list_user_sessions(&self, user: &str) -> Vec<SimSession> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.user == user)
            .cloned()
            .collect()
    }

    /// List all active sessions.
    pub fn list_active(&self) -> Vec<SimSession> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Queued))
            .cloned()
            .collect()
    }

    /// Semua sesi (termasuk completed/failed/cancelled), urut created_at lalu id.
    pub fn list_all(&self) -> Vec<SimSession> {
        let mut out: Vec<SimSession> = self.sessions.lock().unwrap().values().cloned().collect();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        out
    }

    /// Queue depth — hanya sesi yang MASIH berstatus Queued (entri antrean
    /// basi dari sesi yang dibatalkan/selesai tidak dihitung).
    pub fn queue_depth(&self) -> usize {
        let sessions = self.sessions.lock().unwrap();
        let mut queue = self.queue.lock().unwrap();
        queue.retain(|id| {
            matches!(
                sessions.get(id).map(|s| &s.status),
                Some(SessionStatus::Queued)
            )
        });
        queue.len()
    }

    /// Summary.
    pub fn summary(&self) -> String {
        let sessions = self.sessions.lock().unwrap();
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

/// ID berikutnya — dipanggil sambil memegang lock sessions (tanpa lock ulang).
fn next_id(sessions: &HashMap<String, SimSession>) -> u64 {
    sessions.len() as u64 + 1
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
        let result = server.submit("alice", vec![], 100);
        assert!(result.is_err());
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
        use std::sync::Arc;
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
}
