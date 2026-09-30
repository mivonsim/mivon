// tb_19_segment_bcd — drive bcd + clk/rst, verifikasi decoder + counter.
module tb_segment_bcd;
  logic [3:0] bcd = 0;
  logic [6:0] seg;

  logic clk = 0;
  logic rst_n = 0;
  logic [3:0] cnt;
  logic carry;

  segment_dec dec (.bcd(bcd), .seg(seg));
  bcd_counter ctr (.clk(clk), .rst_n(rst_n), .bcd(cnt), .carry(carry));

  always #5 clk = ~clk;

  initial begin
    bcd = 4'h0; #1;
    $display("ASRT_SEG_0=<%0d>", seg);
    bcd = 4'h1; #1;
    $display("ASRT_SEG_1=<%0d>", seg);
    bcd = 4'h9; #1;
    $display("ASRT_SEG_9=<%0d>", seg);
    bcd = 4'hF; #1;
    $display("ASRT_SEG_F=<%0d>", seg);
    rst_n = 0;
    repeat (10) @(posedge clk);
    #1;
    rst_n = 1;
    repeat (2) @(posedge clk);
    // #1: baca POST-NBA (tanpa ini $display bisa baca nilai pre-edge —
    // race urutan Active region, tak dispesifikasi LRM).
    #1;
    $display("ASRT_BCD2=<%0d>", cnt);
    repeat (8) @(posedge clk);
    #1;
    $display("ASRT_BCD10=<%0d>", cnt);
    $display("ASRT_CARRY=<%0d>", carry);
    @(posedge clk);
    #1;
    $display("ASRT_BCD11=<%0d>", cnt);
    $finish;
  end
endmodule