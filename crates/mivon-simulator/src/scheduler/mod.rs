//! Scheduler — work-stealing task pool, dependency-aware scheduling.
//!
//! Phase 1 implementation. Menggunakan crossbeam-deque untuk work-stealing.
//!
//! Migrasi monorepo (crate 6, mivon-compiler): cluster kompilasi
//! (dag/incremental/work_stealing/priority) pindah ke crates/mivon-compiler
//! karena dipakai pipeline kompilasi. Cluster simulasi (sim_dag/clock_domain/
//! cdc) tetap di sini (crate mivon-simulator) karena bergantung pada
//! simulator::parallel. Re-export di bawah menjaga `crate::scheduler::Task`
//! dkk tetap valid bagi konsumen lama.

pub mod cdc;
pub mod clock_domain;
pub mod reset_domain;
pub mod sim_dag;

pub use clock_domain::{ClockDomain, ClockDomainAnalysis, ClockEdgeType};
pub use sim_dag::{
    evaluate_layer_parallel, is_process_parallelizable, layer_to_string, SimulationDag,
};

// Cluster kompilasi — di-reexport dari mivon-compiler agar path lama
// (`crate::scheduler::DependencyGraph`, `crate::scheduler::Task`, ...) tetap
// valid bagi konsumen yang belum dimigrasi.
pub use mivon_compiler::scheduler::priority::task_priority;
pub use mivon_compiler::scheduler::{
    DependencyGraph, IncrementalTracker, NodeId, Priority, PriorityQueue, Task,
};
