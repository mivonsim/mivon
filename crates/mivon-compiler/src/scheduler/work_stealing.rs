//! Tipe `Task` untuk cluster scheduler kompilasi.
//!
//! Catatan Par-01 (audit.txt): work-stealing paralel pada produk disediakan
//! rayon — `files.par_iter()` di `CompileSession` (preprocess/lex/parse) dan
//! evaluasi layer DAG di `mivon-simulator/src/scheduler/sim_dag.rs`.
//! Executor `Scheduler` kustom (queue + worker loop dengan handler no-op)
//! sebelumnya dead code dan telah dihapus; tipe `Task` dipertahankan karena
//! masih hidup sebagai payload `DependencyGraph` (compile_session) dan
//! pemetaan prioritas (`priority::task_priority`).

/// Task types untuk scheduler.
#[derive(Debug, Clone)]
pub enum Task {
    // File-level
    PreprocessFile(String),
    TokenizeFile(String),
    ParseFile(String),
    // Module-level
    TypeCheck(String),
    ElaborateModule(String),
    ResolvePackage(String),
    // Post-processing
    FlattenHierarchy,
    LowerToSimIr,
    // System
    DiagnosticFlush,
    CacheEviction,
    // Custom
    Custom(String),
}

#[cfg(test)]
mod tests {
    use super::Task;
    use crate::scheduler::priority::{task_priority, Priority};

    /// Regresi Par-01: `Task` tetap terpakai sebagai API publik tanpa
    /// executor `Scheduler` — payload DependencyGraph + pemetaan prioritas.
    #[test]
    fn task_survives_without_scheduler_executor() {
        let t = Task::ParseFile("design.sv".into());
        let cloned = t.clone();
        assert!(format!("{cloned:?}").contains("ParseFile"));
        assert_eq!(
            task_priority(&Task::PreprocessFile("a.sv".into())),
            Priority::Critical
        );
        assert_eq!(
            task_priority(&Task::ParseFile("a.sv".into())),
            Priority::High
        );
        assert_eq!(task_priority(&Task::CacheEviction), Priority::Low);
    }
}
