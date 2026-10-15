// TB 93_range_init — packed range bit/part-select positions + declaration-init
// edge sensitivity. Regression guard untuk fuzz-fix: bit-posisi fisik
// (verify_bad_0021) dan negedge palsu t=0 (verify_bad_0586).
`timescale 1ns/1ps
module tb_range_init;
  logic [0:3] asc;      // asc[0]=MSB ... asc[3]=LSB
  logic [7:4] d74;      // d74[7]=MSB ... d74[4]=LSB
  logic [3:0] rs_asc, rs_d74;
  logic [1:0] ps_d74a, ps_d74b, ps_asc;
  logic rst_n = 0;      // declaration initializer (LRM §6.8) = nilai awal
  int n_negedge;

  always @(negedge rst_n) n_negedge = n_negedge + 1;

  initial begin
    $display("ASRT_START tb_range_init");
    asc = 4'b1100;       // asc[0]=1(M), asc[3]=0(L)
    d74 = 4'b1010;       // d74[7]=1(M), d74[4]=0(L)
    rs_asc  = asc[0:3];  // 1100 = 12
    rs_d74  = d74[7:4];  // 1010 = 10
    ps_d74a = d74[4 +: 2]; // d74[5:4] = 10 = 2
    ps_d74b = d74[6 -: 2]; // d74[6:5] = 01 = 1
    ps_asc  = asc[0 +: 2];  // asc[0:1] = 11 = 3
    #1;
    $display("ASRT_RS_ASC=<%0d>", rs_asc);
    $display("ASRT_RS_D74=<%0d>", rs_d74);
    $display("ASRT_PS_D74A=<%0d>", ps_d74a);
    $display("ASRT_PS_D74B=<%0d>", ps_d74b);
    $display("ASRT_PS_ASC=<%0d>", ps_asc);
    $display("ASRT_NEDGE=<%0d>", n_negedge);
    $display("ASRT_END tb_range_init");
  end
endmodule
