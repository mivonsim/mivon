use super::*;

// REGRESI LANG-43: write pada part-select referensi hierarkis.
//
// Sebelumnya `u.data[3:0] = ...` jatuh ke fallback "nested range select
// lvalue tidak di-resolve statis" (ObjectField no-op) dan
// `u.data[base +: 4] = ...` memicu warning "dynamic part-select pada lvalue
// hierarkis — write diabaikan" sehingga SELURUH sinyal tertimpa nilai RHS.
// Sekarang keduanya didekomposisi ke concat HierRefIndex (MSB-first) dan
// di-resolve engine dari SignalInfo.

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
fn hier_part_select_constant_range_write() {
    let source = r#"
module child;
    logic [15:0] data;
endmodule

module top;
    child u();
    initial begin
        u.data[3:0] = 4'hA;
        u.data[15:8] = 8'h5;
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get(&sigs, "u.data").to_u64(),
        0x050A,
        "part-select hierarkis konstan: [15:8]=5, [3:0]=A"
    );
}

#[test]
fn hier_part_select_dynamic_base_write() {
    let source = r#"
module child;
    logic [15:0] data;
endmodule

module top;
    child u();
    int base;
    initial begin
        base = 4;
        u.data[base +: 4] = 4'hF;
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get(&sigs, "u.data").to_u64(),
        0x00F0,
        "part-select hierarkis dinamis: base=4 → bit 7:4"
    );
}

#[test]
fn hier_bit_select_write() {
    let source = r#"
module child;
    logic [7:0] data;
endmodule

module top;
    child u();
    int i;
    initial begin
        i = 5;
        u.data[i] = 1'b1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get(&sigs, "u.data").to_u64(),
        0x20,
        "bit-select hierarkis dinamis: i=5 → bit 5"
    );
}
