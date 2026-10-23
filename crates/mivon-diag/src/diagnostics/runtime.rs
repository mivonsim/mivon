/// Konteks simulasi saat runtime error terjadi.
///
/// Ini adalah informasi yang tidak dimiliki compiler biasa — spesifik untuk HDL simulation.
/// Memberi insinyur gambaran lengkap: kapan, di mana, dan dalam konteks apa error terjadi.
#[derive(Debug, Clone, Default)]
pub struct RuntimeContext {
    /// Simulation time (misal 125 ns)
    pub time: Option<String>,
    /// Delta cycle
    pub delta: Option<u64>,
    /// Worker thread
    pub thread: Option<String>,
    /// Module name
    pub module: Option<String>,
    /// Hierarchical instance path
    pub instance: Option<String>,
    /// RNG seed
    pub seed: Option<u64>,
    /// Process type (always_ff, always_comb, initial, dll)
    pub process: Option<String>,
}

impl RuntimeContext {
    pub fn new() -> Self {
        RuntimeContext::default()
    }

    pub fn with_time(mut self, time: impl Into<String>) -> Self {
        self.time = Some(time.into());
        self
    }

    pub fn with_delta(mut self, delta: u64) -> Self {
        self.delta = Some(delta);
        self
    }

    pub fn with_thread(mut self, thread: impl Into<String>) -> Self {
        self.thread = Some(thread.into());
        self
    }

    pub fn with_module(mut self, module: impl Into<String>) -> Self {
        self.module = Some(module.into());
        self
    }

    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn with_process(mut self, process: impl Into<String>) -> Self {
        self.process = Some(process.into());
        self
    }

    /// Format konteks simulasi sebagai string multi-baris.
    pub fn format(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(module) = &self.module {
            parts.push(format!("module : {}", module));
        }
        if let Some(instance) = &self.instance {
            parts.push(format!("instance: {}", instance));
        }
        if let Some(process) = &self.process {
            parts.push(format!("process: {}", process));
        }
        if let Some(time) = &self.time {
            parts.push(format!("time   : {}", time));
        }
        if let Some(delta) = self.delta {
            parts.push(format!("delta  : {}", delta));
        }
        if let Some(thread) = &self.thread {
            parts.push(format!("thread : {}", thread));
        }
        if let Some(seed) = self.seed {
            parts.push(format!("seed   : {}", seed));
        }
        if parts.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        out.push_str("Simulation Context\n\n");
        for part in &parts {
            out.push_str(&format!("  • {}\n", part));
        }
        out
    }
}
