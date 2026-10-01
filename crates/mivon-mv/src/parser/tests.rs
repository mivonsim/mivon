//! Parser — unit tests (dipindah dari parser.rs saat pemecahan struktur).
//! Di-include oleh `parser/mod.rs` (`#[cfg(test)] mod tests;`).

use super::*;

#[test]
fn parse_counter() {
    let src = r#"
module counter #(WIDTH = 8) {
    in  clk, rst_n : bit
    in  enable     : bit
    out count      : logic[WIDTH-1:0]

    seq(clk, rst_n) {
        if (!rst_n) {
            count <= '0
        } else if (enable) {
            count <= count + 1
        }
    }
}
"#;
    let f = parse(src).expect("parse counter");
    assert_eq!(f.modules.len(), 1);
    let m = &f.modules[0];
    assert_eq!(m.name, "counter");
    assert_eq!(m.params.len(), 1);
    assert_eq!(m.items.len(), 4); // 3 port + 1 blok seq
    match &m.items[0] {
        MItem::Port(p) => {
            assert_eq!(p.dir, Dir::In);
            assert_eq!(p.names, vec!["clk", "rst_n"]);
        }
        _ => panic!("harus port"),
    }
}

#[test]
fn parse_precedence_power_vs_unary() {
    // `**` terikat lebih erat dari unary minus: `-a ** b` = `-(a ** b)` (SV).
    let src = "module m { in a, b : bit\n out y : bit\n comb { y = -a ** b } }";
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    let mut found = false;
    for item in &m.items {
        if let MItem::Comb(body) = item {
            let stmts: &[Stmt] = match body {
                Stmt::Block(s) => s.as_slice(),
                other => std::slice::from_ref(other),
            };
            for s in stmts {
                if let Stmt::Assign { rhs, .. } = s {
                    // rhs harus Unary("-", Binary("**", a, b))
                    match rhs {
                        Expr::Unary(op, inner) if op.as_str() == "-" => {
                            if let Expr::Binary(bop, l, r) = inner.as_ref() {
                                assert_eq!(bop, "**");
                                assert!(matches!(l.as_ref(), Expr::Ident(..)));
                                assert!(matches!(r.as_ref(), Expr::Ident(..)));
                                found = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    assert!(found, "`-a ** b` harus di-parse sebagai `-(a ** b)`");
}

#[test]
fn parse_package_and_types() {
    let src = r#"
package counter_pkg {
    type Addr = logic[15:0]
    packed struct Packet {
        valid : bit,
        addr  : Addr
    }
    enum(3) Color { RED = 0, GREEN = 2, BLUE = 4 }
}
"#;
    let f = parse(src).unwrap();
    assert_eq!(f.packages.len(), 1);
    let pkg = &f.packages[0];
    assert_eq!(pkg.typedefs.len(), 3);
    assert!(matches!(pkg.typedefs[0], Typedef::Alias { name: ref n, .. } if n == "Addr"));
    assert!(matches!(
        pkg.typedefs[1],
        Typedef::Struct { packed: true, .. }
    ));
    assert!(matches!(pkg.typedefs[2], Typedef::Enum { name: ref n, .. } if n == "Color"));
}

#[test]
fn parse_traffic() {
    let src = r#"
module traffic #(GREEN_T = 30) {
    use traffic_pkg::*
    in  clk, rst_n : bit
    out state      : State
    out red, green, yellow : bit

    reg state : State
    reg timer : uint[8]

    seq(clk, rst_n) {
        if (!rst_n) {
            state <= RED
            timer <= 0
        } else {
            case (state) {
                RED: {
                    if (timer == GREEN_T) {
                        state <= GREEN
                        timer <= 0
                    } else {
                        timer <= timer + 1
                    }
                }
                default: {
                    state <= RED
                }
            }
        }
    }

    comb {
        red    = (state == RED)
        green  = (state == GREEN)
        yellow = (state == YELLOW)
    }
}
"#;
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    assert_eq!(m.name, "traffic");
    let seq_count = m
        .items
        .iter()
        .filter(|i| matches!(i, MItem::Seq(..)))
        .count();
    assert_eq!(seq_count, 1);
    let comb_count = m
        .items
        .iter()
        .filter(|i| matches!(i, MItem::Comb(..)))
        .count();
    assert_eq!(comb_count, 1);
}

#[test]
fn parse_instance() {
    let src = r#"
module top {
    in clk : bit
    inst counter_small u_small (.clk, .rst_n, .count(count[3:0]))
    inst alu u_alu (a, b, op, y)
    inst flipflop u_ff[8] (.clk, .d(d_in), .q(q_out))
}
"#;
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    let insts: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            MItem::Inst {
                name, dims, conns, ..
            } => Some((name.as_str(), dims.is_some(), conns.len())),
            _ => None,
        })
        .collect();
    assert_eq!(insts.len(), 3);
    assert_eq!(insts[0], ("u_small", false, 3));
    assert_eq!(insts[1], ("u_alu", false, 4));
    assert_eq!(insts[2], ("u_ff", true, 3));
}

#[test]
fn parse_func_task() {
    let src = r#"
func clog2(x : int) -> int {
    var r : int = 0
    var n : int = x - 1
    while (n > 0) {
        r = r + 1
        n = n >> 1
    }
    return r
}

task send(data : logic[7:0], out ok : bit) {
    #10
    ok = 1
}
"#;
    let f = parse(src).unwrap();
    assert_eq!(f.funcs.len(), 1);
    assert_eq!(f.tasks.len(), 1);
    let func = &f.funcs[0];
    assert_eq!(func.name, "clog2");
    assert!(func.ret.is_some());
    assert_eq!(func.args.len(), 1);
    assert_eq!(func.body.len(), 4); // var, var, while, return
}

#[test]
fn parse_expr_precedence() {
    let src = r#"
module m {
    out y : logic[7:0]
    comb {
        y = a + b * c - d / e
        y = (a || b) && (c & d) | (e ^ f)
        y = a ? b : c
        y = {a, b, c}
        y = {4{a}}
        y = x[3:0]
        y = pkt.valid
        y = $clog2(DEPTH)
        y = 8'hFF
    }
}
"#;
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    let comb = m.items.iter().find_map(|i| match i {
        MItem::Comb(s) => Some(s),
        _ => None,
    });
    let comb = comb.expect("comb");
    let stmts = match comb {
        Stmt::Block(v) => v,
        _ => panic!("comb body harus block"),
    };
    assert_eq!(stmts.len(), 9);
}

#[test]
fn parse_assert_property_raw() {
    // Body `assert property (...)` dipertahankan RAW (operator SVA `|->`
    // dan `##` bukan token .mv) — emisi 1:1.
    let src = r#"
module m {
    in clk, enable : bit
    in count       : logic[7:0]
    initial {
        assert property (@(posedge clk) enable |-> count == $past(count) + 1)
    }
}
"#;
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    // module punya 2 port + 1 initial = 3 item; cari initial via iter
    let init = m
        .items
        .iter()
        .find_map(|i| match i {
            MItem::Initial(body) => Some(body),
            _ => None,
        })
        .expect("harus ada initial");
    let stmts: &[Stmt] = match init {
        Stmt::Block(s) => s.as_slice(),
        other => std::slice::from_ref(other),
    };
    let mut found = false;
    for s in stmts {
        if let Stmt::AssertProperty(raw) = s {
            assert_eq!(
                raw, "(@(posedge clk) enable |-> count == $past(count) + 1)",
                "raw harus persis (termasuk parens): {raw}"
            );
            found = true;
        }
    }
    assert!(found, "harus ada Stmt::AssertProperty");
}

#[test]
fn parse_program_block() {
    // Program block (MIVON-HDL.md §7.3)
    let src = r#"
program test_runner {
    in clk : bit
    initial {
        run_test("my_test")
    }
}
"#;
    let f = parse(src).unwrap();
    assert_eq!(f.programs.len(), 1);
    let p = &f.programs[0];
    assert_eq!(p.name, "test_runner");
    assert_eq!(p.items.len(), 2); // port + initial
    assert!(matches!(p.items[0], MItem::Port(..)));
    assert!(matches!(p.items[1], MItem::Initial(..)));
}

#[test]
fn parse_class_uvm() {
    // Class + UVM subset (MIVON-HDL.md §8)
    let src = r#"
class my_test extends uvm_test {
    field count     : uint
    rand field seed : uint
    constraint c { seed > 10, seed < 200 }
    func new(name : string) {
        super.new(name)
    }
    func build_phase() {
        uvm_config_db::set(this, "*.agent", "count", count)
    }
    task run_phase() {
        var seqr : uvm_sequencer
        seqr.start_item(item)
        seqr.finish_item(item)
        #100
    }
}
"#;
    let f = parse(src).unwrap();
    assert_eq!(f.classes.len(), 1);
    let c = &f.classes[0];
    assert_eq!(c.name, "my_test");
    assert_eq!(c.extends.as_deref(), Some("uvm_test"));
    assert_eq!(c.fields.len(), 2);
    assert_eq!(c.fields[0], ("count".into(), MvType::Uint, false));
    assert_eq!(c.fields[1], ("seed".into(), MvType::Uint, true));
    assert_eq!(c.constraints.len(), 1);
    assert_eq!(c.funcs.len(), 2);
    assert_eq!(c.tasks.len(), 1);
    // method call + delay-only di body task
    let t = &c.tasks[0];
    assert!(t.body.iter().any(|s| matches!(
        s,
        Stmt::ExprStmt(Expr::MethodCall {
            method,
            obj: _,
            args: _
        }) if method == "start_item"
    )));
    assert!(t.body.iter().any(|s| matches!(
        s,
        Stmt::Delay {
            amt: Expr::Int(100),
            body
        } if matches!(body.as_ref(), Stmt::Block(v) if v.is_empty())
    )));
}

#[test]
fn parse_class_field_edges() {
    // multi-nama field, constraint kosong
    let src = r#"
class c {
    field a, b : uint
    rand field r1, r2 : bit
    constraint empty { }
}
"#;
    let f = parse(src).unwrap();
    let c = &f.classes[0];
    assert_eq!(c.fields.len(), 4);
    assert_eq!(c.fields[0], ("a".into(), MvType::Uint, false));
    assert_eq!(c.fields[1], ("b".into(), MvType::Uint, false));
    assert_eq!(c.fields[2], ("r1".into(), MvType::Bit, true));
    assert_eq!(c.fields[3], ("r2".into(), MvType::Bit, true));
    assert_eq!(c.constraints.len(), 1);
    assert!(c.constraints[0].1.is_empty());
}

#[test]
fn parse_delay_only_in_initial() {
    // delay-only `#100` di akhir blok module (bukan hanya class task)
    let src = "module tb {\n in clk : bit\n initial {\n clk = 0\n #100\n } }\n";
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    if let MItem::Initial(body) = &m.items[1] {
        let stmts: &[Stmt] = match body {
            Stmt::Block(s) => s.as_slice(),
            other => std::slice::from_ref(other),
        };
        assert!(stmts.iter().any(|s| matches!(
            s,
            Stmt::Delay {
                amt: Expr::Int(100),
                body
            } if matches!(body.as_ref(), Stmt::Block(v) if v.is_empty())
        )));
    } else {
        panic!("harus initial");
    }
}

#[test]
fn parse_class_plain() {
    // Class tanpa extends — reuse parse_class
    let src = r#"
class counter_model {
    field value : uint
    func new() {
        value = 0
    }
    task tick() {
        value = value + 1
    }
}
"#;
    let f = parse(src).unwrap();
    assert_eq!(f.classes.len(), 1);
    let c = &f.classes[0];
    assert_eq!(c.name, "counter_model");
    assert!(c.extends.is_none());
    assert_eq!(c.fields.len(), 1);
    assert_eq!(c.funcs.len(), 1);
    assert_eq!(c.tasks.len(), 1);
}

#[test]
fn parse_constraint_advanced() {
    // F12: constraint lanjutan — inside/dist/if-else/solve dalam satu blok
    let src = r#"
class item extends uvm_sequence_item {
    rand field mode : uint[2]
    rand field addr : uint[8]
    rand field data : uint[8]
    field limit : uint[8]
    constraint c_adv {
        addr inside {[1:10], 20, 30},
        data dist { 0 := 1, [1:5] :/ 9 },
        if (mode == 1) { addr > 5 } else { addr < 100 },
        solve addr before data
    }
}
"#;
    let f = parse(src).unwrap();
    assert_eq!(f.classes.len(), 1);
    let c = &f.classes[0];
    assert_eq!(c.constraints.len(), 1);
    let (name, items) = &c.constraints[0];
    assert_eq!(name, "c_adv");
    assert_eq!(items.len(), 4);
    // 1) inside (dengan range + nilai tunggal, urutan dijaga 1:1)
    assert!(matches!(
        items[0],
        ConstraintItem::Expr(Expr::Inside { .. })
    ));
    if let ConstraintItem::Expr(Expr::Inside { items: ins, .. }) = &items[0] {
        assert_eq!(ins.len(), 3);
        assert!(matches!(ins[0], InsideItem::Range(_, _))); // [1:10]
        assert!(matches!(ins[1], InsideItem::Value(_))); // 20
        assert!(matches!(ins[2], InsideItem::Value(_))); // 30
    } else {
        panic!("item 0 harus inside");
    }
    // 2) dist (nilai := dan range :/)
    assert!(matches!(items[1], ConstraintItem::Expr(Expr::Dist { .. })));
    if let ConstraintItem::Expr(Expr::Dist { items: d, .. }) = &items[1] {
        assert_eq!(d.len(), 2);
        assert!(d[0].exact && d[0].range.is_none()); // 0 := 1
        assert!(!d[1].exact && d[1].range.is_some()); // [1:5] :/ 9
    } else {
        panic!("item 1 harus dist");
    }
    // 3) if/else
    assert!(matches!(items[2], ConstraintItem::If { .. }));
    if let ConstraintItem::If { then, els, .. } = &items[2] {
        assert_eq!(then.len(), 1);
        assert_eq!(els.len(), 1);
    } else {
        panic!("item 2 harus if");
    }
    // 4) solve
    assert!(matches!(&items[3], ConstraintItem::Solve { var, .. } if var == "addr"));
}

#[test]
fn parse_generate() {
    let src = r#"
module shiftreg #(N : int = 8) {
    in clk : bit
    in d   : bit
    out q  : logic[N-1:0]
    seq(clk) {
        q[0] <= d
    }
    for i in 1..N {
        seq(clk) {
            q[i] <= q[i-1]
        }
    }
}
"#;
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    assert!(m.items.iter().any(|i| matches!(i, MItem::GenFor { .. })));
}

#[test]
fn parse_wait_fork_and_disable_fork() {
    // F45: `wait fork;` + `disable fork;` / `disable <label>;`
    let src = r#"
module m {
    sig a : logic[7:0]
    initial {
        fork {
            #10
            a = 1
        } {
            #5
            a = 2
        } join_none
        wait fork
        disable fork
        disable my_block
    }
}
"#;
    let f = parse(src).expect("parse wait/disable fork");
    let m = &f.modules[0];
    let has_wait = m.items.iter().any(|i| match i {
        MItem::Initial(Stmt::Block(s)) => s.iter().any(|x| matches!(x, Stmt::WaitFork)),
        _ => false,
    });
    assert!(has_wait, "wait fork harus ter-parse");
    let has_dis = m.items.iter().any(|i| match i {
        MItem::Initial(Stmt::Block(s)) => s.iter().any(|x| matches!(x, Stmt::Disable { .. })),
        _ => false,
    });
    assert!(has_dis, "disable harus ter-parse");
}

#[test]
fn parse_force_release() {
    // F46: `force lhs = rhs;` + `release target;`
    let src = r#"
module m {
    sig a : logic[7:0]
    sig arr : logic[8][4]
    initial {
        force a = 8'd99
        force arr[1] = 8'd7
        release a
    }
}
"#;
    let f = parse(src).expect("parse force/release");
    let m = &f.modules[0];
    let (mut has_force, mut has_rel) = (false, false);
    for i in &m.items {
        if let MItem::Initial(Stmt::Block(s)) = i {
            for x in s {
                if matches!(x, Stmt::Force { .. }) {
                    has_force = true;
                }
                if matches!(x, Stmt::Release { .. }) {
                    has_rel = true;
                }
            }
        }
    }
    assert!(has_force, "force harus ter-parse");
    assert!(has_rel, "release harus ter-parse");
}

#[test]
fn parse_named_block() {
    // F47: `label : { stmt* }` — blok bernama (target `disable <label>`).
    let src = r#"
module m {
    sig a : logic[7:0]
    initial {
        worker : {
            #10
            a = 1
        }
        disable worker
    }
}
"#;
    let f = parse(src).expect("parse named block");
    let m = &f.modules[0];
    let has_named = m.items.iter().any(|i| match i {
        MItem::Initial(Stmt::Block(s)) => s.iter().any(|x| matches!(x, Stmt::NamedBlock { .. })),
        _ => false,
    });
    assert!(has_named, "named block harus ter-parse");
}
