//! Mivon — Test Suite Terpadu.
//!
//! Seluruh test suite (`src/tests/`, `src/edge_tests.rs`, `src/debug_lexer.rs`)
//! pindah ke crate ini pada migrasi monorepo. `crate::simulator`,
//! `crate::compile_str`, `crate::compare_asts`, dll. tetap valid karena
//! crate ini mere-export seluruh API `mivon_api` (dan `simulator` milik
//! mivon-simulator) ke akar crate.
//!
//! Menjalankan: `cargo test -p mivon-tests` (atau `cargo test --workspace`).

pub use mivon_api::*;

// MDME module - available for both tests and example binary

// Private use mirror dari lib.rs lama — dibawa ke submodule test via glob
// `use super::*` (tests/mod.rs, edge_tests.rs) sehingga `fs::…`, `Lexer`,
// `Parser`, `Preprocessor` tetap valid tanpa edit di 15K LOC test.
// (cfg(test): hanya terpakai saat crate ini di-test, hindari unused-import
// warning pada build lib biasa.)
#[cfg(test)]
use mivon_parser::lexer::Lexer;
#[cfg(test)]
use mivon_parser::preprocessor::Preprocessor;
#[cfg(test)]
use mivon_parser::Parser;
#[cfg(test)]
use std::fs;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod edge_tests;

#[cfg(test)]
mod debug_lexer;

#[cfg(test)]
mod synth_phase4;

#[cfg(test)]
mod synth_phase5;

#[cfg(test)]
mod parallel_emi_tests;

/// Regresi implicit sampling covergroup (§19.8) — gap sesi fuzz 2026-09-28.
mod covergroup_tests;

/// Regresi fallback serial DAG-parallel utk ekspresi tak didukung
/// (evaluator parallel jatuh ke X senyap — fuzzer sim bug_0508).
mod dag_serial_fallback_tests;

/// Regresi localparam lokal body generate-for (scope rename + fold) —
/// fuzzer 2026-09-29, golden iverilog.
mod generate_localparam_tests;

// Guided structure-aware fuzzer (metode test fuzzing "tidak buta"):
// grammar-aware generation + coverage feedback + differential oracle.
