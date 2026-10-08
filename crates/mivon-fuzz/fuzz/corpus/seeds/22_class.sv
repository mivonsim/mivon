// Seed 22: class-based verification pattern (self-contained)
class packet;
  rand bit [7:0] addr;
  rand bit [7:0] data;
  constraint c_addr { addr != 8'h00; }
  constraint c_data { data inside {[1:100]}; }
endclass

module class_top (
  input logic clk,
  output logic [7:0] out
);
  packet p;
  integer seed = 42;

  initial begin
    p = new();
    // NOTE differential-hygiene: with-block deterministik PENUH (addr==AA;
    // out=p.addr → ASRT_OUT=<aa> stabil antar run mivon, untuk oracle
    // internal O1/O4/O5). BUKAN vs iverilog: iverilog tak dukung deklarasi
    // `constraint` ("sorry: Constraint declarations not supported") → selalu
    // RefUnavailable. `out = p.addr ^ p.data` lama nondeterministik
    // (PRNG beda tiap tool). TANPA $finish di DUT ($finish DUT mematikan sim
    // sebelum TB sampling — pola sama dgn seed 21_fork; TB yang $finish).
    assert (p.randomize() with { addr == 8'hAA; });
    out = p.addr;
  end
endmodule