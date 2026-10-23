//! String interning — semua identifier jadi `Symbol` (Copy, u32).
//!
//! Strategi:
//! - Global concurrent string table (DashMap-backed)
//! - Thread-local cache untuk mengurangi contention
//! - Pre-populated dengan keywords umum

mod span;
mod string_intern;
mod table;

pub use span::*;
pub use string_intern::*;
pub use table::*;
