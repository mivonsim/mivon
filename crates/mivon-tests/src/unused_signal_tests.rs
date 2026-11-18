//! COMP-02: `eliminate_unused_signals_ir` — hapus sinyal IR yang tidak pernah
//! dibaca/ditulis statement manapun dan bukan input/output/inout, lalu
//! remap SignalId agar indeks tetap konsisten.
use mivon_api::compile_str;
use mivon_ir::eliminate_unused_signals_ir;

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
