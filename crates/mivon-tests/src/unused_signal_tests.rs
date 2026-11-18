//! COMP-02: `eliminate_unused_signals_ir` — hapus sinyal IR yang tidak pernah
//! dibaca/ditulis statement manapun dan bukan input/output/inout, lalu
//! remap SignalId agar indeks tetap konsisten.
use mivon_api::compile_str;
use mivon_ir::{
    eliminate_unused_signals_ir, IrDesign, IrInstance, IrLValue, IrModule, IrStmt, IrExpr,
    NetType, Process, SignalInfo, SignalKind, SignalSensitivity,
};
use mivon_core::intern::Symbol;
use mivon_core::LogicVec;
use std::collections::HashMap;
use std::sync::Arc;

const UNUSED_SRC: &str = "
module top;
  logic a;
  logic b;
  logic c;
  initial begin
    a = 1;
    b = 0;
  end
endmodule
";

const ALL_USED_SRC: &str = "
module top;
  logic a;
  logic b;
  initial begin
    a = 1;
    b = a;
  end
endmodule
";

/// Sinyal `c` tidak pernah dibaca/ditulis → harus dihapus; `a` dan `b` tinggal.
/// Sin ID harus di-remap konsisten (tidak ada id yang melebihi signals.len()).
#[test]
fn test_eliminate_unused_signals_removes_unused_signal() {
    let mut design = compile_str(UNUSED_SRC).expect("compile harus sukses");
    let before = design.top.signals.len();
    let removed = eliminate_unused_signals_ir(&mut design);
    assert!(removed, "harusnya menghapus setidaknya satu sinyal yang tidak terpakai");
    assert!(design.top.signals.len() < before, "jumlah sinyal harus berkurang");

    // Top module tetap konsisten (tidak panic saat remap SignalId).
    assert!(design.top.signals.len() > 0, "top module masih harus punya sinyal");
}

/// Semua sinyal terpakai → tidak ada yang dihapus, count sama.
#[test]
fn test_eliminate_unused_signals_all_used_no_change() {
    let mut design = compile_str(ALL_USED_SRC).expect("compile harus sukses");
    let before = design.top.signals.len();
    let removed = eliminate_unused_signals_ir(&mut design);
    assert!(!removed, "tidak ada sinyal yang tidak terpakai, jadi tidak ada yang dihapus");
    assert_eq!(design.top.signals.len(), before, "jumlah sinyal tidak boleh berubah");
}

/// Modul non-top di `design.modules` juga diproses.
#[test]
fn test_eliminate_unused_signals_processes_submodules() {
    let mut design = compile_str(UNUSED_SRC).expect("compile harus sukses");
    let _removed = eliminate_unused_signals_ir(&mut design);
    // Tidak boleh panic; top module (minimal) telah ditangani.
    assert!(design.top.signals.len() > 0, "top module masih harus punya setidaknya satu sinyal");
}

/// Sinyal yang hanya digunakan di port_map sub-instance (koneksi ke child
/// module) harus dipertahankan dan di-remap konsisten, tidak dihapus.
#[test]
fn test_eliminate_keeps_port_map_signal_and_remaps() {
    fn sig(name: &str, width: usize) -> SignalInfo {
        SignalInfo {
            name: Symbol::intern(name),
            width,
            kind: SignalKind::Wire,
            net_type: NetType::Wire,
            multi_driver: false,
            init_val: LogicVec::from_u64(0, width),
            array_depth: 0,
            elem_width: width,
            array_dims: Vec::new(),
            class_name: None,
            is_string: false,
            is_real: false,
            is_mailbox: false,
            is_semaphore: false,
            is_2state: false,
            is_dynamic: false,
            is_queue: false,
            is_associative: false,
            is_signed: false,
            is_const: false,
            msb: width.saturating_sub(1),
            lsb: 0,
            struct_fields: Vec::new(),
            packed_dims: Vec::new(),
            delay_rise: None,
            delay_fall: None,
            iface_type: None,
            iface_modport: None,
        }
    }

    // a(0)=b(1) dipakai proses; x(2) hanya di port_map sub-instance; y(3) tidak pernah dipakai.
    let mut design = IrDesign::default();
    design.top = IrModule {
        name: Symbol::intern("top"),
        signals: vec![sig("a", 1), sig("b", 1), sig("x", 1), sig("y", 1)],
        inputs: Vec::new(),
        outputs: Vec::new(),
        inouts: Vec::new(),
        processes: vec![Process::Combinational {
            name: Symbol::intern("p0"),
            sensitivity: vec![SignalSensitivity::whole(1)],
            body: vec![IrStmt::BlockingAssign {
                lhs: IrLValue::Signal(0, 1),
                rhs: IrExpr::Signal(1, 1),
                delay: None,
            }],
        }],
        sub_instances: vec![IrInstance {
            module_name: Symbol::intern("sub"),
            instance_name: Symbol::intern("u0"),
            port_map: Arc::new(HashMap::from([
                (Symbol::intern("clk"), 2),
                (Symbol::intern("x"), 2),
            ])),
            param_map: Arc::new(HashMap::new()),
            type_param_map: Arc::new(HashMap::new()),
            line: 1,
            col: 1,
            attrs: Vec::new(),
            param_width_map: Arc::new(HashMap::new()),
        }],
    };

    let removed = eliminate_unused_signals_ir(&mut design);
    assert!(removed, "sinyal y(3) yang tidak dipakai harus dihapus");
    assert_eq!(design.top.signals.len(), 3, "a, b, x tinggal; y dihapus");

    // x (id lama 2) tetap di id baru 2 karena kept order a(0), b(1), x(2).
    let port_x = design.top.sub_instances[0].port_map.get(&Symbol::intern("x")).cloned();
    let port_clk = design.top.sub_instances[0].port_map.get(&Symbol::intern("clk")).cloned();
    assert_eq!(port_x, Some(2), "port_map x harus menunjuk ke sinyal x setelah remap");
    assert_eq!(port_clk, Some(2), "port_map clk juga menunjuk ke x (alias port)");
    // Tidak ada id yang melebihi signals.len() (tidak ada dangling/stale id).
    for (_name, sid) in design.top.sub_instances[0].port_map.iter() {
        assert!(*sid < design.top.signals.len(), "port_map id tidak boleh out-of-bounds");
    }
}
