use super::*;

// REGRESI LANG-44: akses field melalui VARIABEL virtual interface
// (`vif.data = x`) — baik variabel module maupun field class di dalam
// method class. Sebelumnya jalur AST menulis ke field objek (ObjId bisa
// bertabrakan dengan SignalId binding) sehingga instance interface tidak
// pernah menerima nilai.

fn sim(source: &str, max_time: u64) -> Vec<(String, mivon_ir::LogicVec)> {
    simulate_signals(source, max_time).unwrap_or_else(|e| panic!("sim gagal: {e}\n{source}"))
}

fn get(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> mivon_ir::LogicVec {
    sigs.iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal tidak ada: {name}"))
        .1
        .clone()
}

#[test]
fn vif_write_module_scope() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
endinterface

module tb;
    bus_if b();
    virtual bus_if vif;
    initial begin
        vif = b;
        vif.data = 8'h5A;
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(get(&sigs, "b.data").to_u64(), 0x5A);
}

#[test]
fn vif_write_in_class_method() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
endinterface

class drv;
    virtual bus_if vif;
    function void set_d(input logic [7:0] d);
        vif.data = d;
    endfunction
endclass

module tb;
    bus_if b();
    drv d;
    initial begin
        d = new();
        d.vif = b;
        d.set_d(8'h5A);
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get(&sigs, "b.data").to_u64(),
        0x5A,
        "write lewat field class virtual interface harus sampai ke instance"
    );
}

#[test]
fn vif_range_select_write_in_class_method() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
endinterface

class drv;
    virtual bus_if vif;
    function void poke();
        vif.data[3:0] = 4'hA;
    endfunction
endclass

module tb;
    bus_if b();
    drv d;
    initial begin
        d = new();
        d.vif = b;
        b.data = 8'hF0;
        d.poke();
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get(&sigs, "b.data").to_u64(),
        0xFA,
        "part-select pada vif field: nibble bawah diganti, atas dipertahankan"
    );
}

#[test]
fn vif_binding_inside_task() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
endinterface

module tb;
    bus_if b();
    virtual bus_if vif;
    task automatic bind_it();
        vif = b;
    endtask
    initial begin
        bind_it();
        vif.data = 8'h3C;
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get(&sigs, "b.data").to_u64(),
        0x3C,
        "binding virtual interface di body task harus menghasilkan handle instance"
    );
}
