//! Adaptor dispatch untuk `mtcl` — Tcl scripting interface (FEAT-06).
//!
//! Hanya menerjemahkan argumen CLI menjadi pemanggilan interpreter
//! `mivon_api::tools::tcl`, mencetak buffer `puts`, lalu keluar dengan kode
//! `exit` skrip bila ada.

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;

use super::exit_tool;

pub(crate) fn dispatch_tcl(a: &crate::cli::MtclArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        let script = std::fs::read_to_string(&a.script)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", a.script, e)))?;
        let mut interp = mivon_api::tools::tcl::Interp::new();
        interp.set_argv(&a.args);
        interp.run(&script).map_err(|e| {
            SimError::with_diag(
                DiagCode::InvalidSyntax,
                format!("{}: {}", a.script, e.message()),
            )
        })?;
        for line in interp.output() {
            println!("{}", line);
        }
        if let Some(code) = interp.exit_code() {
            std::process::exit(code);
        }
        Ok(())
    })();
    exit_tool(result);
}
