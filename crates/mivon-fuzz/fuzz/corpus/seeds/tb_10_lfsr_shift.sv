// tb_10_lfsr — drive clk/load, verifikasi LFSR + shifter (satu modul tb).
module tb_lfsr_shift;
  logic clk = 0;
  logic rst_n = 0;
  logic en = 1;
  logic [7:0] data_out;

  logic clk2 = 0;
  logic rst_n2 = 0;
  logic load = 0;
  logic [15:0] din = 16'hA5A5;
  logic [15:0] dout;

  lfsr #(.W(8)) lfsr_dut (.clk(clk), .rst_n(rst_n), .en(en), .data_out(data_out));
  shifter #(.W(16)) sh_dut (.clk(clk2), .rst_n(rst_n2), .load(load), .din(din), .dout(dout));

  always #5 clk = ~clk;
  always #5 clk2 = ~clk2;

  initial begin
    rst_n = 0;
    load = 0; din = 16'hA5A5;
    repeat (2) @(posedge clk);
    // #1: tulis SETELAH edge (bukan di slot edge — race urutan Active
    // region dgn always_ff pada edge sama; golden iverilog terkadang
    // menang urutan → divergensi cross-sim).
    #1;
    rst_n = 1;
    rst_n2 = 1;
    $display("ASRT_SEED=<%0d>", data_out);
    repeat (4) @(posedge clk);
    #1;
    $display("ASRT_LFSR4=<%0d>", data_out);
    // #1 setelah edge sebelum clear: DUT wajib baca load=1 dulu (pulse
    // di slot edge = race → load bisa hilang: LOADED=0 vs golden 42405).
    load = 1; @(posedge clk2); #1; load = 0;
    #1;
    $display("ASRT_LOADED=<%0d>", dout);
    @(posedge clk2);
    #1;
    $display("ASRT_SHIFT1=<%0d>", dout);
    $finish;
  end
endmodule