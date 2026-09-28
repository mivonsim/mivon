// Seed 47: class rand/randc + constraint + randomize() (elab+sim class path)
class pkt_item;
  rand  int unsigned addr;
  rand  int unsigned len;
  randc bit [1:0]    kind;
  bit [31:0]         data;

  constraint c_addr { addr inside {[0 : 1023]}; addr % 4 == 0; }
  constraint c_len  { len >= 1; len <= 16; (kind == 2) -> len <= 4; }

  function new();
    addr = 0;
    len  = 1;
    kind = 0;
  endfunction

  function string fmt();
    return $sformatf("pkt{kind=%0d addr=0x%0h len=%0d}", kind, addr, len);
  endfunction
endclass

module pkt_driver (input logic clk, input logic rst_n);
  pkt_item item;

  initial begin
    item = new();
    if (!item.randomize()) begin
      $display("randomize gagal");
    end
    $display("generated %s", item.fmt());
  end

  always @(posedge clk) begin
    if (rst_n && item != null) begin
      item.addr = item.addr + 4;
    end
  end
endmodule
