// TB 54_decl_init — initializer cross-sim differential reference.
`timescale 1ns/1ps
module tb_decl_init;
  logic clk = 0;
  logic rst_n = 0;
  logic q;

  decl_init_demo dut (.clk(clk), .rst_n(rst_n), .q(q));

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_decl_init");
    #20;
    rst_n = 1;
    repeat (3) @(posedge clk);
    @(negedge clk);
    $display("ASRT_Q=<%0h>", q);
    assert (q !== 1'bx) else $error("ASRT_Q_X_BAD=<%0h>", q);
    $display("ASRT_END tb_decl_init");
    $finish;
  end
endmodule
