// tb_01_counter — drive clk/rst, verifikasi counter secara mandiri.
module tb_counter;
  logic clk = 0;
  logic rst_n = 0;
  logic en = 1;
  logic [7:0] count;
  logic done;

  counter #(.W(8), .MAX(5)) dut (
    .clk(clk), .rst_n(rst_n), .en(en), .count(count), .done(done)
  );

  always #5 clk = ~clk;

  initial begin
    rst_n = 0;
    repeat (2) @(posedge clk);
    // Deassert TIDAK di slot edge (race ordering tak dispesifikasi LRM) —
    // midpoint setelah negedge agar deterministik lintas simulator.
    @(negedge clk);
    rst_n = 1;
    repeat (8) @(posedge clk);
    // #1: baca POST-NBA (nilai settle) — tanpa ini $display bisa membaca
    // pre-edge (race urutan Active region, tak dispesifikasi LRM).
    #1;
    $display("ASRT_COUNT=<%0d>", count);
    $display("ASRT_DONE=<%0d>", done);
    $finish;
  end
endmodule