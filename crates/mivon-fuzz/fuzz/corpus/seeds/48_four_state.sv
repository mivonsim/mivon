// Seed 48: 4-state X/Z propagation — gap yang ditemukan sweep differential
// vs iverilog -g2012 (bug F82: X/Z dihapus const-fold concat).
//
// Semua_cases punya marker ASRT_*=<val> supaya bisa dibandingkan silang
// dengan iverilog+vvp (oracle_icarus). Nilai X/Z sengaja dipakai karena
// area 4-state adalah yang paling rapuh di elaborator/simulator: X bisa
// hilang diam-diam di const-fold, di concat, di reduction, di casez/casex.

`timescale 1ns/1ps
module four_state_probe (
  input  logic [3:0] sel,
  input  logic       en,
  output logic [7:0] q
);
  // Concat constant-fold: X/Z harus bertahan di setiap elemen.
  wire [7:0] c_x0   = {4'bx,   4'b0};
  wire [7:0] c_0x   = {4'b0,   4'bx};
  wire [7:0] c_zA   = {4'bz,   4'hA};
  wire [7:0] c_x1z0 = {4'bx1z0, 4'b0000};

  // Reduction atas input 4-state.
  wire r_or  = |sel;
  wire r_and = &sel;

  // Zero-extension dan context: operand X tak boleh jadi 0 diam-diam.
  wire [7:0] ext_x = {4'b0, sel};

  // `always @*` bukan `always_comb`: iverilog 11 belum mendukung constant
  // select di `always_*` (`sorry: constant selects in always_* processes are
  // not currently supported`) dan itu jadi warning yang menutupi diff
  // marker asli. `@*` dipilih agar reference iverilog bersih.
  always @* begin
    q = en ? (c_x0 ^ c_0x ^ c_zA ^ c_x1z0 ^ {4'b0, ext_x[7:4]})
           : 8'h00;
    if (r_or)  q[7] = 1'b1;
    if (r_and) q[6] = 1'b1;
  end
endmodule