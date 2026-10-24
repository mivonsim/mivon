use crate::env::config::ConfigContext;
use crate::env::runtime::{CpuInfo, MemoryInfo, ThreadPoolHandle};
use std::path::PathBuf;
use std::sync::Arc;

/// RuntimeContext — mengatur seluruh resource host: CPU, memori, thread pool,
/// temp directory.
///
/// Compiler cukup meminta `runtime.thread_pool.spawn(...)` atau
/// `runtime.spawn(...)` tanpa tahu implementasi thread.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub thread_pool: Arc<ThreadPoolHandle>,
    pub temp_dir: PathBuf,
}

impl RuntimeContext {
    /// Runtime default: thread pool = jumlah core host.
    pub fn new() -> Self {
        let threads = num_cpus::get();
        RuntimeContext {
            cpu: CpuInfo::detect(),
            memory: MemoryInfo::read(),
            thread_pool: Arc::new(ThreadPoolHandle::new(threads)),
            temp_dir: std::env::temp_dir(),
        }
    }

    /// Inisialisasi runtime — jumlah thread mengikuti config (`jobs`).
    pub fn init(config: &ConfigContext) -> Result<Self, String> {
        let threads = config.max_threads();
        Ok(RuntimeContext {
            cpu: CpuInfo::detect(),
            memory: MemoryInfo::read(),
            thread_pool: Arc::new(ThreadPoolHandle::new(threads)),
            temp_dir: std::env::temp_dir(),
        })
    }

    pub fn parallelism(&self) -> usize {
        self.thread_pool.threads()
    }

    /// Spawn task fire-and-forget ke thread pool.
    pub fn spawn<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.thread_pool.spawn(f);
    }

    pub fn summary(&self) -> String {
        format!(
            "cpu={} physical/{} logical mem={}MiB avail temp={}",
            self.cpu.physical_cores,
            self.cpu.logical_cores,
            self.memory.available_mib(),
            self.temp_dir.display(),
        )
    }
}

impl Default for RuntimeContext {
    fn default() -> Self {
        RuntimeContext::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::config::MivonConfig;

    #[test]
    fn test_runtime_init_respects_config() {
        let mut cfg = MivonConfig::default();
        cfg.compiler.jobs = Some(3);
        let ctx = ConfigContext::new(cfg);
        let rt = RuntimeContext::init(&ctx).unwrap();
        assert_eq!(rt.parallelism(), 3);
        assert!(!rt.summary().is_empty());
    }

    #[test]
    fn test_runtime_default() {
        let rt = RuntimeContext::new();
        assert!(rt.parallelism() >= 1);
        assert!(rt.cpu.logical_cores >= 1);
    }

    #[test]
    fn test_runtime_spawn() {
        let rt = RuntimeContext::new();
        let (tx, rx) = std::sync::mpsc::channel();
        rt.spawn(move || {
            let _ = tx.send(true);
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
    }
}
