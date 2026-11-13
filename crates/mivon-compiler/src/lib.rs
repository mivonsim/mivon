//! mivon-compiler — pipeline kompilasi: frontend (discovery/lexer/parse),
//! cache, node_cache (filesystem-native incremental cache), HIR, MIR,
//! profiling, dan scheduler task cluster (dag/incremental/work_stealing/
//! priority).
//!
//! Crate 6 dalam migrasi monorepo (src/ → crates/). Bergantung pada
//! mivon-core, mivon-ast, mivon-ir, mivon-parser, mivon-elaboration.

// Allow large Result Err variant for SimError (intentional — Diagnostic contains spans/files)
#![allow(clippy::result_large_err, clippy::type_complexity)]

pub mod cache;
pub mod frontend;
pub mod hir;
// node_cache menggantikan MICD: cache filesystem-native, content-addressed.
pub use mivon_node_cache as node_cache;
pub mod mir;
pub mod profiling;
pub mod scheduler;

// Re-export API level atas (seperti mod.rs masing-masing).
pub use frontend::{CompileSession, FileDiscovery, ModuleIndex, PackageResolver};
pub use mivon_node_cache::{CacheStats, PreprocEntry, ProjectCache};
pub use scheduler::{DependencyGraph, IncrementalTracker, NodeId, Priority, PriorityQueue, Task};
