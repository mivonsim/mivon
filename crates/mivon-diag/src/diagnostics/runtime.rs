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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_context_default() {
        let ctx = RuntimeContext::default();
        assert_eq!(ctx.time, None);
        assert_eq!(ctx.delta, None);
        assert_eq!(ctx.thread, None);
        assert_eq!(ctx.module, None);
        assert_eq!(ctx.instance, None);
        assert_eq!(ctx.seed, None);
        assert_eq!(ctx.process, None);
    }

    #[test]
    fn test_runtime_context_all_fields() {
        let ctx = RuntimeContext {
            time: Some("100ns".to_string()),
            delta: Some(5),
            thread: Some("sim_thread".to_string()),
            module: Some("top".to_string()),
            instance: Some("uut".to_string()),
            seed: Some(42),
            process: Some("always_ff".to_string()),
        };
        assert_eq!(ctx.time, Some("100ns".to_string()));
        assert_eq!(ctx.delta, Some(5));
        assert_eq!(ctx.thread, Some("sim_thread".to_string()));
        assert_eq!(ctx.module, Some("top".to_string()));
        assert_eq!(ctx.instance, Some("uut".to_string()));
        assert_eq!(ctx.seed, Some(42));
        assert_eq!(ctx.process, Some("always_ff".to_string()));
    }

    #[test]
    fn test_runtime_context_builder() {
        let ctx = RuntimeContext::new()
            .with_time("200ns".to_string())
            .with_delta(3)
            .with_thread("thread_1".to_string())
            .with_module("my_module".to_string())
            .with_instance("inst_1".to_string())
            .with_seed(7)
            .with_process("always_comb".to_string());

        assert_eq!(ctx.time, Some("200ns".to_string()));
        assert_eq!(ctx.delta, Some(3));
        assert_eq!(ctx.thread, Some("thread_1".to_string()));
        assert_eq!(ctx.module, Some("my_module".to_string()));
        assert_eq!(ctx.instance, Some("inst_1".to_string()));
        assert_eq!(ctx.seed, Some(7));
        assert_eq!(ctx.process, Some("always_comb".to_string()));
    }

    #[test]
    fn test_runtime_context_format() {
        let ctx = RuntimeContext {
            time: Some("100ns".to_string()),
            delta: Some(5),
            thread: Some("sim_thread".to_string()),
            module: Some("top".to_string()),
            instance: Some("uut".to_string()),
            seed: Some(42),
            process: Some("always_ff".to_string()),
        };
        let formatted = ctx.format();
        assert!(formatted.contains("Simulation Context"));
        assert!(formatted.contains("module : top"));
        assert!(formatted.contains("instance: uut"));
        assert!(formatted.contains("process: always_ff"));
        assert!(formatted.contains("time   : 100ns"));
        assert!(formatted.contains("delta  : 5"));
        assert!(formatted.contains("thread : sim_thread"));
        assert!(formatted.contains("seed   : 42"));
    }

    #[test]
    fn test_runtime_context_format_empty() {
        let ctx = RuntimeContext::default();
        let formatted = ctx.format();
        assert!(formatted.is_empty(), "empty context should return empty string, got: {:?}", formatted);
    }
}
