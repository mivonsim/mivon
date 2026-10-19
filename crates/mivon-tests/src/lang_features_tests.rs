use super::*;

// REGRESI: fitur yang sebelumnya ditandai Partial kini Full.
// Mengamankan dukungan nyata untuk package import, virtual method,
// static/automatic qualifier, dan final block.

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
fn language_features_full_support() {
    let source = r#"
package p;
  function int add(int a, int b); return a + b; endfunction
endpackage

class base;
  virtual function int f(); return 1; endfunction
endclass
class ext extends base;
  function int f(); return 2; endfunction
endclass

module tb;
  import p::add;
  base b; ext e;
  int out_add;
  int out_vf;
  int out_cf;
  function int cnt();
    automatic int x = 0;
    static int s = 0;
    s = s + 1;
    return s;
  endfunction
  int out_cnt;
  int out_fin;
  final begin
    out_fin = 1;
  end
  initial begin
    b = new(); e = new();
    out_add = add(2, 3);
    out_vf = e.f();
    out_cf = b.f();
    out_cnt = cnt();
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim(source, 10);
    assert_eq!(get(&sigs, "out_add").to_u64(), 5, "package import function");
    assert_eq!(get(&sigs, "out_vf").to_u64(), 2, "virtual method override");
    assert_eq!(get(&sigs, "out_cf").to_u64(), 1, "base virtual method");
    assert_eq!(get(&sigs, "out_cnt").to_u64(), 1, "static/automatic qualifier");
    assert_eq!(get(&sigs, "out_fin").to_u64(), 1, "final block runs");
}
