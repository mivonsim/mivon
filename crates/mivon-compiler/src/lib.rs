//! mivon-compiler — pipeline kompilasi: frontend (discovery/lexer/parse),
//! cache, MICD (incremental database), HIR, MIR, profiling, dan scheduler
//! task cluster (dag/incremental/work_stealing/priority).
//!
//! Crate 6 dalam migrasi monorepo (src/ → crates/). Bergantung pada
//! mivon-core, mivon-ast, mivon-ir, mivon-parser, mivon-elaboration.

// Allow large Result Err variant for SimError (intentional — Diagnostic contains spans/files)
#![allow(clippy::result_large_err, clippy::type_complexity)]

pub mod cache;
pub mod frontend;
pub mod hir;
pub mod micd;
pub mod mir;
pub mod profiling;
pub mod scheduler;

// Re-export API level atas (seperti mod.rs masing-masing).
pub use frontend::{CompileSession, FileDiscovery, ModuleIndex, PackageResolver};
pub use micd::cache::{CacheCategory, CacheLayer, CacheLayerStats};
pub use micd::MicdDatabase;
pub use scheduler::{DependencyGraph, IncrementalTracker, NodeId, Priority, PriorityQueue, Task};
