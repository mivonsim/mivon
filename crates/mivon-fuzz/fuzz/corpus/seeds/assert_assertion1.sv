// Seed assert_assertion1: Property + sequence + assume + cover
// Bug area: assertion enflag, property timing, constraint logic

module test_assert (
    input bit clk,
    input bit rst_n
);

property reset_deassertion;
    @(posedge clk) !rst_n |=> (rst_n == 1);
endproperty

assume property assume_rst { ##5 (rst_n == 1); } else { #0 rst_n == 0; }

sequence reset_ok;
    (rst_n == 0) ##1 (rst_n == 1);
endsequence

initial begin
    // property instantiation via cover
    cover property {
        reset_deassertion rst_deassert_inst();
        assume_rst assume_inst();
        reset_ok seq_inst();
    }
end
endmodule