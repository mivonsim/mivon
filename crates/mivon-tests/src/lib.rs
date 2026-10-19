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
#[cfg(test)]
mod covergroup_tests;

/// Regresi fallback serial DAG-parallel utk ekspresi tak didukung
/// (evaluator parallel jatuh ke X senyap — fuzzer sim bug_0508).
#[cfg(test)]
mod dag_serial_fallback_tests;

/// Regresi localparam lokal body generate-for (scope rename + fold) —
/// fuzzer 2026-09-29, golden iverilog.
#[cfg(test)]
mod generate_localparam_tests;

/// Regresi implicit port connection `.*` — dulu di-skip total (port tak
/// terhubung, output X tanpa error) — fuzzer 2026-09-29.
#[cfg(test)]
mod dotstar_tests;

/// Regresi system task/function tanpa argumen kurung (`$display;`,
/// `$time;`) — fuzzer MV roundtrip, golden `iverilog -g2012`.
#[cfg(test)]
mod syscall_noargs_tests;

/// Regresi const-fold X/Z pada concat + lebar hasil `**` — sweep
/// differential vs `iverilog -g2012`.
#[cfg(test)]
mod const_fold_xz_power_tests;

/// `timescale` per-module (LRM §19.8) — `#N` harus pakai satuan module itu,
/// bukan satuan global design.
#[cfg(test)]
mod timescale_tests;

/// `timescale` per-module di jalur CompileSession (CLI) + multi-file.
#[cfg(test)]
mod timescale_session_tests;

/// Regresi `$realtime` sebagai real — sweep differential vs `iverilog -g2012`
/// (marker `%f` di-cetak dari bit-pattern f64 yang salah dibaca sebagai integer).
#[cfg(test)]
mod realtime_tests;

/// Regresi parser untuk konstruk yang gagal saat compile CVA6
/// (`mivon --filelist cva6/cva6.f`): dimensi array range `[msb:lsb]`,
/// keyword `none`/`some` sebagai identifier, dan parameter override bertipe
/// `#(.P(v))` pada deklarasi variabel.
#[cfg(test)]
mod cva6_parse_regression_tests;

/// Regresi elaborasi CVA6: propagasi field struct-parameter melalui override
/// instance (`#(.CVA6Cfg(CVA6Cfg))`) — field `param.field` harus ter-flatten ke
/// `param_vals` submodule agar range-select bergantung parameter ter-resolve
/// (bukan "dynamic range select not supported").
#[cfg(test)]
mod cva6_struct_param_regression_tests;

// REGRESI Tahap 1: nested bit/part-select pada lvalue.
#[cfg(test)]
mod nested_select_tests;
#[cfg(test)]
mod unit_scope_tests;
#[cfg(test)]
mod named_sva_tests;
#[cfg(test)]
mod hier_lvalue_tests;
#[cfg(test)]
mod vif_write_tests;
#[cfg(test)]
mod interface_port_tests;
#[cfg(test)]
mod synth_sir_tests;

// Guided structure-aware fuzzer (metode test fuzzing "tidak buta"):
// grammar-aware generation + coverage feedback + differential oracle.
