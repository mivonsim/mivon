//! Runner formal verification (BMC) untuk CLI `--formal`.
//!
//! Dipisah dari main.rs: menerima `IrDesign` hasil elaborasi, menjalankan
//! `FormalEngine`, mencetak hasil + laporan konektivitas (FORMAL-13), dan
//! mengembalikan Err bila ada counterexample/error (gate CI).

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;

/// Run formal verification (BMC) and print results.
/// Returns Err if any assertion fails (counterexample found — for CI/CD integration).
pub(crate) fn run_formal(
    ir_design: &mivon_ir::IrDesign,
    bound: u64,
    quiet: bool,
    induction: bool,
    connect_pairs: &[String],
) -> Result<(), SimError> {
    use mivon_api::formal::*;
    let formal_cfg = FormalConfig {
        bound,
        induction,
        ..Default::default()
    };
    let mut formal_engine = FormalEngine::new(formal_cfg);
    let results = formal_engine.check_assertions_bmc(ir_design);

    // FORMAL-13: connectivity check (statis, tanpa solver).
    let conn_results = if !connect_pairs.is_empty() {
        let pairs: Vec<(String, String)> = connect_pairs
            .iter()
            .filter_map(|s| {
                let (a, b) = s.split_once(',')?;
                Some((a.trim().to_string(), b.trim().to_string()))
            })
            .collect();
        Some(mivon_api::formal::connectivity::check_connectivity(
            ir_design, &pairs,
        ))
    } else {
        None
    };

    if !quiet {
        println!("\n── Formal Verification Results (BMC bound={}) ──", bound);
    }

    let has_fail = results
        .iter()
        .any(|(_, r)| matches!(r, FormalResult::Counterexample(_)));
    let mut has_error = results
        .iter()
        .any(|(_, r)| matches!(r, FormalResult::Error(_)));

    // FORMAL-13: laporan konektivitas.
    if let Some(conn) = &conn_results {
        if !quiet {
            println!("\n── Connectivity Check ──");
            for r in conn {
                if let Some(err) = &r.error {
                    println!("  ! ERROR: {} → {} — {}", r.src, r.dst, err);
                } else if r.connected {
                    let path = r.path.join(" → ");
                    match r.path_len {
                        Some(0) => println!("  ✓ CONNECTED (self): {}", r.src),
                        Some(n) => {
                            println!(
                                "  ✓ CONNECTED: {} → {} via {} hop{} ({})",
                                r.src,
                                r.dst,
                                n,
                                if n == 1 { "" } else { "s" },
                                path
                            )
                        }
                        None => println!("  ✗ NOT CONNECTED: {} → {}", r.src, r.dst),
                    }
                } else {
                    println!("  ✗ NOT CONNECTED: {} → {}", r.src, r.dst);
                }
            }
        }
        // Pasangan dengan error ikut dihitung sebagai kegagalan CI gate.
        if conn_results
            .as_ref()
            .is_some_and(|c| c.iter().any(|r| r.error.is_some()))
        {
            has_error = true;
        }
    }

    for (name, result) in &results {
        if quiet {
            continue;
        }
        match result {
            FormalResult::Pass => println!("  ✓ PASS: {}", name),
            FormalResult::Counterexample(d) => {
                println!("  ✗ FAIL: {} — counterexample at depth {}", name, d)
            }
            FormalResult::Unknown => println!("  ? UNKNOWN: {}", name),
            FormalResult::Error(e) => println!("  ! ERROR: {} — {}", name, e),
            FormalResult::InductiveProof(k) => {
                println!(
                    "  ✓ INDUCTIVE PROOF (k={}): {} — holds for ALL depths",
                    k, name
                )
            }
        }
    }

    if !quiet {
        if results.is_empty() {
            println!("  (no assertions found)");
        }
        println!(
            "── End of Formal Results ({}/{} passed) ──\n",
            results
                .iter()
                .filter(|(_, r)| matches!(r, FormalResult::Pass))
                .count(),
            results.len()
        );
    }

    if has_error {
        return Err(SimError::with_diag(
            DiagCode::InternalError,
            "formal verification encountered errors",
        ));
    }
    if has_fail {
        return Err(SimError::with_diag(
            DiagCode::AssertionFailed,
            "formal verification FAILED — counterexample(s) found",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_formal;

    /// Desain tanpa assertion → tidak ada counterexample → Ok.
    /// Regresi pemisahan modul: jalur BMC tetap berjalan dari IrDesign.
    #[test]
    fn formal_without_assertions_is_ok() {
        let ir = mivon_api::compile_str("module m; logic a; initial a = 1'b0; endmodule")
            .expect("compile");
        assert!(run_formal(&ir, 5, true, false, &[]).is_ok());
    }
}
