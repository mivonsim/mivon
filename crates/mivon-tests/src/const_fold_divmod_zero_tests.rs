//! Regresi div/mod-nol pada const-fold elaborasi (fuzz regress
//! bug_4dcdecaac736, kampanye parser, oracle O2-diag-location).
//!
//! Dulu: `P % Z0` dgn P/Z0 param konstanta (Z0=0) memicu Err hard di
//! `try_fold_const` → E9001 "modulo by zero in constant expression"
//! TANPA lokasi (String const_eval tak membawa span; konversi SimError
//! di atas kehilangan posisi) → elaborasi fatal.
//!
//! Semantik benar: fold = optimasi. §11.6.1 — div/mod dgn 0 → hasil X
//! di runtime (evaluator value.rs menangani r==0 → X). Biarkan
//! unfolded; elaborasi lanjut, sim menghasilkan X.

use mivon_api::{compile_str, simulate_signals};

const DIVMOD0_SRC: &str = r#"
module top;
  localparam int P = 5;
  localparam int Z0 = 0;
  reg [7:0] y1, y2;
  always_comb begin
    y1 = P % Z0;
    y2 = P / Z0;
  end
endmodule
"#;

/// Elaborasi Wajib sukses — dulu E9001 tanpa lokasi jadi fatal.
#[test]
fn divmod_zero_compiles_without_fatal() {
    assert!(
        compile_str(DIVMOD0_SRC).is_ok(),
        "div/mod-0 const-fold tidak boleh mematikan elaborasi"
    );
}

/// Runtime: `%0` dan `/0` → X penuh (§11.6.1).
#[test]
fn divmod_zero_runtime_yields_x() {
    let sigs = simulate_signals(DIVMOD0_SRC, 50).expect("sim harus jalan");
    for name in ["y1", "y2"] {
        let (_, v) = sigs
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("sinyal '{name}' harus ada"));
        assert!(
            v.bits.len() == 8 && v.bits.iter().all(|b| *b == mivon_ir::LogicVal::X),
            "{name} = tidak semua X (div/mod 0 → X): {v:?}"
        );
    }
}
