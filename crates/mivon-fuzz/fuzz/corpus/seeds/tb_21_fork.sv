// TB 21_fork — fork/join_any cross-sim differential reference.
`timescale 1ns/1ps
module tb_fork;
  logic clk = 0;
  logic [3:0] result;

  fork_demo dut (.clk(clk), .result(result));

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_fork");
    repeat (3) @(posedge clk);
    @(negedge clk);
    // join_any keluar saat branch pertama selesai (t=5, result=1) lalu #5 →
    // result=F di t=10 — TAPI branch kedua tetap jalan dan menimpa result=2
    // di t=15 (join_any tak mematikan branch sisa). TB sampling di t=30 →
    // 2 di SEMUA simulator (mivon == iverilog, terverifikasi).
    assert (result == 4'h2) else $error("ASRT_FORK_BAD=<%0h>", result);
    $display("ASRT_FORK=<%0h>", result);
    $display("ASRT_END tb_fork");
    $finish;
  end
endmodule