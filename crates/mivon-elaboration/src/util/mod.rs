//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: mod.rs ini adalah entry point utama untuk modul util.
//! Semua fungsi di-re-export dari submodule masing-masing agar
//! use super::util::*; tetap berfungsi (backward compatible).
//!
//! Setiap submodule memiliki EXACTLY 1 (satu) tanggung jawab (SRP):
//!   - type_util.rs       — type checking & reset detection
//!   - param_util.rs      — parameter resolution
//!   - generate.rs        — generate block expansion & genvar substitution
//!   - loop_unroll.rs     — loop unrolling & variable substitution
//!   - signal_analysis.rs — signal reading analysis & sensitivity
//!   - const_fold.rs      — constant folding & value-to-LogicVec conversion
//!   - operator.rs        — operator mapping & gate expression building
//!   - width.rs           — expression width computation
//!   - type_subst.rs      — type parameter substitution
//!   - opt_stats.rs       — statistik optimasi (const fold, loop unroll, eval)
//!   - range_index.rs     — konversi indeks deklarasi packed vector → posisi bit
//!
//! ──────────────────────────────────────────────────────────────────────────────

pub mod type_util;
pub use type_util::*;

pub mod param_util;
pub use param_util::*;

pub mod generate;
pub use generate::*;

pub mod loop_unroll;
pub use loop_unroll::*;

pub mod signal_analysis;
pub use signal_analysis::*;

pub mod const_fold;
pub use const_fold::*;

pub mod operator;
pub use operator::*;

pub mod width;
pub use width::*;

pub mod type_subst;
pub use type_subst::*;

pub mod opt_stats;
pub use opt_stats::*;

pub mod range_index;
pub use range_index::*;

pub mod hier_sensitivity;
pub use hier_sensitivity::*;
