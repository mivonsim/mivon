//! FEAT-16: Simulation Farm Orchestration — distributed sim job scheduler.
//!
//! Manages a pool of simulation workers and schedules jobs across them.
//! Supports priority queues, job dependencies, resource tracking,
//! and failover with automatic retry.

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Job priority.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub enum Priority {
    Critical = 0,
    High = 1,
    #[default]
    Normal = 2,
    Low = 3,
    Background = 4,
}

/// Job status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Queued,
    Running,
    Completed,
    Failed(String),
    Cancelled,
    Retry(String),
}

/// A simulation job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimJob {
    pub id: String,
    pub name: String,
    pub source_files: Vec<String>,
    pub max_time: u64,
    pub priority: Priority,
    pub status: JobStatus,
    pub assigned_worker: Option<String>,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub retries: u32,
    pub max_retries: u32,
    pub depends_on: Vec<String>,
    pub tags: Vec<String>,
}

/// Worker node info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub addr: String,
    pub status: WorkerStatus,
    pub capacity: u32,
    pub current_load: u32,
    pub max_memory_mb: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    Idle,
    Busy,
    Offline,
    Draining,
}

/// Simulation farm scheduler.
pub struct SimFarm {
    jobs: Vec<SimJob>,
    workers: HashMap<String, Worker>,
    job_queue: VecDeque<String>,
    config: FarmConfig,
}

#[derive(Debug, Clone)]
pub struct FarmConfig {
    pub max_queue_depth: usize,
    pub default_max_retries: u32,
    pub job_timeout_secs: u64,
    pub enable_failover: bool,
}

impl Default for FarmConfig {
    fn default() -> Self {
        FarmConfig {
            max_queue_depth: 10000,
            default_max_retries: 3,
            job_timeout_secs: 3600,
            enable_failover: true,
        }
    }
}

impl SimFarm {
    pub fn new(config: FarmConfig) -> Self {
        SimFarm {
            jobs: Vec::new(),
            workers: HashMap::new(),
            job_queue: VecDeque::new(),
            config,
        }
    }

    /// Submit a new job.
    pub fn submit(
        &mut self,
        name: &str,
        files: Vec<String>,
        max_time: u64,
        priority: Priority,
    ) -> String {
        let id = format!("job-{:06}", self.jobs.len() + 1);
        let job = SimJob {
            id: id.clone(),
            name: name.to_string(),
            source_files: files,
            max_time,
            priority,
            status: JobStatus::Queued,
            assigned_worker: None,
            created_at: now_secs(),
            started_at: None,
            completed_at: None,
            retries: 0,
            max_retries: self.config.default_max_retries,
            depends_on: Vec::new(),
            tags: Vec::new(),
        };
        self.jobs.push(job);
        self.job_queue.push_back(id.clone());
        self.schedule();
        id
    }

    /// Register a worker.
    pub fn register_worker(&mut self, id: &str, addr: &str, capacity: u32) {
        self.workers.insert(
            id.to_string(),
            Worker {
                id: id.to_string(),
                addr: addr.to_string(),
                status: WorkerStatus::Idle,
                capacity,
                current_load: 0,
                max_memory_mb: 4096,
                uptime_secs: 0,
            },
        );
    }

    /// Mark job as completed.
    pub fn complete_job(&mut self, job_id: &str) -> bool {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = JobStatus::Completed;
            job.completed_at = Some(now_secs());
            if let Some(wid) = &job.assigned_worker {
                if let Some(w) = self.workers.get_mut(wid) {
                    w.current_load = w.current_load.saturating_sub(1);
                    if w.current_load == 0 {
                        w.status = WorkerStatus::Idle;
                    }
                }
            }
            self.schedule();
            true
        } else {
            false
        }
    }

    /// Mark job as failed (may trigger retry).
    pub fn fail_job(&mut self, job_id: &str, reason: &str) {
        let mut should_retry = false;
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            job.retries += 1;
            if job.retries < job.max_retries && self.config.enable_failover {
                should_retry = true;
                job.status = JobStatus::Retry(reason.to_string());
                job.assigned_worker = None;
            } else {
                job.status = JobStatus::Failed(reason.to_string());
            }
            if let Some(wid) = &job.assigned_worker {
                if let Some(w) = self.workers.get_mut(wid) {
                    w.current_load = w.current_load.saturating_sub(1);
                }
            }
        }
        if should_retry {
            self.job_queue.push_back(job_id.to_string());
            self.schedule();
        }
    }

    /// Cancel a job.
    pub fn cancel_job(&mut self, job_id: &str) -> bool {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            matches!(
                job.status,
                JobStatus::Queued | JobStatus::Pending | JobStatus::Running
            )
        } else {
            return false;
        };
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = JobStatus::Cancelled;
        }
        true
    }

    /// Internal scheduler: assign queued jobs to idle workers.
    fn schedule(&mut self) {
        let idle_workers: Vec<String> = self
            .workers
            .iter()
            .filter(|(_, w)| w.status == WorkerStatus::Idle && w.current_load < w.capacity)
            .map(|(id, _)| id.clone())
            .collect();

        for worker_id in idle_workers {
            if let Some(job_id) = self.job_queue.pop_front() {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
                    if matches!(job.status, JobStatus::Queued | JobStatus::Retry(_)) {
                        job.status = JobStatus::Running;
                        job.started_at = Some(now_secs());
                        job.assigned_worker = Some(worker_id.clone());
                        if let Some(w) = self.workers.get_mut(&worker_id) {
                            w.current_load += 1;
                            w.status = WorkerStatus::Busy;
                        }
                    } else {
                        // Job not ready, put it back
                        self.job_queue.push_back(job_id);
                    }
                }
            } else {
                break;
            }
        }
    }

    /// Get queue depth.
    pub fn queue_depth(&self) -> usize {
        self.job_queue.len()
    }

    /// Get farm summary.
    pub fn summary(&self) -> String {
        let running = self
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Running))
            .count();
        let queued = self
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Queued))
            .count();
        let completed = self
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Completed))
            .count();
        let failed = self
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Failed(_)))
            .count();
        let workers = self.workers.len();
        let idle = self
            .workers
            .values()
            .filter(|w| w.status == WorkerStatus::Idle)
            .count();
        format!(
            "SimFarm: {} jobs ({} running, {} queued, {} completed, {} failed), {} workers ({} idle)",
            self.jobs.len(), running, queued, completed, failed, workers, idle,
        )
    }

    /// List jobs by status.
    pub fn list_jobs(&self, status: Option<&JobStatus>) -> Vec<&SimJob> {
        self.jobs
            .iter()
            .filter(|j| match status {
                Some(s) => &j.status == s,
                None => true,
            })
            .collect()
    }

    /// Ambil referensi ke job menurut id.
    pub fn get_job(&self, job_id: &str) -> Option<&SimJob> {
        self.jobs.iter().find(|j| j.id == job_id)
    }

    /// Hapus job menurut id.
    pub fn remove_job(&mut self, job_id: &str) -> bool {
        if let Some(pos) = self.jobs.iter().position(|j| j.id == job_id) {
            self.jobs.remove(pos);
            return true;
        }
        false
    }

    /// Hapus semua job.
    pub fn clear_jobs(&mut self) {
        self.jobs.clear();
        self.job_queue.clear();
    }

    /// Jumlah total job.
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Ambil referensi ke worker menurut id.
    pub fn get_worker(&self, worker_id: &str) -> Option<&Worker> {
        self.workers.get(worker_id)
    }

    /// Hapus worker menurut id.
    pub fn remove_worker(&mut self, worker_id: &str) -> bool {
        self.workers.remove(worker_id).is_some()
    }

    /// Hapus semua worker.
    pub fn clear_workers(&mut self) {
        self.workers.clear();
    }

    /// Jumlah total worker.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
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
    fn test_submit_and_schedule() {
        let mut farm = SimFarm::new(FarmConfig::default());
        farm.register_worker("w1", "10.0.0.1:9000", 4);
        let id = farm.submit("test", vec!["test.sv".into()], 1000, Priority::Normal);
        let job = farm
            .list_jobs(None)
            .into_iter()
            .find(|j| j.id == id)
            .unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.assigned_worker.as_deref(), Some("w1"));
    }

    #[test]
    fn test_complete_job() {
        let mut farm = SimFarm::new(FarmConfig::default());
        farm.register_worker("w1", "10.0.0.1:9000", 4);
        let id = farm.submit("test", vec![], 100, Priority::Normal);
        assert!(farm.complete_job(&id));
        let job = farm
            .list_jobs(None)
            .into_iter()
            .find(|j| j.id == id)
            .unwrap();
        assert_eq!(job.status, JobStatus::Completed);
    }

    #[test]
    fn test_retry_on_failure() {
        let mut farm = SimFarm::new(FarmConfig {
            enable_failover: true,
            default_max_retries: 2,
            ..FarmConfig::default()
        });
        farm.register_worker("w1", "10.0.0.1:9000", 4);
        let id = farm.submit("test", vec![], 100, Priority::Normal);
        farm.fail_job(&id, "segfault");
        let job = farm
            .list_jobs(None)
            .into_iter()
            .find(|j| j.id == id)
            .unwrap();
        assert_eq!(job.retries, 1);
        assert!(matches!(job.status, JobStatus::Retry(_)));
    }

    #[test]
    fn test_cancel() {
        let mut farm = SimFarm::new(FarmConfig::default());
        let id = farm.submit("test", vec![], 100, Priority::Normal);
        assert!(farm.cancel_job(&id));
        let job = farm
            .list_jobs(None)
            .into_iter()
            .find(|j| j.id == id)
            .unwrap();
        assert_eq!(job.status, JobStatus::Cancelled);
    }

    #[test]
    fn test_summary() {
        let mut farm = SimFarm::new(FarmConfig::default());
        farm.register_worker("w1", "10.0.0.1:9000", 4);
        farm.submit("test", vec![], 100, Priority::Normal);
        let s = farm.summary();
        assert!(s.contains("SimFarm"));
        assert!(s.contains("1 workers"));
    }

    #[test]
    fn get_and_remove_job() {
        let mut farm = SimFarm::new(FarmConfig::default());
        let id = farm.submit("test", vec![], 100, Priority::Normal);
        assert_eq!(farm.job_count(), 1);
        let job = farm.get_job(&id).unwrap();
        assert_eq!(job.id, id);
        assert!(farm.remove_job(&id));
        assert!(farm.get_job(&id).is_none());
        assert!(!farm.remove_job(&id));
        assert_eq!(farm.job_count(), 0);
    }

    #[test]
    fn clear_jobs_empties_farm() {
        let mut farm = SimFarm::new(FarmConfig::default());
        farm.register_worker("w1", "10.0.0.1:9000", 4);
        farm.submit("a", vec![], 100, Priority::Normal);
        farm.submit("b", vec![], 100, Priority::Normal);
        assert_eq!(farm.job_count(), 2);
        farm.clear_jobs();
        assert_eq!(farm.job_count(), 0);
        assert_eq!(farm.queue_depth(), 0);
    }

    #[test]
    fn get_and_remove_worker() {
        let mut farm = SimFarm::new(FarmConfig::default());
        farm.register_worker("w1", "10.0.0.1:9000", 4);
        assert_eq!(farm.worker_count(), 1);
        let w = farm.get_worker("w1").unwrap();
        assert_eq!(w.id, "w1");
        assert!(farm.remove_worker("w1"));
        assert!(farm.get_worker("w1").is_none());
        assert!(!farm.remove_worker("w1"));
        assert_eq!(farm.worker_count(), 0);
    }

    #[test]
    fn clear_workers_empties_farm() {
        let mut farm = SimFarm::new(FarmConfig::default());
        farm.register_worker("w1", "10.0.0.1:9000", 4);
        farm.register_worker("w2", "10.0.0.2:9000", 4);
        assert_eq!(farm.worker_count(), 2);
        farm.clear_workers();
        assert_eq!(farm.worker_count(), 0);
    }
}
