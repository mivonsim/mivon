//! Cache context — semua persistent storage (node_cache).
//!
//! Compiler TIDAK pernah membuka file cache secara langsung; ia meminta
//! melalui DatabaseContext. Cache tidak boleh mengetahui Parser.

#[allow(clippy::module_inception)]
mod database;
mod diagnostics_db;
mod graph_db;
mod metadata_db;
mod node_cache;
mod symbol_db;

pub use database::DatabaseContext;
pub use diagnostics_db::{diag_count, total_diag_count};
pub use graph_db::{def_of, deps_of, file_dep_count};
pub use metadata_db::{file_count, file_meta, recompiled_count};
pub use node_cache::{database_root_for, default_database_root, open_database, project_id_for};
pub use symbol_db::{locate_symbol, symbol_count, symbol_names};
