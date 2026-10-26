//! Regression test untuk bug EMI fuzzer (mivon-fuzz kampanye seed 42):
//! evaluator parallel (`evaluate_expr_simple` di simulator/parallel.rs) mem-X-kan
//! SELURUH hasil part-select bila sebagian di luar batas, sedangkan evaluator
//! serial (engine/eval/expr.rs) menerapkan LRM 1800 §11.5.1 per-bit:
//! bit dalam batas = nilai asli, bit luar batas = X.
//!
//! Gejala asli: menambah deklarasi dead-code (`wire [7:0] _fuzz_dn; assign
//! _fuzz_dn = 8'h00;`) menambah 1 proses combinational sehingga jumlah proses
//! melewati `min_processes_parallel` (default 4) → jalur parallel aktif →
//! `__port_u_child_x = xxxx` alih-alih `xx01` → mismatch EMI.
//!
//! Fix: RangeSelect/ExprRangeSelect di parallel.rs kini per-bit §11.5.1,
//! identik dengan jalur serial.

use super::*;

/// SV sumber bug: part-select out-of-range `a[3:0]` pada `a` 2-bit.
/// Jalur serial & parallel wajib memberi hasil identik `xx01`.
const EMI_PARTSEL_SRC: &str = r#"
module child #(
  parameter CW = 4
) (
  input  logic [CW-1:0] x,
  output logic [CW-1:0] q
);
  assign q = ~x;
endmodule

module top (
  input  logic        clk,
  input  logic        rst_n,
  input  logic [1:0]  a,
  input  logic [1:0]  b,
  output logic [1:0]  y,
  output logic [1:0]  flag
);
  logic [1:0] wr;
  child #(.CW(4)) u_child (.x(a[3:0]), .q(wr));
  logic [1:0] r;
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n)
      r <= '0;
    else
      r <= wr | b;
  end
  assign y = r;
  assign flag = (a > b) ? 2'd1 : (a == b) ? 2'd2 : 2'd0;
  initial begin clk = 0; forever #5 clk = ~clk; end
  initial begin rst_n = 0; a = 5; b = 3; #7 rst_n = 1; #3 a = 17; b = 9; end
  DEAD_CODE
endmodule
"#;

/// Jalankan EMI_PARTSEL_SRC pada konfigurasi parallel tertentu, kembalikan
/// nilai sinyal `name` dalam bentuk string bit (mis. "xx01").
fn run_emivariant(
    dead_code: &str,
    pcfg: mivon_simulator::simulator::parallel::ParallelConfig,
) -> String {
    let source = EMI_PARTSEL_SRC.replace("  DEAD_CODE", dead_code);
    let design = compile_str(&source).unwrap();
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 100);
    engine.set_parallel_config(pcfg);
    engine.run().unwrap();
    let sigs = engine.design.top.signals.clone();
    let idx = sigs
        .iter()
        .position(|s| s.name == "__port_u_child_x")
        .expect("sinyal __port_u_child_x harus ada");
    let val = engine.state.read_signal(idx);
    // Penyimpanan internal LSB-first; string ditulis MSB-first (sama dengan
    // --print-state/VCD) agar expected "xx01" terbaca alami.
    val.bits
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

/// Jalur serial (parallel dimatikan) — referensi semantik §11.5.1.
#[test]
fn test_emi_partselect_serial_reference() {
    let pcfg = mivon_simulator::simulator::parallel::ParallelConfig {
        parallel_processes: false,
        ..Default::default()
    };
    let x = run_emivariant("", pcfg);
    assert_eq!(
        x, "xx01",
        "serial: a[3:0] pada a 2-bit → bit dalam batas nilai asli, sisanya X"
    );
}

/// Bug asli fuzzer: proses comb ke-4+ mengaktifkan jalur parallel — hasil
/// wajib identik dengan jalur serial.
#[test]
fn test_emi_partselect_parallel_with_dead_code() {
    let pcfg = mivon_simulator::simulator::parallel::ParallelConfig::default();
    let x = run_emivariant("  wire [7:0] _fuzz_dn;\n  assign _fuzz_dn = 8'h00;", pcfg);
    assert_eq!(
        x, "xx01",
        "parallel (threshold terlampaui via dead-code assign): harus identik dengan serial"
    );
}

/// Parallel dipaksa aktif untuk jumlah proses kecil.
#[test]
fn test_emi_partselect_parallel_forced() {
    let pcfg = mivon_simulator::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        ..Default::default()
    };
    let x = run_emivariant("", pcfg);
    assert_eq!(
        x, "xx01",
        "parallel dipaksa aktif: part-select OOB per-bit §11.5.1"
    );
}

/// Varian ExprRangeSelect: `a[base+:4]` pada `a` 2-bit, base konstan 0.
#[test]
fn test_emi_expr_partselect_parallel() {
    let source = r#"
module tb;
    reg [1:0] a;
    wire [3:0] w;
    assign w = a[0 +: 4];
    initial begin
        a = 2'b01;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    // Serial
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design.clone(), 10);
    let ser = mivon_simulator::simulator::parallel::ParallelConfig {
        parallel_processes: false,
        ..Default::default()
    };
    engine.set_parallel_config(ser);
    engine.run().unwrap();
    let sigs = engine.design.top.signals.clone();
    let idx = sigs.iter().position(|s| s.name == "w").unwrap();
    let serial = engine.state.read_signal(idx).clone();
    // Parallel dipaksa
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 10);
    let par = mivon_simulator::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        ..Default::default()
    };
    engine.set_parallel_config(par);
    engine.run().unwrap();
    let parallel = engine.state.read_signal(idx).clone();
    assert_eq!(serial.width, 4);
    assert_eq!(
        serial.bits[0],
        mivon_ir::LogicVal::One,
        "bit 0 dalam batas = nilai asli"
    );
    assert_eq!(
        serial.bits[1],
        mivon_ir::LogicVal::Zero,
        "bit 1 dalam batas = nilai asli"
    );
    assert_eq!(serial.bits[2], mivon_ir::LogicVal::X, "bit 2 OOB = X");
    assert_eq!(parallel, serial, "parallel harus identik dengan serial");
}

/// Regresi hang parser (ditemukan mivon-fuzz, bugdb Hang opentitan kmac):
/// fragmen `clocking cb @(posedge tck);` TERPOTONG tanpa `endclocking` di level
/// unit (stray decl masuk module implisit) → `parse_clocking_block` catch-all
/// `_ => advance()` memutar di EOF selamanya. `advance()` hanya clamp pos ke
/// len melewati step-limit; loop TIDAK pernah break → infinite loop yang
/// melewati safety counter (`parse_steps`/`peek_count` — keduanya di-reset,
/// pos tidak pernah berubah → stuck detection di parse_design level-atas tak
/// pernah tercapai karena kontrol tidak pernah kembali ke sana).
///
/// Fix: specify.rs `parse_clocking_block` — arm `_` break bila `Token::Eof`.
#[test]
fn test_parser_no_hang_on_truncated_clocking_fragment() {
    let src = r#"
class kmac_smoke_vseq extends kmac_base_vseq;
  task body();
    fork
      begin : send_kmac_req
        wait (cond);
      end
    join
  endtask
endclass
wire [16-1:0] fz_863;
clocking cb @(posedge tck);
input tms;
"#;
    // Watchdog 10s: sebelum fix input ini hang >170 s (infinite loop parser).
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(compile_str(src));
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("compile truncated clocking block harus selesai <10s (dulu hang? regresi)");
    // Hang = regresi (timeout panik di atas). Selesai cepat = ok/err biasa.
    let _ = result;
}

/// Regresi panic elaborasi (ditemukan mivon-fuzz seed 42, corpus opentitan,
/// 6× `attempt to add with overflow`): concat `{ sig[0+:(IDW-STIDW)], ... }`
/// dengan `IDW = top_pkg::TL_AIW` (symbol package tak-resolved → 0) dan
/// `STIDW = $clog2(M)` (2) → `IDW - STIDW` = -2 → const-eval memberi i64
/// negatif → `as usize` mem-bungkus ke 2^64-2 → `ExprRangeSelect(hi=u64::MAX-1)`
/// → `expr_approx_width` concat `.sum()` overflow panic.
///
/// Fix dua lapis (mivon-elaboration):
/// 1. `stmt.rs expr_approx_width` — Concat/Replicate kini saturasi (lebar
///    hanya perkiraan utk konteks sizing — tidak boleh panic).
/// 2. `expr.rs RangeSelect/ExprRangeSelect` — bound const `max(0)` sebelum
///    `as usize`, negatif = OOB (engine mengisi X per §11.5.1).
#[test]
fn test_no_panic_concat_negative_range_select() {
    let src = r#"
module tlul_socket_m1 #(
  parameter int unsigned  M         = 4,
) (
);
  localparam int unsigned IDW   = top_pkg::TL_AIW;
  localparam int unsigned STIDW = $clog2(M);
    assign shifted_id = {
      tl_h_i[i].a_source[0+:(IDW-STIDW)],
      reqid_sub
    };
endmodule
"#;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(compile_str(src));
    });
    let r = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("compile concat lebar negatif harus selesai <10s (dulu panic/regresi?)");
    assert!(
        r.is_ok(),
        "part-select lebar negatif/unresolved harus error elegan, bukan panic"
    );
}

/// Regresi diagnostic RT0001 (ditemukan mivon-fuzz seed 42): concat part-select
/// member `tl_h_i[i].a_source[0+:(IDW-STIDW)]` dengan `tl_h_i[i]` tak-resolved
/// → elaborator fallback `Err(_)` emit `HierRef("")` → engine luntur jadi
/// `error[RT0001]: hierarchical signal '' not found` — nama KOSONG (diagnostic
/// tak berguna untuk input tak-deklar).
///
/// Fix: `expr.rs` MemberAccess fallback — kalau `build_hier_name` kosong (obj
/// berupa Index/PartSelect tak-resolved), pakai nama field (`a_source`) supaya
/// pesan error memuat sinyal yang sebenarnya.
#[test]
fn test_rt0001_reports_real_signal_name_not_empty() {
    let src = r#"
module tlul_socket_m1 #(
  parameter int unsigned  M         = 4,
) (
);
  localparam int unsigned IDW   = top_pkg::TL_AIW;
  localparam int unsigned STIDW = $clog2(M);
    assign shifted_id = {
      tl_h_i[i].a_source[0+:(IDW-STIDW)],
      reqid_sub
    };
endmodule
"#;
    let r = super::simulate_str(src, 10).unwrap_err();
    let msg = r.to_string();
    assert!(
        msg.contains("a_source") && !msg.contains("signal '' not found"),
        "RT0001 harus memuat nama field, bukan kosong: {}",
        msg
    );
    assert!(
        msg.contains("not found"),
        "harus jadi not-found error yang bisa dibaca: {}",
        msg
    );
}

// ─── Regresi mivon-fuzz (kampanye sim seed 9999) — evaluator parallel ───
//
// Dua bug differential default-vs-dag ditemukan fuzzer:
// 1. `IrExpr::Cond` di evaluate_expr_simple: kondisi X → `to_bool().unwrap_or(false)`
//    → pilih cabang else. Serial mengikuti IEEE 1800 §11.4.11 Tabel 11-22:
//    kondisi unknown → merge bitwise (bit sama → nilai itu, beda → X).
//    Gejala: `assign y = dbgon ? x : y2` dgn dbgon=X → serial X, dag 0/1.
// 2. Tidak ada 2-state coercion X/Z→0 pada read/write jalur parallel —
//    `output bit red = (state == RED)` dgn state=X → serial 0, dag X.
//    Serial menerapkan sanitize_for_2state di lvalue.rs (write) & expr.rs (read).
//
// Fix: parallel.rs Cond miror merge serial + sanitize_for_2state read & tiap
// write site.

/// Jalankan satu source pada config parallel tertentu, kembalikan string bit
/// (MSB-first) dari sinyal `name`.
fn run_pcfg_signal(
    source: &str,
    name: &str,
    pcfg: mivon_simulator::simulator::parallel::ParallelConfig,
) -> String {
    let design = compile_str(source).unwrap();
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 100);
    engine.set_parallel_config(pcfg);
    engine.run().unwrap();
    let sigs = engine.design.top.signals.clone();
    let idx = sigs
        .iter()
        .position(|s| s.name.as_str() == name)
        .unwrap_or_else(|| panic!("sinyal {name} harus ada"));
    let val = engine.state.read_signal(idx);
    val.bits
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

/// Ternary kondisi X: serial & parallel wajib merge bitwise (Tabel 11-22).
#[test]
fn test_ternary_x_cond_serial_vs_parallel() {
    let src = r#"
module top;
  logic       sel;         // X (tidak di-drive)
  logic [1:0] tv, fv;
  wire  [1:0] y;
  assign tv = 2'b10;
  assign fv = 2'b01;
  assign y  = sel ? tv : fv;   // sel=X → bitwise merge 10 & 01 = xx
  initial begin #1 $finish; end
endmodule
"#;
    let serial = mivon_simulator::simulator::parallel::ParallelConfig {
        parallel_processes: false,
        ..Default::default()
    };
    let par = mivon_simulator::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        ..Default::default()
    };
    let s = run_pcfg_signal(src, "y", serial);
    let p = run_pcfg_signal(src, "y", par);
    assert_eq!(s, "xx", "serial: sel=X → merge bitwise = xx, got {s}");
    assert_eq!(
        p, s,
        "parallel: ternary X-cond harus identik serial (Tabel 11-22), serial={s} par={p}"
    );
}

/// 2-state coercion: `output bit` menerima X → 0 di serial DAN parallel.
#[test]
fn test_bit_output_coercion_serial_vs_parallel() {
    let src = r#"
module sub(output bit red, input logic [1:0] s);
  always_comb red = (s == 2'b00);   // s=X → == meng-X → bit coercion 0
endmodule
module top;
  logic [1:0] s;
  bit r;
  sub u(.red(r), .s(s));
  initial begin #1 $finish; end
endmodule
"#;
    let serial = mivon_simulator::simulator::parallel::ParallelConfig {
        parallel_processes: false,
        ..Default::default()
    };
    let par = mivon_simulator::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        ..Default::default()
    };
    let s = run_pcfg_signal(src, "r", serial);
    let p = run_pcfg_signal(src, "r", par);
    assert_eq!(s, "0", "serial: bit menerima X → 0, got {s}");
    assert_eq!(
        p, s,
        "parallel: 2-state coercion harus identik serial, serial={s} par={p}"
    );
}

/// PERF-06: dua kelompok comb dalam satu delta. `always @(a)` sengaja
/// memakai sensitivity EKSPLISIT (hanya `a`), jadi proses itu TIDAK
/// terpicu ulang saat `b` berubah — satu-satunya cara ia melihat `b` baru
/// adalah snapshot antar kelompok yang di-refresh. Tanpa refresh ia membaca
/// b lama (0) sehingga d = 0, sedangkan jalur serial = 1.
#[test]
fn test_layered_comb_chain_sees_previous_group_writes() {
    let src = r#"
module layer_chain(input logic a, output logic d);
  logic b, c;
  assign b = a & 1'b1;
  always @(a) c = b & a;
  assign d = c & a;
endmodule
module top;
  logic a;
  logic d;
  layer_chain u(.a(a), .d(d));
  initial begin a = 0; #5 a = 1; end
endmodule
"#;
    let serial = mivon_simulator::simulator::parallel::ParallelConfig {
        parallel_processes: false,
        ..Default::default()
    };
    let par = mivon_simulator::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        ..Default::default()
    };
    let s = run_pcfg_signal(src, "d", serial);
    let p = run_pcfg_signal(src, "d", par);
    assert_eq!(
        s, "1",
        "serial: rantai 4 tahap harus menghasilkan d=1, got {s}"
    );
    assert_eq!(
        p, s,
        "parallel: grup berikutnya wajib melihat tulis grup sebelumnya, serial={s} par={p}"
    );
}

/// PERF-06 (review): proses menulis MEMBER alias — storage berubah hanya di
/// canonical, jadi refresh snapshot antar kelompok wajib melewati
/// alias_redirect (mirror write_signal). Tanpa redirect, grup berikutnya
/// membaca canonical basi (b lama = 0) sehingga d = 0, serial = 1.
#[test]
fn test_layered_chain_refresh_follows_alias_redirect() {
    let src = r#"
module top;
  logic a;
  logic b, c, x, y, d;
  alias c = b;
  assign c = a;
  assign x = a;
  assign y = a;
  always @(a) d = b & x;
  initial begin a = 0; #5 a = 1; end
endmodule
"#;
    let serial = mivon_simulator::simulator::parallel::ParallelConfig {
        parallel_processes: false,
        ..Default::default()
    };
    let par = mivon_simulator::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        ..Default::default()
    };
    let s = run_pcfg_signal(src, "d", serial);
    let p = run_pcfg_signal(src, "d", par);
    assert_eq!(s, "1", "serial: alias c=b, b harus 1 setelah a=1, got {s}");
    assert_eq!(
        p, s,
        "parallel: refresh wajib mengikuti alias_redirect ke canonical, serial={s} par={p}"
    );
}
