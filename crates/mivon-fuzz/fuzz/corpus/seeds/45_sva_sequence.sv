// Seed 45: SVA sequence/property lanjut (##, |=>, repetition [1:3])
module req_ack_sva #(
  parameter int unsigned TIMEOUT = 16
) (
  input logic clk,
  input logic rst_n,
  input logic req,
  input logic ack
);
  // request harus di-ack dalam TIMEOUT cycle
  sequence s_req_ack;
    req ##[1:TIMEOUT] ack;
  endsequence

  sequence s_no_spurious;
    ack |-> $past(req);
  endsequence

  property p_req_followed_by_ack;
    @(posedge clk) disable iff (!rst_n)
    req |-> s_req_ack;
  endproperty

  property p_ack_only_after_req;
    @(posedge clk) disable iff (!rst_n)
    s_no_spurious;
  endproperty

  a_req_ack: assert property (p_req_followed_by_ack)
    else $error("request never acknowledged");
  a_no_spurious: assert property (p_ack_only_after_req)
    else $error("spurious ack");

  c_req: cover property (@(posedge clk) req ##[1:3] ack);
endmodule
