use super::*;

// REGRESI LANG-45: koneksi port instance interface (`bus_if b(clk)`) dan
// akses field melalui port interface (`bus.data`, `bus.clk`) — termasuk
// modport. Sebelumnya port interface tidak di-wire (target_module tidak
// mencakup interface) sehingga clock/field tidak tersambung.

fn sim(source: &str, max_time: u64) -> Vec<(String, mivon_ir::LogicVec)> {
    simulate_signals(source, max_time)
        .unwrap_or_else(|e| panic!("sim gagal: {e}\n{source}"))
}

fn get(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> mivon_ir::LogicVec {
    sigs.iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal tidak ada: {name}"))
        .1
        .clone()
}

#[test]
fn iface_port_clock_and_field_write() {
    let source = r#"
interface bus_if(input logic clk);
    logic [7:0] data;
endinterface

module drv(bus_if bus);
    always_ff @(posedge bus.clk) bus.data <= 8'h33;
endmodule

module tb;
    logic clk = 0;
    bus_if b(clk);
    drv d(b);
    always #5 clk = ~clk;
    initial begin
        #32 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 40);
    assert_eq!(get(&sigs, "b.data").to_u64(), 0x33, "field via interface port terhubung ke instance");
}

#[test]
fn iface_modport_master_write() {
    let source = r#"
interface bus_if(input logic clk);
    logic [7:0] data;
    logic valid;
    modport master(output data, input clk);
    modport slave(input data, input clk);
endinterface

module drv(bus_if.master bus);
    always_ff @(posedge bus.clk) bus.data <= 8'hA5;
endmodule

module tb;
    logic clk = 0;
    bus_if b(clk);
    drv d(b);
    always #5 clk = ~clk;
    initial begin
        #32 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 40);
    assert_eq!(get(&sigs, "b.data").to_u64(), 0xA5, "modport master write sampai ke instance");
}

#[test]
fn iface_modport_handshake() {
    let source = r#"
interface bus_if(input logic clk);
    logic [7:0] data;
    logic valid;
    modport master(output data, output valid, input clk);
    modport slave(input data, input valid, input clk);
endinterface

module master_m(bus_if.master bus);
    always_ff @(posedge bus.clk) begin
        bus.data <= 8'hA5;
        bus.valid <= 1'b1;
    end
endmodule

module slave_m(bus_if.slave bus, output logic [7:0] seen);
    always_ff @(posedge bus.clk) begin
        if (bus.valid) seen <= bus.data;
    end
endmodule

module tb;
    logic clk = 0;
    logic [7:0] seen;
    bus_if b(clk);
    master_m m(b);
    slave_m s(b, seen);
    always #5 clk = ~clk;
    initial begin
        #32 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 40);
    assert_eq!(
        get(&sigs, "seen").to_u64(),
        0xA5,
        "handshake master→slave lewat interface yang sama harus menangkap data"
    );
}
