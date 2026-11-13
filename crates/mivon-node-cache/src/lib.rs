//! node_cache — cache komputasi filesystem-native, content-addressed,
//! incremental, dan transaksional. Pengganti penuh MICD (tanpa database).
//!
//! ```text
//! node_cache/
//! ├── objects/ab/cd/<content-hash>   artefak immutable (BLAKE3)
//! ├── nodes/01/02/<node-id>          manifest node (biner versioned)
//! ├── graphs/<graph-id>             graph snapshot immutable
//! ├── refs/<project-id>             graph terakhir project
//! ├── transactions/<txn-id>         journal commit
//! ├── tmp/                          artefak sementara
//! ├── locks/<node-id>.lock          lock per-node
//! └── FORMAT                        versi format + algoritma hash
//! ```
//!
//! Empat komponen inti: Node Resolver, Artifact Store, Dependency Graph,
//! Transaction & Recovery. GC dan scheduler internal, bukan subsystem
//! mandiri. Lihat `node_cache.md` untuk desain lengkap.

#![deny(unsafe_code)]

pub mod ast;
pub mod cache_layer;
pub mod dependency;
pub mod entries;
pub mod filesystem;
pub mod format;
pub mod gc;
pub mod graph;
pub mod identity;
pub mod manifest;
pub mod object;
pub mod pipeline;
pub mod precompiled;
pub mod project;
pub mod recovery;
pub mod resolver;
pub mod scheduler;
pub mod transaction;

pub use ast::{
    deserialize_design, deserialize_ir, serialize_design, serialize_ir, AST_FORMAT_VERSION,
    IR_FORMAT_VERSION,
};
pub use dependency::{checksum64, file_checksum, DependencySet, FileDep};
pub use entries::{
    flags_hash, now_ns, path_hash, peak_rss_kb, BuildProfile, CacheStats, CheckResult, FileGraph,
    FileMeta, FileStatus, PreprocEntry, StatsDb, SymbolIndex, VerifyCheckKind, VerifyResult,
    FILE_SCHEMA, INDEX_SCHEMA, PREPROC_SCHEMA, TOOLCHAIN, VERIFY_SCHEMA,
};
pub use cache_layer::{
    CacheCategory, CacheLayer, CacheLayerStats, CategoryStats, CategoryStore,
};
pub use filesystem::{Layout, NodeLock, STORAGE_VERSION};
pub use gc::{collect as collect_garbage, GcStats};
pub use graph::{GraphEntry, GraphSnapshot, GraphStore, RefRecord};
pub use identity::{ContentId, NodeFingerprint, NodeIdentity, HASH_ALGO};
pub use manifest::{NodeManifest, OutputRef};
pub use object::ObjectStore;
pub use precompiled::{PrecompiledDb, PrecompiledModule, PrecompiledStats, PortInfo};
pub use project::ProjectCache;
pub use recovery::{recover, RecoveryReport};
pub use resolver::{NodeResolver, ResolvedNode};
pub use scheduler::{run_parallel, worker_count};
pub use transaction::{Journal, Transaction};

/// Re-export tipe diagnostik yang dipakai payload index (satu sumber tipe:
/// `mivon-diag` via `mivon-core`).
pub use mivon_core::diagnostics::{DiagEntry, FileDiags};

/// Kompatibilitas jalur lama: `node_cache::verify::<tipe>`.
pub mod verify {
    pub use crate::entries::{now_ns, CheckResult, VerifyCheckKind, VerifyResult};
}

/// Kompatibilitas jalur lama: `node_cache::cache::<kategori>` dan
/// `node_cache::cache::pipeline::<Payload>` tempat tool membaca artefak.
pub mod cache {
    pub use crate::cache_layer::*;

    pub mod category {
        pub use crate::cache_layer::CacheCategory;
    }

    pub mod pipeline {
        pub use crate::pipeline::*;
    }
}

/// Re-export tipe span diagnostik untuk pemanggil yang membangun entry diag.
pub mod diag {
    pub use mivon_core::diagnostics::TextSpan as Span;
}
