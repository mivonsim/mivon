//! Regresi IMPLICIT SAMPLING covergroup (IEEE 1800-2017 §19.8).
//!
//! `covergroup cg @ (posedge clk)` wajib di-sample OTOMATIS tiap edge —
//! tanpa `sample()` eksplisit. Dulu clocking event dibuang parser dan tak
//! pernah turun ke IR → covergroup `@event` tanpa `sample()` = 0 samples
//! SENYAP (temuan seed 44, sesi fuzz 2026-09-28).

const CG_IMPLICIT_SRC: &str = r#"
module cg_implicit;
  logic clk = 0;
  logic x = 0;
  covergroup cg @ (posedge clk);
    cp: coverpoint x {
      bins a = {0};
      bins b = {1};
    }
  endgroup
  cg g = new; // decl-init — TANPA sample() eksplisit di mana pun
  initial begin
    forever #5 clk = ~clk;
  end
  initial begin
    #12 x = 1;
    #20 $finish;
  end
endmodule
"#;

/// Covergroup `@posedge clk` tanpa `sample()` eksplisit harus ter-sample
/// tiap edge (coverage > 0), bukan 0/1 senyap.
#[test]
fn covergroup_implicit_sampling_on_event() {
    let design = mivon_api::compile_str(CG_IMPLICIT_SRC).expect("covergroup source harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 100);
    engine.run().expect("sim harus jalan");
    let stats = engine.coverage_stats();
    let points = stats.get("covergroup_points").copied().unwrap_or(0.0);
    let covered = stats.get("covergroup_covered").copied().unwrap_or(0.0);
    assert!(
        points >= 1.0,
        "covergroup harus ter-elaborate (minimal 1 poin): {stats:?}"
    );
    assert!(
        covered >= 1.0,
        "implicit sampling @posedge harus menghasilkan hit tanpa sample() eksplisit \
         (dulu 0 senyap — gap sesi 2026-09-28): {stats:?}"
    );
    // Minimal 3 posedge selama 100ns (t=5,15,25...) — sampel ≥ 2.
    let cg_pct = stats.get("covergroup_percent").copied().unwrap_or(0.0);
    assert!(cg_pct > 0.0, "covergroup percent harus > 0: {cg_pct}");
}
