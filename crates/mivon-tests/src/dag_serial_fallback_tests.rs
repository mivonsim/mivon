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

/// Par-02 tahap 3 (body_write_kinds): FF BlockingOnly dgn RAW intra-body —
/// `tmp` ditulis blocking lalu dibaca di stmt berikutnya (overlay blocking
/// = semantik serial), commit ke `a` dan `b`. Companion NbaOnly `w <= 5`
/// membuat batch >= 2 → jalur apply ganda (blocking immediate + NBA push).
const FF_BLK_TMP_SRC: &str = r#"
module ff_blk_tmp_top;
  logic clk;
  logic [7:0] a, b, tmp;
  logic [3:0] w;
  initial begin clk = 0; a = 0; b = 0; tmp = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) begin
    tmp = a + 8'd1;
    b = tmp;
    a = tmp;
  end
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_blocking_tmp_batch_diff() {
    for (name, expect) in [("a", "00001010"), ("b", "00001010"), ("w", "0101")] {
        let d = run_signal_bits(FF_BLK_TMP_SRC, name, false);
        let g = run_signal_bits(FF_BLK_TMP_SRC, name, true);
        assert_eq!(
            g, d,
            "{name}: DAG harus identik serial (BlockingOnly + RAW intra-body)"
        );
        assert_eq!(g, expect, "{name}: nilai akhir ter-pinned (10 posedge)");
    }
}

/// Par-02 tahap 3: FF BlockingOnly dgn LoopFor berbatas RUNTIME (`i < n`,
/// n sinyal — tidak di-unroll elaborator) yang menjumlah, lalu commit
/// blocking. Companion NbaOnly `w <= 5` → batch >= 2 dgn LoopFor utuh
/// di IR (body_write_kinds arm LoopFor init/step BlockingAssign).
const FF_LOOP_RT_SRC: &str = r#"
module ff_loop_rt_top;
  logic clk;
  logic [2:0] n;
  logic [7:0] acc, b;
  logic [3:0] w;
  integer i;
  initial begin clk = 0; n = 3; acc = 0; b = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) begin
    acc = 8'd0;
    for (i = 0; i < n; i = i + 1)
      acc = acc + 8'd2;
    b = acc;
  end
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_loop_runtime_bound_batch_diff() {
    for (name, expect) in [("acc", "00000110"), ("b", "00000110"), ("w", "0101")] {
        let d = run_signal_bits(FF_LOOP_RT_SRC, name, false);
        let g = run_signal_bits(FF_LOOP_RT_SRC, name, true);
        assert_eq!(
            g, d,
            "{name}: DAG harus identik serial (LoopFor runtime-bound)"
        );
        assert_eq!(
            g, expect,
            "{name}: nilai akhir ter-pinned (n=3 iterasi x 2)"
        );
    }
}

/// Par-02 tahap 3 — regresi WAW batch-vs-serial inline (ditemukan
/// self-review R186): FF batchable (s <= 7) MENULIS sinyal yang sama
/// dgn FF non-batchable serial inline ($display → body_write_kinds
/// None; s <= s+1). Serial: push NBA pid-order → pemenang = FF serial
/// (s = s_lama+1). Tanpa guard WAW-vs-serial, kandidat batch di-defer
/// → push-nya TERAKHIR → s = 7 (divergen). Guard demosi FF batch →
/// urutan push serial dipulihkan.
const FF_WAW_SERIAL_SRC: &str = r#"
module ff_waw_serial_top;
  logic clk;
  logic [7:0] s;
  logic [3:0] w;
  initial begin clk = 0; s = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) s <= 8'd7;
  always_ff @(posedge clk) begin
    $display("tick");
    s <= s + 8'd1;
  end
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_waw_batch_vs_serial_inline_diff() {
    for (name, expect) in [("s", "00001010"), ("w", "0101")] {
        let d = run_signal_bits(FF_WAW_SERIAL_SRC, name, false);
        let g = run_signal_bits(FF_WAW_SERIAL_SRC, name, true);
        assert_eq!(
            g, d,
            "{name}: DAG harus identik serial (WAW batch vs serial inline)"
        );
        assert_eq!(
            g, expect,
            "{name}: FF serial inline harus pemenang terakhir (s=10)"
        );
    }
}

/// Regresi review R186 BUG-1: FF NbaOnly batch membaca sinyal yang
/// ditulis BLOCKING oleh FF serial inline ($display -> kinds None).
/// Serial: q membaca en lama. Tanpa guard, q di-defer -> snapshot
/// batch pasca-loop berisi en baru -> q = en baru (divergen). Guard
/// serial_blocking_writes mendemosi q -> inline di posisinya.
const FF_BUG1_SRC: &str = r#"
module ff_bug1_top;
  logic clk;
  logic [3:0] en, q, w;
  initial begin clk = 0; en = 0; q = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) q <= en;
  always_ff @(posedge clk) begin
    $display("t");
    en = en + 4'd1;
  end
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_reads_serial_blocking_write_diff() {
    for (name, expect) in [("q", "1001"), ("en", "1010"), ("w", "0101")] {
        let d = run_signal_bits(FF_BUG1_SRC, name, false);
        let g = run_signal_bits(FF_BUG1_SRC, name, true);
        assert_eq!(
            g, d,
            "{name}: DAG harus identik serial (baca blocking write serial inline)"
        );
        assert_eq!(g, expect, "{name}: q harus membaca en PRA-increment (9)");
    }
}

/// Regresi review R186 BUG-2: FF BlockingOnly PEMBACA blocking write
/// FF batch lain tidak didemosi (hanya penulisnya, via stale readers).
/// Serial: q2 = q1 lama. Tanpa guard: penulis q1 didemosi-inline,
/// pembaca q2 tetap ter-batch -> snapshot pasca-loop berisi q1 baru.
const FF_BUG2_SRC: &str = r#"
module ff_bug2_top;
  logic clk;
  logic [3:0] q1, q2, w;
  initial begin clk = 0; q1 = 0; q2 = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) q2 = q1;
  always_ff @(posedge clk) q1 = ~q1;
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_reads_batch_blocking_write_diff() {
    for (name, expect) in [("q1", "0000"), ("q2", "1111"), ("w", "0101")] {
        let d = run_signal_bits(FF_BUG2_SRC, name, false);
        let g = run_signal_bits(FF_BUG2_SRC, name, true);
        assert_eq!(
            g, d,
            "{name}: DAG harus identik serial (RAW lintas FF blocking)"
        );
        assert_eq!(g, expect, "{name}: q2 harus membaca q1 PRA-toggle (15)");
    }
}

/// Regresi review R186 BUG-3: Err evaluator paralel di grup akhir
/// membuat push NBA grup sukses hilang senyap (return dini melewati
/// flush). $countones -> SysFunc -> Err di evaluate_expr_simple; FF
/// d (grup sukses) push NBA tertahan wajib tetap ter-flush.
const FF_BUG3_SRC: &str = r#"
module ff_bug3_top;
  logic clk;
  logic [3:0] en, d, q, e, w;
  initial begin clk = 0; en = 3; d = 0; q = 0; e = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) d <= en;
  always_ff @(posedge clk) q <= d;
  always_ff @(posedge clk) e <= ($countones(q) > 0) ? 4'd1 : 4'd2;
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_err_group_keeps_batch_nba_pushes_diff() {
    for (name, expect) in [("d", "0011"), ("q", "0011"), ("e", "0001"), ("w", "0101")] {
        let dflt = run_signal_bits(FF_BUG3_SRC, name, false);
        let g = run_signal_bits(FF_BUG3_SRC, name, true);
        assert_eq!(
            g, dflt,
            "{name}: DAG harus identik serial (Err grup tidak boleh buang push NBA)"
        );
        assert_eq!(g, expect, "{name}: push NBA grup sukses wajib ter-commit");
    }
}

/// Regresi review R186 BUG-2: WAW antar dua kandidat NbaOnly batch.
/// N1 (x <= 5) dan N2 (x <= y; y ditulis blocking FF C) sama-sama
/// menulis x. N2 ter-demosi (baca blocking write y) -> push NBA x
/// inline saat fase 2; tanpa guard WAW Nba-vs-Nba, N1 di-defer ->
/// push-nya SETELAH N2 -> x = 5, padahal serial pemenangnya N2
/// (pid lebih besar) -> x = y = 3.
const FF_BUG2WAW_SRC: &str = r#"
module ff_bug2waw_top;
  logic clk;
  logic [3:0] x, y;
  initial begin clk = 0; x = 0; y = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) x <= 4'd5;
  always_ff @(posedge clk) x <= y;
  always_ff @(posedge clk) y = 4'd3;
endmodule
"#;

#[test]
fn dag_ff_waw_nba_pair_diff() {
    for (name, expect) in [("x", "0011"), ("y", "0011")] {
        let d = run_signal_bits(FF_BUG2WAW_SRC, name, false);
        let g = run_signal_bits(FF_BUG2WAW_SRC, name, true);
        assert_eq!(
            g, d,
            "{name}: DAG harus identik serial (WAW dua kandidat NbaOnly)"
        );
        assert_eq!(
            g, expect,
            "{name}: pemenang WAW harus N2 pid-terbesar (x = y = 3)"
        );
    }
}

/// Regresi review R186 BUG-4: assign NBA ber-delay (`q <= #2 d`)
/// wajib serial (body_write_kinds None utk delay Some). Catatan: pada
/// engine ini serial juga mengabaikan delay secara final-state (q = r
/// = d lama) — test ini mengunci klasifikasi serial + pinned; bila
/// klasifikasi dilonggarkan dan evaluator paralel berbeda, differential
/// menangkap.
const FF_BUG4_SRC: &str = r#"
module ff_bug4_top;
  logic clk;
  logic [3:0] d, q, r, w;
  initial begin clk = 0; d = 0; q = 0; r = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) d <= d + 4'd1;
  always_ff @(posedge clk) begin
    q <= #2 d;
    r <= d;
  end
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_delayed_assign_serial_diff() {
    for (name, expect) in [("d", "1010"), ("q", "1001"), ("r", "1001"), ("w", "0101")] {
        let dflt = run_signal_bits(FF_BUG4_SRC, name, false);
        let g = run_signal_bits(FF_BUG4_SRC, name, true);
        assert_eq!(
            g, dflt,
            "{name}: DAG harus identik serial (assign ber-delay -> serial)"
        );
        assert_eq!(
            g, expect,
            "{name}: pinned serial (delay tak mengubah final-state)"
        );
    }
}

/// Regresi review R186 BUG-5: Foreach wajib serial — evaluator
/// paralel hardcode elem_width = 1 (serial pakai sig_info.elem_width);
/// unpacked int array (elemen 32-bit) -> iterasi/index salah di jalur
/// paralel. Probe serial: foreach unpacked array ter-index benar.
const FF_BUG5_SRC: &str = r#"
module ff_bug5_top;
  logic clk;
  int uarr[4];
  logic [7:0] a0, a1;
  logic [3:0] w;
  initial begin
    clk = 0; uarr[0] = 1; uarr[1] = 3; uarr[2] = 0; uarr[3] = 0;
    a0 = 0; a1 = 0; w = 0; #40;
  end
  always #5 clk = ~clk;
  always_ff @(posedge clk) begin
    foreach (uarr[j]) begin
      if (j == 0) a0 <= uarr[j][7:0];
      else if (j == 1) a1 <= uarr[j][7:0];
    end
  end
  always_ff @(posedge clk) w <= 4'd5;
endmodule
"#;

#[test]
fn dag_ff_foreach_wide_elems_serial_diff() {
    for (name, expect) in [("a0", "00000001"), ("a1", "00000011"), ("w", "0101")] {
        let d = run_signal_bits(FF_BUG5_SRC, name, false);
        let g = run_signal_bits(FF_BUG5_SRC, name, true);
        assert_eq!(
            g, d,
            "{name}: DAG harus identik serial (Foreach elemen 32-bit -> serial)"
        );
        assert_eq!(
            g, expect,
            "{name}: elemen array harus ter-copy utuh per 8-bit"
        );
    }
}

/// Par-02 tahap 4: seq_bodies dalam clock-domain fused dieval via
/// mesinari batch tahap 3 (SeqCand::classify + run_seq_batch di
/// evaluate_clock_domain Step 1). Domain = 2 FF clock sama: FF1
/// BlockingOnly dgn RAW intra-body, FF2 NbaOnly — keduanya batchable
/// tanpa demosi -> jalur batch paralel di dalam domain (fixed-point
/// Step 1), commit NBA identik serial. CATATAN: FF TANPA self-feedback
/// — fixed-point domain me-re-eval bodies tiap iterasi (commit_nba di
/// dalam loop), FF self-feedback terakumulasi di mode fusion (pre-
/// existing, beda dari default) — desain ini konvergen di semua mode.
const CF_DOMAIN_SEQ_SRC: &str = r#"
module ff_domain_top;
  logic clk;
  logic [7:0] a, b, tmp, w;
  initial begin clk = 0; a = 5; b = 0; tmp = 0; w = 0; #40; end
  always #5 clk = ~clk;
  always_ff @(posedge clk) begin
    tmp = a + 8'd1;
    b = tmp;
  end
  always_ff @(posedge clk) w <= 8'd5;
endmodule
"#;

#[test]
fn dag_cf_domain_seq_batch_diff() {
    for (name, expect) in [
        ("a", "00000101"),
        ("b", "00000110"),
        ("tmp", "00000110"),
        ("w", "00000101"),
    ] {
        let dflt = run_cf(CF_DOMAIN_SEQ_SRC, name, false, false);
        let fus = run_cf(CF_DOMAIN_SEQ_SRC, name, true, false);
        let gd = run_cf(CF_DOMAIN_SEQ_SRC, name, true, true);
        assert_eq!(fus, dflt, "{name}: fusion saja harus identik default");
        assert_eq!(
            gd, fus,
            "{name}: batch domain (tahap 4) harus identik serial domain"
        );
        assert_eq!(gd, expect, "{name}: nilai akhir ter-pinned (konvergen)");
    }
}
