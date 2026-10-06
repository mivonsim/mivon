// TB 48_four_state — 4-state X/Z probe dengan marker ASRT (cross-sim
// differential reference vs iverilog -g2012).
//
// Setiap stimulus menghasilkan marker `ASRT_<nama>=<nilai>` supaya
// oracle_icarus bisa membandingkan stream mivon vs iverilog. Nilai X/Z
// sengaja: area 4-state adalah tempat X hilang diam-diam (bug F82 di
// const-fold concat). Divider ini memb就让 fuzzer menguji apakah mutasi
// merusak propagasi X, apakah X bocor ke jalur 0/1 yang seharusnya
// ternerisasi, dan apakah reduction consistent.

`timescale 1ns/1ps
module tb_four_state;
  logic [3:0] sel;
  logic       en;
  logic [7:0] q;

  four_state_probe dut (.sel(sel), .en(en), .q(q));

  initial begin
    $display("ASRT_START tb_four_state");

    // en=0 → q harus 0 (jalur inaktif, tak ada X yang boleh bocor).
    en = 1'b0; sel = 4'bxxxx;
    #1 $display("ASRT_Q_IDLE=%h", q);

    // en=1 dengan X di concat → q boleh X (L(R) tak bisa menentukan).
    en = 1'b1; sel = 4'bxxxx;
    #1 $display("ASRT_Q_XSEL=%h", q);

    // en=1 dengan sel 0 → concat tetap X dari literal X di probe.
    en = 1'b1; sel = 4'b0000;
    #1 $display("ASRT_Q_ZEROSEL=%h", q);

    // en=1 dengan semua satu → reduction or = 1, and = 0.
    en = 1'b1; sel = 4'b1111;
    #1 $display("ASRT_Q_ALLONE=%h", q);

    // en=1 dengan semua nol → reduction or = 0.
    en = 1'b1; sel = 4'b0000;
    #1 $display("ASRT_Q_ALLZERO=%h", q);

    // en=1 dengan satu hot → or = 1, and = 0.
    en = 1'b1; sel = 4'b0010;
    #1 $display("ASRT_Q_ONEHOT=%h", q);

    $display("ASRT_END tb_four_state");
    $finish;
  end
endmodule