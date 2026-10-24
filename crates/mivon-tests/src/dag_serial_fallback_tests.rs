//! Regresi fallback SERIAL jalur DAG-parallel utk ekspresi tak didukung.
//!
//! Evaluator parallel (`evaluate_expr_simple`) dulu punya fallback senyap
//! `_ => Ok(LogicVec::new(32))` = 32-bit X utk variant tanpa arm
//! (FuncCall/DpiCall/SysFunc/...) → `assign mem = {pkg::f(), ...}` hasil
//! default/`-P` = 0 (DPI stub return 0) tapi `--parallel` = X →
//! differential antar engine (fuzzer sim bug_0508). Kini evaluator
//! parallel mengembalikan Err → core.rs men-fallback layer ke jalur serial
//! (jalur referensi) — hasil wajib identik.

use mivon_api::compile_str;

/// Persis repro fuzzer bug_0508: package function ABEN → DPI stub
/// returns 0 di jalur serial; tanpa fallback, DAG menghasilkan X.
const DAG_DPI_SRC: &str = r#"
module dpi_assign (
    input  logic         clk_i,
    input  logic         req_i,
    input  logic [31:0]  addr_i,
    output logic [31:0]  rdata_o
);
    localparam int RomSize = 2;
    logic [RomSize-1:0][31:0] mem;
    assign mem = {
        absent_pkg::jalr(5'h0, 5'h1, 32'h1c00_0080),
        absent_pkg::lui(5'h1, 32'h1c00_0080)
    };
    logic [$clog2(RomSize)-1:0] addr_q;
    assign rdata_o = (addr_q < RomSize) ? mem[addr_q] : '0;
    always_ff @(posedge clk_i) begin
        if (req_i) begin
            addr_q <= addr_i[$clog2(RomSize)-1+3:2];
        end
    end
endmodule
"#;

/// Jalankan design dgn flag DAG, kembalikan nilai `rdata_o` sbg string bit
/// MSB-first (pola sama parallel_emi_tests).
fn run_rdata(use_dag: bool) -> String {
    let design = compile_str(DAG_DPI_SRC).expect("design harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 50);
    engine.use_dag_parallel = use_dag;
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == "rdata_o")
        .expect("rdata_o harus ada");
    engine
        .state
        .read_signal(idx)
        .bits
        .iter()
        .rev()
        .map(|b| match b {
            mivon_ir::LogicVal::Zero => '0',
            mivon_ir::LogicVal::One => '1',
            mivon_ir::LogicVal::X => 'x',
            mivon_ir::LogicVal::Z => 'z',
        })
        .collect()
}

/// Jalur `--parallel` wajib identik dgn default utk assign berisi
/// function-call package (ekspresi tanpa arm di evaluator parallel).
#[test]
fn dag_parallel_serial_fallback_matches_default() {
    let default_v = run_rdata(false);
    let dag_v = run_rdata(true);
    assert_eq!(
        default_v, dag_v,
        "DAG-parallel beda dgn default (fallback serial tidak bekerja): \
         default={default_v} dag={dag_v}"
    );
    // DPI stub returns 0 → rdata_o ter-drive 0 (bukan X senyap).
    assert!(
        !dag_v.contains('x'),
        "rdata_o mengandung X di jalur DAG — evaluator parallel fallback ke X \
         senyap lagi: {dag_v}"
    );
}

/// Baca nilai sinyal flat (post-flatten) sbg string bit MSB→LSB utk
/// design dari source `src` dgn flag DAG on/off.
fn run_signal_bits(src: &str, name: &str, use_dag: bool) -> String {
    let design = compile_str(src).expect("design harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 100);
    engine.use_dag_parallel = use_dag;
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("sinyal '{name}' harus ada di design flat"));
    engine
        .state
        .read_signal(idx)
        .bits
        .iter()
        .rev()
        .map(|b| match b {
            mivon_ir::LogicVal::Zero => '0',
            mivon_ir::LogicVal::One => '1',
            mivon_ir::LogicVal::X => 'x',
            mivon_ir::LogicVal::Z => 'z',
        })
        .collect()
}

/// MINIMAL REPRO (fuzz bug_0cb12a3dc00b / bug_63d353b8c78f, kampanye
/// sim 2026-10-19): submodule pasif (`assign out = m`, m masih X,
/// tanpa stimulus) + 2 part-select assign ke `q` di parent. Semantik
/// LRM: out = m(X) → dout X → kedua slice q = X; bit58 TIDAK ter-drive
/// → tetap Z. Fix dag (commit cbf7608) = nilai PINNED persis (bukan
/// sekadar identik): default = dag = "z" (bit58) + "x"×58. Kalau remake
/// silent shift kebalik arah (mis. patch drop → all-z), test ini tetap
/// kena.
const PASSIVE_CHAIN_SRC: &str = r#"
module mem_sub(out);
  output [28:0] out;
  reg [28:0] m;
  assign out = m;
endmodule

module top;
  wire [28:0] dout_lo;
  wire [28:0] dout_hi;
  wire [58:0] q;
  mem_sub s_lo(.out(dout_lo));
  mem_sub s_hi(.out(dout_hi));
  assign q[57:29] = dout_hi;
  assign q[28:0]  = dout_lo;
endmodule
"#;

#[test]
fn dag_vs_default_passive_slice_chain_identical() {
    let def = run_signal_bits(PASSIVE_CHAIN_SRC, "dout_lo", false);
    let dag = run_signal_bits(PASSIVE_CHAIN_SRC, "dout_lo", true);
    assert_eq!(
        def, dag,
        "dout_lo beda antar jalur: default={def} dag={dag}"
    );
    // PINNED — bukan sekadar identik: bit58 undriven z, 58 bit cover q = x.
    let expect = format!("z{}", "x".repeat(58));
    assert_eq!(def, "x".repeat(29), "dout_lo harus X semua (reg masih X)");
    let qd = run_signal_bits(PASSIVE_CHAIN_SRC, "q", false);
    let qg = run_signal_bits(PASSIVE_CHAIN_SRC, "q", true);
    assert_eq!(qd, expect, "q default beda pin: {qd}");
    assert_eq!(qg, expect, "q dag beda pin: {qg}");
}

/// Par-03 tahap 2 (differential snapshot): transisi dalam SATU batch DAG.
/// initial #1 menulis d (X -> A5) di t=1; e dan f KEDUANYA ter-trigger oleh
/// d (f membaca e DAN d) sehingga satu batch berisi dua layer: e=~d
/// (layer0) lalu f=e^d (layer1, RAW pada e). Layer1 wajib melihat e hasil
/// layer0 lewat snapshot yang di-refresh differential — bila basi (e=X),
/// f=X permanen. Hasil wajib identik default DAN ter-pinned.
const CHAIN_SRC: &str = r#"
module chain_top;
  logic [7:0] d;
  logic [7:0] e;
  logic [7:0] f;
  initial begin
    #1 d = 8'hA5;
  end
  assign e = ~d;
  assign f = e ^ d;
endmodule
"#;

#[test]
fn dag_differential_snapshot_cross_layer_chain() {
    let ed = run_signal_bits(CHAIN_SRC, "e", false);
    let eg = run_signal_bits(CHAIN_SRC, "e", true);
    assert_eq!(eg, ed, "e dag harus identik default");
    assert_eq!(eg, "01011010", "e = ~A5 = 5A ter-pinned");
    let fd = run_signal_bits(CHAIN_SRC, "f", false);
    let fg = run_signal_bits(CHAIN_SRC, "f", true);
    assert_eq!(fg, fd, "f dag harus identik default");
    assert_eq!(fg, "11111111", "f = 5A ^ A5 = FF ter-pinned");
}

/// Par-02 tahap 1 (batch-parallel sequential): 4 always_ff independen pada
/// clock sama -> dalam satu delta posedge keempatnya ter-trigger dan (mode
/// DAG) dieval paralel via snapshot pre-edge + writes sebagai NBA pending.
/// Hasil akhir wajib identik jalur default/serial.
const FF_BATCH_SRC: &str = r#"
module ff_batch_top;
  logic clk;
  logic [7:0] a, b, c, d;
  logic [7:0] x, y, z, w;
  initial begin
    clk = 1'b0;
    a = 8'h10; b = 8'h20; c = 8'h30; d = 8'h40;
    #12;
    a = 8'h50; b = 8'h60; c = 8'h70; d = 8'h80;
    #10;
  end
  always #5 clk = ~clk;
  always_ff @(posedge clk) x <= a + 8'd1;
  always_ff @(posedge clk) y <= b + 8'd2;
  always_ff @(posedge clk) z <= c + 8'd3;
  always_ff @(posedge clk) w <= d + 8'd4;
endmodule
"#;

#[test]
fn dag_sequential_ff_batch_parallel_diff() {
    // Pinned: posedge t=15 dst memakai nilai a..d update t=12.
    let cases = [
        ("x", "01010001"), // 0x50 + 1 = 0x51
        ("y", "01100010"), // 0x60 + 2 = 0x62
        ("z", "01110011"), // 0x70 + 3 = 0x73
        ("w", "10000100"), // 0x80 + 4 = 0x84
    ];
    for (name, expect) in cases {
        let serial = run_signal_bits(FF_BATCH_SRC, name, false);
        let dag = run_signal_bits(FF_BATCH_SRC, name, true);
        assert_eq!(dag, serial, "{name}: dag harus identik serial");
        assert_eq!(dag, expect, "{name}: nilai akhir ter-pinned");
    }
}

/// Par-02 review fix A (intra-body RAW): `x <= x+1; y <= x` — evaluator
/// paralel memperlakukan NBA sebagai blocking intra-body (overlay), serial
/// tidak. Guard A menolak body ini dari batch → kedua jalur serial-equivalent.
/// y wajib lag satu (nilai x PRA-kenaikan).
const FF_RAW_SRC: &str = r#"
module ff_raw_top;
  logic clk;
  logic [7:0] x, y, w;
  logic [7:0] a;
  initial begin clk = 0; x = 0; a = 5; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) begin x <= x + 8'd1; y <= x; end
  always_ff @(posedge clk) w <= a;
endmodule
"#;

#[test]
fn dag_ff_intra_body_raw_diff() {
    let xd = run_signal_bits(FF_RAW_SRC, "x", false);
    let xg = run_signal_bits(FF_RAW_SRC, "x", true);
    assert_eq!(xg, xd, "x dag harus identik serial (guard A)");
    assert_eq!(xg, "00001010", "x = 10 posedge ter-pinned");
    let yd = run_signal_bits(FF_RAW_SRC, "y", false);
    let yg = run_signal_bits(FF_RAW_SRC, "y", true);
    assert_eq!(yg, yd, "y dag harus identik serial");
    assert_eq!(yg, "00001001", "y = x pra-kenaikan (9) ter-pinned");
}

/// Par-02 review fix B (serial-inline writes vs batch reads): kandidat
/// batch (x) membaca b yang ditulis kandidat serial inline (b = ~b).
/// Tanpa guard B, snapshot post-loop membuat x membaca b PASCA-tulis —
/// divergen dari serial. Guard B membatalkan keanggotaan x → serial.
const FF_CROSS_SRC: &str = r#"
module ff_cross_top;
  logic clk;
  logic [3:0] b;
  logic [3:0] x, w;
  initial begin clk = 0; b = 4'd3; #11; b = 4'd7; #20; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) x <= b + 4'd1;
  always_ff @(posedge clk) begin b = ~b; end
  always_ff @(posedge clk) w <= 4'd1;
endmodule
"#;

#[test]
fn dag_ff_cross_serial_write_diff() {
    let xd = run_signal_bits(FF_CROSS_SRC, "x", false);
    let xg = run_signal_bits(FF_CROSS_SRC, "x", true);
    assert_eq!(xg, xd, "x dag harus identik serial (guard B)");
    let wd = run_signal_bits(FF_CROSS_SRC, "w", false);
    let wg = run_signal_bits(FF_CROSS_SRC, "w", true);
    assert_eq!(wg, wd, "w dag harus identik serial");
    assert_eq!(wg, "0001", "w = 1 ter-pinned");
}

/// Par-02 review fix C (WAW lintas grup): p0 q<=b, p1 (b<=c; x<=e),
/// p2 x<=f. Greedy: grup1=[p0,p2], grup2=[p1]. Serial pid-order: x terakhir
/// ditulis p2 (f). Push ter-tag terurut pid → x=f (4), bukan x=e (3).
const FF_WAW_SRC: &str = r#"
module ff_waw_top;
  logic clk;
  logic [3:0] b, c, e, f;
  logic [3:0] q, x;
  initial begin clk = 0; b = 1; c = 2; e = 3; f = 4; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) q <= b;
  always_ff @(posedge clk) begin b <= c; x <= e; end
  always_ff @(posedge clk) x <= f;
endmodule
"#;

#[test]
fn dag_ff_waw_cross_group_order_diff() {
    let xd = run_signal_bits(FF_WAW_SRC, "x", false);
    let xg = run_signal_bits(FF_WAW_SRC, "x", true);
    assert_eq!(xg, xd, "x dag harus identik serial (urutan push WAW)");
    assert_eq!(xg, "0100", "x = f = 4 (pid 2 terakhir) ter-pinned");
    let qd = run_signal_bits(FF_WAW_SRC, "q", false);
    let qg = run_signal_bits(FF_WAW_SRC, "q", true);
    assert_eq!(qg, qd, "q dag harus identik serial");
    assert_eq!(qg, "0010", "q mengikuti b (c=2) pasca-posedge-2 ter-pinned");
}

/// Par-02 re-review fix BUG-1: iff wajib dieval PADA POSISI kandidat
/// (urutan pid), bukan di fase klasifikasi. en-FF (pid lebih kecil,
/// blocking) menulis en=1 pada posedge pertama; q (iff en) wajib
/// ter-trigger pada posedge PERTAMA juga (iff melihat en pasca-tulis).
/// Jika iff dieval terlalu dini, q kehilangan 1 trigger → q=9, bukan 10.
const FF_IFF_SRC: &str = r#"
module ff_iff_top;
  logic clk;
  logic en;
  logic [3:0] q;
  logic [3:0] d;
  logic [3:0] w;
  initial begin clk = 0; en = 0; q = 0; d = 4'd3; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) if (!en) en = 1'b1;
  always_ff @(posedge clk iff en) q <= q + 4'd1;
  always_ff @(posedge clk) w <= d;
endmodule
"#;

#[test]
fn dag_ff_iff_position_diff() {
    let qd = run_signal_bits(FF_IFF_SRC, "q", false);
    let qg = run_signal_bits(FF_IFF_SRC, "q", true);
    assert_eq!(qg, qd, "q dag harus identik serial (iff di posisi)");
    assert_eq!(qg, "1010", "q = 10 trigger (posedge pertama terhitung)");
    let ed = run_signal_bits(FF_IFF_SRC, "en", false);
    let eg = run_signal_bits(FF_IFF_SRC, "en", true);
    assert_eq!(eg, ed, "en dag harus identik serial");
    assert_eq!(eg, "1", "en = 1 (ditulis sekali) ter-pinned");
}

/// Par-02 tahap 2: follower combinational dalam clock domain (cycle
/// fusion) dieval paralel per grup disjoint. 1 FF (fused) + rantai 2
/// follower comb (y = x+1; z = y^FF) — z wajib melihat y grup sebelumnya
/// (snapshot refresh antar grup). Identik default vs fusion+DAG.
const CF_SRC: &str = r#"
module cf_top;
  logic clk;
  logic [7:0] a;
  logic [7:0] x, y, y2, z;
  initial begin clk = 0; a = 8'h0F; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) x <= a + 8'd1;
  assign y = x + 8'd1;
  assign y2 = x ^ 8'hAA;
  assign z = y ^ y2;
endmodule
"#;

fn run_cf(src: &str, name: &str, fusion: bool, dag: bool) -> String {
    let design = compile_str(src).expect("design harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 100);
    engine.use_cycle_fusion = fusion;
    engine.use_dag_parallel = dag;
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("sinyal '{name}' harus ada di design flat"));
    engine
        .state
        .read_signal(idx)
        .bits
        .iter()
        .rev()
        .map(|b| match b {
            mivon_ir::LogicVal::Zero => '0',
            mivon_ir::LogicVal::One => '1',
            _ => 'x',
        })
        .collect()
}

#[test]
fn dag_cycle_fusion_followers_parallel_diff() {
    // Default (tanpa fusion) = referensi.
    let cases = [
        ("x", "00010000"),  // 0x0F + 1 = 0x10
        ("y", "00010001"),  // 0x10 + 1 = 0x11
        ("y2", "10111010"), // 0x10 ^ 0xAA = 0xBA
        ("z", "10101011"),  // 0x11 ^ 0xBA = 0xAB
    ];
    for (name, expect) in cases {
        let dflt = run_cf(CF_SRC, name, false, false);
        let cf = run_cf(CF_SRC, name, true, true);
        assert_eq!(cf, dflt, "{name}: fusion+DAG harus identik default");
        assert_eq!(cf, expect, "{name}: nilai akhir ter-pinned");
    }
}

/// Par-02 tahap 2 re-review: pola inversi greedy non-transitif —
/// konflik (f0,f1) dan (f1,f2), f0-f2 bebas → G1=[f0,f2], G2=[f1].
/// f2 (G1) membaca B tulisan f1 (G2) → stale di iterasi 1, dikoreksi
/// fixed-point domain (iterasi ekstra) → hasil akhir identik serial.
/// Non-racy: tiap sinyal satu driver.
const CF_WAW_SRC: &str = r#"
module cf_waw_top;
  logic clk;
  logic [7:0] a;
  logic [7:0] x, A, B, C;
  initial begin clk = 0; a = 8'd5; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) x <= a;
  assign A = x + 8'd1;
  always_comb B = A + x;
  assign C = B + x;
endmodule
"#;

#[test]
fn dag_cf_waw_cross_group_diff() {
    let cases = [
        ("x", "00000101"), // 5
        ("A", "00000110"), // x + 1 = 6
        ("B", "00001011"), // A + x = 11
        ("C", "00010000"), // B + x = 16 (konvergen via fixed-point)
    ];
    for (name, expect) in cases {
        let dflt = run_cf(CF_WAW_SRC, name, false, false);
        let cf = run_cf(CF_WAW_SRC, name, true, true);
        assert_eq!(cf, dflt, "{name}: fusion+DAG harus identik default");
        assert_eq!(
            cf, expect,
            "{name}: nilai akhir ter-pinned (konvergensi fixed-point)"
        );
    }
}
