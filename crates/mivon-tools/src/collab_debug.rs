//! FEAT-02: Collaborative debug — multi-user debugging session foundation.
//! Stage 1 stub (High, open).
//!
//! Model sesi debugging multi-user: satu sesi debug yang bisa diikuti
//! beberapa pengguna secara bersamaan, dengan snapshot breakpoints/watchpoints
//! yang dishare. Implementasi server/WebSocket nyata BELUM ada — stage 1
//! adalah model data + tes yang mendokumentasikan gap.
//! Belum di-wire ke debugger/LSP/server.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Status satu sesi debugging kolaboratif.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugSessionStatus {
    Waiting,    // menunggu peserta bergabung
    Running,    // simulasi berjalan, semua peserta melihat state yang sama
    Paused,     // semua peserta di breakpoint/watchpoint yang sama
    Stopped,    // sesi selesai
}

/// Titik henti yang dishare antar peserta sesi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedBreakpoint {
    pub id: String,
    pub signal: String,
    pub condition: Option<String>,
    pub hit_count: u64,
}

/// Sesi debugging multi-user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugSession {
    pub session_id: String,
    pub owner: String,
    pub participants: Vec<String>,
    pub status: DebugSessionStatus,
    pub breakpoints: HashMap<String, SharedBreakpoint>,
    pub current_time: u64,
}

impl DebugSession {
    pub fn new(session_id: &str, owner: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            owner: owner.to_string(),
            participants: vec![owner.to_string()],
            status: DebugSessionStatus::Waiting,
            breakpoints: HashMap::new(),
            current_time: 0,
        }
    }

    pub fn add_participant(&mut self, user: &str) {
        if !self.participants.contains(&user.to_string()) {
            self.participants.push(user.to_string());
        }
        if self.status == DebugSessionStatus::Waiting && self.participants.len() >= 2 {
            self.status = DebugSessionStatus::Running;
        }
    }

    pub fn set_breakpoint(&mut self, bp: SharedBreakpoint) {
        self.breakpoints.insert(bp.id.clone(), bp);
    }

    pub fn remove_breakpoint(&mut self, id: &str) {
        self.breakpoints.remove(id);
    }

    pub fn advance_time(&mut self, t: u64) {
        self.current_time = t;
    }

    pub fn participant_count(&self) -> usize {
        self.participants.len()
    }

    pub fn is_multi_user(&self) -> bool {
        self.participants.len() > 1
    }
}

/// Manajer sesi debug kolaboratif (in-memory, stage 1).
#[derive(Debug, Clone, Default)]
pub struct CollabDebugManager {
    sessions: Arc<Mutex<HashMap<String, DebugSession>>>,
}

impl CollabDebugManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_session(&self, session_id: &str, owner: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(session_id) {
            return false;
        }
        sessions.insert(session_id.to_string(), DebugSession::new(session_id, owner));
        true
    }

    pub fn join_session(&self, session_id: &str, user: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(session_id) {
            Some(s) => {
                s.add_participant(user);
                true
            }
            None => false,
        }
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_and_join_multi_user() {
        let mgr = CollabDebugManager::new();
        assert!(mgr.create_session("s1", "alice"));
        assert!(!mgr.create_session("s1", "bob")); // duplikat id ditolak
        assert!(mgr.join_session("s1", "bob"));
        let guard = mgr.sessions.lock().unwrap();
        let s = guard.get("s1").unwrap();
        assert!(s.is_multi_user());
        assert_eq!(s.participant_count(), 2);
        assert_eq!(s.status, DebugSessionStatus::Running);
        drop(guard);
    }

    #[test]
    fn join_nonexistent_session_rejected() {
        let mgr = CollabDebugManager::new();
        assert!(!mgr.join_session("nope", "alice"));
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn shared_breakpoint_add_remove() {
        let mut s = DebugSession::new("s1", "alice");
        s.add_participant("bob");
        s.set_breakpoint(SharedBreakpoint {
            id: "bp1".into(),
            signal: "clk".into(),
            condition: Some("clk == 1".into()),
            hit_count: 0,
        });
        assert_eq!(s.breakpoints.len(), 1);
        s.remove_breakpoint("bp1");
        assert!(s.breakpoints.is_empty());
    }

    #[test]
    fn single_user_not_multi_user() {
        let s = DebugSession::new("s1", "alice");
        assert!(!s.is_multi_user());
        assert_eq!(s.status, DebugSessionStatus::Waiting);
    }
}
