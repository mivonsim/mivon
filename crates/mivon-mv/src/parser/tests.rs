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
    // dan `##` bukan token .mv) — emisi 1:1. Diparse sebagai MODULE ITEM
    // (concurrent assertion, LRM 1800 §14).
    let src = r#"
module m {
    in clk, enable : bit
    in count       : logic[7:0]
    assert property (@(posedge clk) enable |-> count == $past(count) + 1)
}
"#;
    let f = parse(src).unwrap();
    let m = &f.modules[0];
    let raw = m
        .items
        .iter()
        .find_map(|i| match i {
            MItem::AssertProperty(raw) => Some(raw.as_str()),
            _ => None,
        })
        .expect("harus ada MItem::AssertProperty");
    assert_eq!(
        raw, "(@(posedge clk) enable |-> count == $past(count) + 1)",
        "raw harus persis (termasuk parens): {raw}"
    );
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
        Stmt::ExprStmt(Expr::MethodCall { method, .. }) if method == "start_item"
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

#[test]
fn parse_dsl_verbs_are_aliases_of_sv_forms() {
    // Kata kerja DSL (bukan sintaks SV) → AST yang SAMA dengan bentuk SV,
    // jadi codegen & check tidak perlu tahu asal tulisannya.
    //   emit ev ≡ -> ev ; override x = v ≡ force x = v ;
    //   restore x ≡ release x ; await all ≡ wait fork ; stop fork ≡ disable fork
    let dsl = parse(
        r#"
module tb {
    sig clk : bit
    sig flag : bit
    sig w : logic[7:0]
    initial {
        emit flag
        override w = 8'd99
        restore w
        await all
        stop fork
    }
}
"#,
    )
    .unwrap();
    let sv = parse(
        r#"
module tb {
    sig clk : bit
    sig flag : bit
    sig w : logic[7:0]
    initial {
        -> flag
        force w = 8'd99
        release w
        wait fork
        disable fork
    }
}
"#,
    )
    .unwrap();
    assert_eq!(
        stmt_kinds(&first_initial(&dsl)),
        stmt_kinds(&first_initial(&sv)),
        "bentuk DSL harus menghasilkan statement yang sama dengan bentuk SV"
    );
}

#[test]
fn parse_await_cond_is_alias_of_wait_cond() {
    let dsl = parse("module m { sig c : bit\n initial { await (c) { } } }").unwrap();
    let sv = parse("module m { sig c : bit\n initial { wait (c) { } } }").unwrap();
    assert_eq!(
        stmt_kinds(&first_initial(&dsl)),
        stmt_kinds(&first_initial(&sv))
    );
}

#[test]
fn parse_inst_params_before_or_after_instance_name() {
    // Kedua urutan diterima; keduanya di-emit `mod #(...) name (...)`.
    let a = parse("module t { in clk : bit\n inst f u #(.D(4)) (.clk(clk)) }").unwrap();
    let b = parse("module t { in clk : bit\n inst f #(.D(4)) u (.clk(clk)) }").unwrap();
    assert_eq!(a.modules[0].items, b.modules[0].items);

    // Ditolak kalau ditulis dua kali.
    let err =
        parse("module t { in clk : bit\n inst f #(.D(4)) u #(.D(8)) (.clk(clk)) }").unwrap_err();
    assert!(err.msg.contains("dua kali"), "msg: {}", err.msg);
}

#[test]
fn parse_assert_property_only_at_module_level() {
    // LRM 1800 §14: `assert property` = concurrent assertion = module item.
    let src = "module m {\n in clk : bit\n assert property (@(posedge clk) 1'b1)\n }\n";
    let f = parse(src).unwrap();
    assert!(
        f.modules[0]
            .items
            .iter()
            .any(|i| matches!(i, MItem::AssertProperty(_))),
        "harus ada MItem::AssertProperty"
    );
}

#[test]
fn parse_immediate_assert_at_module_level_is_rejected() {
    // Immediate assertion hanya sah di dalam blok prosedural.
    let err = parse("module m {\n in clk : bit\n assert (1'b1) $info(\"x\")\n }\n").unwrap_err();
    assert!(err.msg.contains("concurrent assertion"), "msg: {}", err.msg);
}

#[test]
fn parse_assume_immediate_and_property() {
    // `assume (c)` immediate di blok prosedural + `assume property` RAW.
    let src = "module m {\n sig a : bit\n initial {\n assume (a == 0) $info(\"ok\") else $error(\"bad\")\n }\n assume property (@(posedge clk) a |-> b)\n }\n";
    let f = parse(src).expect("parse assume");
    let stmts = first_initial(&f);
    assert!(
        stmts.iter().any(|s| matches!(s, Stmt::Assume { .. })),
        "harus ada Stmt::Assume: {stmts:?}"
    );
    assert!(
        f.modules[0]
            .items
            .iter()
            .any(|i| matches!(i, MItem::AssumeProperty(_))),
        "harus ada MItem::AssumeProperty"
    );
}

#[test]
fn parse_immediate_assume_at_module_level_is_rejected() {
    // Mirror `assert`: immediate `assume` di level module ditolak.
    let err = parse("module m {\n in clk : bit\n assume (1'b1) $info(\"x\")\n }\n").unwrap_err();
    assert!(err.msg.contains("assume property"), "msg: {}", err.msg);
}

#[test]
fn parse_case_inside_values_ranges_default() {
    // `case (x) inside` — label nilai, rentang `[lo:hi]`, multi-label, default.
    let src = "module m {\n sig x : logic[7:0]\n sig y : bit\n comb {\n case (x) inside {\n 0 : { y = 0 }\n [1:10], 30 : { y = 1 }\n default : { y = 0 }\n }\n }\n}\n";
    let f = parse(src).expect("parse case inside");
    let stmts = first_comb(&f);
    let ci = stmts
        .iter()
        .find_map(|s| match s {
            Stmt::CaseInside { items, default, .. } => Some((items, default)),
            _ => None,
        })
        .expect("harus ada Stmt::CaseInside");
    assert_eq!(ci.0.len(), 2, "2 branch: {ci:?}");
    assert!(matches!(ci.0[0].0[0], InsideItem::Value(_)), "label 0 nilai");
    assert!(matches!(ci.0[1].0[0], InsideItem::Range(_, _)), "label [1:10] rentang");
    assert!(matches!(ci.0[1].0[1], InsideItem::Value(_)), "label 30 nilai");
    assert!(ci.1.is_some(), "default ada");
}

#[test]
fn parse_case_inside_with_qualifier() {
    // Qualifier + inside ortogonal: `priority case (x) inside`.
    let src = "module m {\n sig x : logic[7:0]\n sig y : bit\n comb {\n priority case (x) inside {\n [1:10] : { y = 1 }\n }\n }\n}\n";
    let f = parse(src).expect("parse priority case inside");
    let stmts = first_comb(&f);
    assert!(
        stmts.iter().any(|s| matches!(
            s,
            Stmt::CaseInside {
                qual: Some(_),
                ..
            }
        )),
        "qualifier dipertahankan: {stmts:?}"
    );
}

#[test]
fn parse_case_inside_bracket_without_colon_rejected() {
    // `[` tanpa `:` di posisi label ditolak eksplisit.
    let err = parse("module m {\n sig x : logic[7:0]\n sig y : bit\n comb {\n case (x) inside {\n [5] : { y = 1 }\n }\n }\n}\n").unwrap_err();
    assert!(err.msg.contains("'[lo:hi]'"), "msg: {}", err.msg);
}

#[test]
fn parse_unique_if_keeps_qualifier() {
    // `unique if` / `priority if` / `unique0 if` — qualifier di head.
    for (kw, want) in [("unique", "unique"), ("priority", "priority"), ("unique0", "unique0")] {
        let src = format!("module m {{\n sig a : bit\n sig y : bit\n comb {{\n {kw} if (a) {{ y = 1 }} else {{ y = 0 }}\n }}\n}}\n");
        let f = parse(&src).expect("parse qualified if");
        let stmts = first_comb(&f);
        assert!(
            stmts.iter().any(|s| matches!(s, Stmt::If { qual: Some(q), .. } if q == want)),
            "{kw}: qualifier dipertahankan: {stmts:?}"
        );
    }
}

#[test]
fn parse_qualifier_without_if_or_case_rejected() {
    // Pesan error menyebut `if` sebagai opsi sah.
    let err = parse("module m {\n sig a : bit\n comb {\n unique foo { }\n }\n}\n").unwrap_err();
    assert!(err.msg.contains("'if'"), "msg: {}", err.msg);
}

#[test]
fn parse_cover_immediate_and_property() {
    // `cover (c)` immediate (tanpa `else`) + `cover property` RAW.
    let src = "module m {\n sig a : bit\n initial {\n cover (a == 0) $info(\"ok\")\n }\n cover property (@(posedge clk) a |-> b)\n }\n";
    let f = parse(src).expect("parse cover");
    let stmts = first_initial(&f);
    assert!(
        stmts.iter().any(|s| matches!(s, Stmt::Cover { .. })),
        "harus ada Stmt::Cover: {stmts:?}"
    );
    assert!(
        f.modules[0]
            .items
            .iter()
            .any(|i| matches!(i, MItem::CoverProperty(_))),
        "harus ada MItem::CoverProperty"
    );
}

#[test]
fn parse_cover_with_else_is_rejected() {
    // `cover` tanpa `else` (tak ada cabang gagal).
    let err = parse("module m {\n sig a : bit\n initial {\n cover (a == 0) $info(\"x\") else $error(\"y\")\n }\n }\n").unwrap_err();
    assert!(err.msg.contains("tidak memakai 'else'"), "msg: {}", err.msg);
}

#[test]
fn parse_statement_level_property() {
    // `assert/assume/cover property` di dalam blok prosedural (bukan cuma
    // level module): `property` harus dimakan parser — regresi missing
    // `advance()` yang melempar `diharapkan LParen, ditemukan Ident(property)`.
    let src = "module m {\n sig a : bit\n initial {\n assert property (@(posedge clk) a == 1)\n assume property (@(posedge clk) a == 1)\n cover property (@(posedge clk) a == 1)\n }\n}\n";
    let f = parse(src).expect("parse statement-level property");
    let stmts = first_initial(&f);
    assert!(
        stmts.iter().any(|s| matches!(s, Stmt::AssertProperty(_))),
        "assert property: {stmts:?}"
    );
    assert!(
        stmts.iter().any(|s| matches!(s, Stmt::AssumeProperty(_))),
        "assume property: {stmts:?}"
    );
    assert!(
        stmts.iter().any(|s| matches!(s, Stmt::CoverProperty(_))),
        "cover property: {stmts:?}"
    );
}

#[test]
fn parse_immediate_cover_at_module_level_is_rejected() {
    // Mirror `assert`: immediate `cover` di level module ditolak.
    let err = parse("module m {\n in clk : bit\n cover (1'b1) $info(\"x\")\n }\n").unwrap_err();
    assert!(err.msg.contains("cover property"), "msg: {}", err.msg);
}

/// Statement pertama pada blok `initial` pertama (helper test DSL-verb).
fn first_initial(f: &MvFile) -> Vec<Stmt> {
    f.modules[0]
        .items
        .iter()
        .find_map(|i| match i {
            MItem::Initial(body) => match body {
                Stmt::Block(s) => Some(s.clone()),
                other => Some(vec![other.clone()]),
            },
            _ => None,
        })
        .expect("harus ada blok initial")
}

/// Isi blok `comb` pertama (helper test case-inside).
fn first_comb(f: &MvFile) -> Vec<Stmt> {
    f.modules[0]
        .items
        .iter()
        .find_map(|i| match i {
            MItem::Comb(body) => match body {
                Stmt::Block(s) => Some(s.clone()),
                other => Some(vec![other.clone()]),
            },
            _ => None,
        })
        .expect("harus ada blok comb")
}

/// Nama varian statement (tanpa posisi) — bentuk DSL dan bentuk SV harus
/// menghasilkan urutan varian yang sama walau `line`/`col` berbeda
/// (kata `emit` 4 huruf vs `->` 2 huruf).
fn stmt_kinds(stmts: &[Stmt]) -> Vec<&'static str> {
    fn kind(s: &Stmt) -> &'static str {
        match s {
            Stmt::Block(_) => "Block",
            Stmt::NamedBlock { .. } => "NamedBlock",
            Stmt::Assign { .. } => "Assign",
            Stmt::CompoundAssign { .. } => "CompoundAssign",
            Stmt::IncDec { .. } => "IncDec",
            Stmt::If { .. } => "If",
            Stmt::Case { .. } => "Case",
            Stmt::For { .. } => "For",
            Stmt::While { .. } => "While",
            Stmt::DoWhile { .. } => "DoWhile",
            Stmt::Repeat { .. } => "Repeat",
            Stmt::Forever(_) => "Forever",
            Stmt::Wait { .. } => "Wait",
            Stmt::WaitFork => "WaitFork",
            Stmt::Disable { .. } => "Disable",
            Stmt::Force { .. } => "Force",
            Stmt::Release { .. } => "Release",
            Stmt::EventTrigger(_) => "EventTrigger",
            Stmt::Event { .. } => "Event",
            Stmt::Delay { .. } => "Delay",
            Stmt::VarDecl { .. } => "VarDecl",
            Stmt::Return(..) => "Return",
            Stmt::Break(..) => "Break",
            Stmt::Continue(..) => "Continue",
            Stmt::Fork { .. } => "Fork",
            Stmt::Foreach { .. } => "Foreach",
            Stmt::Assert { .. } => "Assert",
            Stmt::AssertProperty(_) => "AssertProperty",
            Stmt::Assume { .. } => "Assume",
            Stmt::AssumeProperty(_) => "AssumeProperty",
            Stmt::Cover { .. } => "Cover",
            Stmt::CoverProperty(_) => "CoverProperty",
            Stmt::CaseInside { .. } => "CaseInside",
            Stmt::ExprStmt(_) => "ExprStmt",
            Stmt::RawSvh(_) => "RawSvh",
        }
    }
    stmts.iter().map(kind).collect()
}

#[test]
fn parse_wire_and_assign() {
    // F75: `wire w : logic[7:0]` (net) + `assign y = expr` (continuous).
    let src = r#"
module m {
    in a, b : bit
    out y : bit
    wire w1 : bit
    wire w2, w3 : logic[7:0]
    assign w1 = a & b
    assign y = w1
}
"#;
    let f = parse(src).expect("parse wire/assign");
    let m = &f.modules[0];
    let has_wire = m.items.iter().any(|i| matches!(i, MItem::Wire { .. }));
    let has_assign = m.items.iter().any(|i| matches!(i, MItem::Assign { .. }));
    assert!(has_wire, "wire harus ter-parse");
    assert!(has_assign, "assign harus ter-parse");
    // multi-nama wire
    let wires: Vec<_> = m.items.iter().filter_map(|i| match i {
        MItem::Wire { names, .. } => Some(names.clone()),
        _ => None,
    }).collect();
    assert_eq!(wires.len(), 2);
    assert_eq!(wires[1].len(), 2, "wire multi-nama");
}

#[test]
fn parse_net_kinds_and_aliases() {
    // F76: `wand`/`wor`/`tri`/`tri0`/`tri1`/`supply0`/`supply1` + alias
    // `triand`→wand, `trior`→wor (LRM 1800 §6.5).
    let src = r#"
module m {
    in a : bit
    wand wa : bit
    wor wo : bit
    tri tr : bit
    tri0 t0 : bit
    tri1 t1 : bit
    triand ta : bit
    trior to : bit
    supply0 s0 : bit
    supply1 s1 : bit
    assign wa = a
    assign wo = a
}
"#;
    let f = parse(src).expect("parse net kinds");
    let kinds: Vec<NetKind> = f.modules[0]
        .items
        .iter()
        .filter_map(|i| match i {
            MItem::Wire { net, .. } => Some(*net),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            NetKind::Wand,
            NetKind::Wor,
            NetKind::Tri,
            NetKind::Tri0,
            NetKind::Tri1,
            NetKind::Wand,
            NetKind::Wor,
            NetKind::Supply0,
            NetKind::Supply1,
        ],
        "net kinds + alias: {kinds:?}"
    );
}

#[test]
fn parse_bind_with_dotted_target() {
    // F78: `bind <target> <module> <name> [(conns)]` — target path dotted.
    let src = r#"
module tb {
    sig clk : bit
    sig flag : bit
    inst dut u_dut (.clk, .flag)
    bind u_dut fmon u_chk (.clk(clk), .flag)
    bind top.u_dut fmon u_chk2 (.clk, .flag(flag))
}
"#;
    let f = parse(src).expect("parse bind");
    let binds: Vec<_> = f.modules[0]
        .items
        .iter()
        .filter_map(|i| match i {
            MItem::Bind { target, module, name, conns, .. } => {
                Some((target.clone(), module.clone(), name.clone(), conns.len()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(binds.len(), 2);
    assert_eq!(binds[0].0, "u_dut");
    assert_eq!(binds[0].1, "fmon");
    assert_eq!(binds[0].2, "u_chk");
    assert_eq!(binds[0].3, 2);
    assert_eq!(binds[1].0, "top.u_dut", "target dotted");
}

#[test]
fn parse_gen_case() {
    // F79: `case (e) { v: {...} default: {...} }` di level module = generate.
    let src = r#"
module m #(SEL = 1) {
    in clk : bit
    out y : logic[7:0]
    case (SEL) {
        0: {
            comb { y = 1 }
        }
        1, 2: {
            comb { y = 2 }
        }
        default: {
            comb { y = 3 }
        }
    }
    casez (SEL) {
        3'b1??: {
            comb { y = 4 }
        }
    }
}
"#;
    let f = parse(src).expect("parse generate case");
    let m = &f.modules[0];
    let cases: Vec<_> = m.items.iter().filter_map(|i| match i {
        MItem::GenCase { kind, items, default, .. } => Some((kind.clone(), items.len(), default.len())),
        _ => None,
    }).collect();
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].0, "case");
    assert_eq!(cases[0].1, 2, "dua branch nilai");
    assert_eq!(cases[0].2, 1, "satu default");
    assert_eq!(cases[1].0, "casez");
}
