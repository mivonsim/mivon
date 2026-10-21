//! Dispatch subcommand CLI → backend tool (tools.md).
//!
//! `main.rs` tetap entrypoint tipis: seluruh adaptor `dispatch_*` yang
//! menerjemahkan argumen `cli::*Args` menjadi `*Args` milik `mivon-api`
//! hidup di sini.
//!
//! 1 file = 1 tanggung jawab (granularitas: satu keluarga tool):
//! - `tools.rs`  — tool inti: inspect, lint, elab, sim, cov, wave, fmt, gen,
//!   prof, check, bench, synth, update.
//! - `manage.rs` — manajemen desain & kualitas: design_repo, project, sdc,
//!   equiv_check, regression, eco, cov_closure.
//! - `extra.rs`  — tool tambahan: batch, memcheck, tbgen, waiver, vault,
//!   ipxact.
//!
//! Adaptor per tool tetap fungsi terpisah (bukan satu `match` raksasa) agar
//! tiap tool bisa diubah/diuji tanpa menyentuh yang lain.

mod extra;
mod manage;
mod tools;

pub(crate) use extra::*;
pub(crate) use manage::*;
pub(crate) use tools::*;

use mivon_core::error::SimError;

/// Keluar proses dengan kode exit sesuai jenis error (`SimError::exit_code`),
/// mencetak diagnostik ke terminal bila ada.
pub(crate) fn exit_tool(result: Result<(), SimError>) -> ! {
    if let Err(e) = result {
        let mut emitter = mivon_core::diagnostics::TerminalEmitter::new();
        let diag = e.to_diagnostic();
        let _ = emitter.emit(&diag);
        std::process::exit(e.exit_code());
    }
    std::process::exit(0);
}
