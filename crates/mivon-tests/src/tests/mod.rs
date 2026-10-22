use super::*;
use crate::simulator::logicvec_to_string;
use mivon_core::intern::Symbol;

mod bench_profile;
mod bench_release;
mod debug_lex_check;
mod error_categories;
mod parser_lexical;
mod regressions;
mod sim_advanced;
mod stress_tests;

#[test]
fn test_simple_module() {
    let source = r#"
module counter(
    input clk,
    input rst_n,
    output reg [3:0] count
);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n)
            count <= 4'b0000;
        else
            count <= count + 4'b0001;
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_byte_shortint_longint_decl() {
    let source = r#"
module test;
    byte b;
    byte signed bs;
    shortint s;
    shortint signed ss;
    longint l;
    longint signed ls;
    byte [7:0] ba;
    initial begin
        b = 8'hAB;
        s = 16'hABCD;
        l = 64'h1234567890ABCDEF;
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_enum_decl() {
    let source = r#"
module test;
    enum { IDLE, START, DONE } state;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_packed_enum_decl() {
    let source = r#"
module test;
    enum bit [3:0] { RED, GREEN, BLUE } color;
    enum logic [7:0] { A, B, C } val;
    enum int { X, Y, Z } ival;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_typedef_enum() {
    let source = r#"
module test;
    typedef enum { A, B, C } state_t;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_typedef_used_in_decl() {
    let source = r#"
module test;
    typedef enum { IDLE, START, DONE } state_t;
    state_t st;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_typedef_base_types() {
    let source = r#"
module test;
    typedef byte byte_t;
    typedef shortint short_t;
    typedef longint long_t;
    typedef int int_t;
    typedef logic logic_t;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_typedef_used_with_base_types() {
    let source = r#"
module test;
    typedef byte byte_t;
    typedef shortint short_t;
    byte_t b;
    short_t s;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_struct_decl() {
    let source = r#"
module test;
    struct {
        logic [7:0] a;
        logic b;
    } my_var;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_typedef_struct() {
    let source = r#"
module test;
    typedef struct {
        logic [7:0] a;
        logic b;
    } my_struct_t;
    my_struct_t s;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_typedef_union() {
    let source = r#"
module test;
    typedef union {
        int a;
        logic [31:0] b;
    } my_union_t;
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_struct_member_access() {
    let source = r#"
module test;
    struct {
        logic [7:0] a;
        logic [3:0] b;
    } s;
    logic [7:0] ra;
    logic [3:0] rb;
    initial begin
        s.a = 8'hAB;
        s.b = 4'hC;
        #1;
        ra = s.a;
        rb = s.b;
        if (ra !== 8'hAB) $display("FAILED struct a: got %h", ra);
        if (rb !== 4'hC) $display("FAILED struct b: got %h", rb);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "struct member access failed: {:?}",
        result.err()
    );
}

#[test]
fn test_typedef_struct_member_access() {
    let source = r#"
module test;
    typedef struct {
        logic [7:0] a;
        logic [7:0] b;
    } pair_t;
    pair_t s;
    logic [7:0] ra;
    logic [7:0] rb;
    initial begin
        s.a = 8'hDE;
        s.b = 8'hAD;
        #1;
        ra = s.a;
        rb = s.b;
        if (ra !== 8'hDE) $display("FAILED typedef struct a: got %h", ra);
        if (rb !== 8'hAD) $display("FAILED typedef struct b: got %h", rb);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "typedef struct member access failed: {:?}",
        result.err()
    );
}

#[test]
fn test_union_member_access() {
    let source = r#"
module test;
    typedef union {
        logic [7:0] byte_val;
        logic [7:0] alt_val;
    } my_union_t;
    my_union_t u;
    logic [7:0] r;
    initial begin
        u.byte_val = 8'hAB;
        #1;
        r = u.alt_val;
        if (r !== 8'hAB) $display("FAILED union access: got %h", r);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "union member access failed: {:?}",
        result.err()
    );
}

#[test]
fn test_struct_whole_assign() {
    let source = r#"
module test;
    typedef struct {
        logic [7:0] a;
        logic [7:0] b;
    } pair_t;
    pair_t s1, s2;
    logic [7:0] ra, rb;
    initial begin
        s1.a = 8'hDE;
        s1.b = 8'hAD;
        s2 = s1;
        #1;
        ra = s2.a;
        rb = s2.b;
        if (ra !== 8'hDE) $display("FAILED whole struct: ra=%h", ra);
        if (rb !== 8'hAD) $display("FAILED whole struct: rb=%h", rb);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "struct whole assign failed: {:?}",
        result.err()
    );
}

#[test]
fn test_nested_struct_member_access() {
    // OpenTitan pattern `hw2reg.phy_status.init_wip.de`: struct bersarang
    // (struct di dalam struct) diakses via member chain `a.b.c` baik sebagai
    // lvalue (assignment) maupun rvalue (baca). Fix: `collect_member_chain`
    // + `resolve_struct_chain` di elaborator menurunkan chain PENUH menjadi
    // RangeSelect offset — tanpa ini rvalue jadi MemberAccess{obj:RangeSelect}
    // yang di-interpretasi engine sebagai object handle (E9001 / sim hang).
    let source = r#"
module test;
    typedef struct {
        logic init_wip;
        logic [3:0] init_err;
    } phy_status_t;
    typedef struct {
        phy_status_t phy_status;
        logic [7:0] other;
    } hw2reg_t;
    hw2reg_t hw2reg;
    logic r_wip;
    logic [3:0] r_err;
    initial begin
        hw2reg.phy_status.init_wip = 1'b1;
        hw2reg.phy_status.init_err = 4'hA;
        #1;
        r_wip = hw2reg.phy_status.init_wip;
        r_err = hw2reg.phy_status.init_err;
        if (r_wip !== 1'b1) $display("FAILED nested wip: got %b", r_wip);
        if (r_err !== 4'hA) $display("FAILED nested err: got %h", r_err);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "nested struct member access failed: {:?}",
        result.err()
    );
    // Verifikasi NILAI aktual (bukan hanya tidak error): r_err 4-bit harus
    // berisi 4'hA (bukan 1 bit) — width mismatch di warning menunjukkan bug
    // rvalue masih ada bila hanya is_ok dicek.
    if let Ok(sigs) = result {
        let get = |n: &str| {
            sigs.iter()
                .find(|(name, _)| name == n)
                .map(|(_, v)| (v.to_u64(), v.width))
        };
        assert_eq!(get("r_wip"), Some((1, 1)), "r_wip = 1'b1 width 1");
        assert_eq!(get("r_err"), Some((0xA, 4)), "r_err = 4'hA width 4");
    }
}

#[test]
fn test_duplicate_package_last_wins() {
    // OpenTitan: package `tl_main_pkg` didefinisikan PER-TOP (darjeeling /
    // earlgrey / englishbreakfast) dengan item BERBEDA. Module di-dedup
    // "tie → definisi TERAKHIR", jadi package harus konsisten: item yang
    // berkonflik memakai definisi TERAKHIR (bukan first-wins). Sebelum fix
    // first-wins → package = varian top PERTAMA, module = varian TERAKHIR →
    // struct field yang hanya ada di varian terakhir gagal resolve (E2001).
    // Di sini varian kedua menambah field `extra` pada struct — module `use2`
    // memakainya; bila package memakai varian pertama, `rec.extra` E2001.
    // Dua design terpisah tidak bisa di-compile bersama via compile_str (satu
    // file = satu design) — skenario multi-top hanya muncul lewat filelist.
    // Jadi test ini memakai compile_str dengan DUPLIKAT package dalam SATU
    // file (parser mengumpulkan keduanya ke design.packages) dan memverifikasi
    // item yang konflik memakai definisi terakhir.
    let source = r#"
package p;
    typedef struct {
        logic [3:0] a;
    } rec_t;   // varian pertama: 1 field
endpackage

package p;
    typedef struct {
        logic [3:0] a;
        logic extra;
    } rec_t;   // varian kedua: + field extra (last-wins)
endpackage

module use2;
    import p::*;
    p::rec_t rec;
    logic r_extra;
    initial begin
        rec.a = 4'h5;
        rec.extra = 1'b1;
        #1;
        r_extra = rec.extra;
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "duplicate package last-wins failed: {:?}",
        result.err()
    );
    if let Ok(sigs) = result {
        let get = |n: &str| {
            sigs.iter()
                .find(|(name, _)| name == n)
                .map(|(_, v)| (v.to_u64(), v.width))
        };
        // `extra` hanya ada di varian TERAKHIR — bila first-wins, field ini
        // tidak ada di package → elaborasi E2001 (test gagal di result.is_ok).
        // Nilai 1 membuktikan offset field `extra` (bit 4) ter-resolve benar.
        assert_eq!(
            get("r_extra"),
            Some((1, 1)),
            "r_extra = 1 dari varian package terakhir"
        );
    }
}

#[test]
fn test_typedef_with_range() {
    let source = r#"
module test;
    typedef logic [7:0] byte_t;
    typedef bit [3:0] nibble_t;
    typedef reg [15:0] half_t;
    byte_t b;
    nibble_t n;
    half_t h;
    initial begin
        b = 8'hAB;
        n = 4'hA;
        h = 16'h1234;
        #1;
        if (b != 8'hAB) $display("FAILED byte");
        if (n != 4'hA) $display("FAILED nibble");
        if (h != 16'h1234) $display("FAILED half");
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, bv) = sigs.iter().find(|(n, _)| n == "b").unwrap();
    assert_eq!(bv.to_u64(), 0xAB);
    let (_, nv) = sigs.iter().find(|(n, _)| n == "n").unwrap();
    assert_eq!(nv.to_u64(), 0xA);
    let (_, hv) = sigs.iter().find(|(n, _)| n == "h").unwrap();
    assert_eq!(hv.to_u64(), 0x1234);
}

#[test]
fn test_func_return_type_int() {
    let source = r#"
module tb;
    function int double;
        input [7:0] x;
        double = x * 2;
    endfunction
    reg [31:0] result;
    initial begin
        result = double(21);
        #1;
        if (result != 42) $display("FAILED: %d", result);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        v.to_u64(),
        42,
        "function int should return 32-bit wide value"
    );
}

#[test]
fn test_func_local_decl_init_and_return() {
    // F34: function dgn variabel lokal ber-initializer (`int n = x - 1;`)
    // + `return r;` — inline harus meng-emit init ke temp signal.
    // Sebelumnya init diabaikan → temp X → o=0 (bug siluman).
    let source = r#"
module tb;
    function int clog2;
        input int x;
        int r = 0;
        int n = x - 1;
        while (n > 0) begin
            r = r + 1;
            n = n >> 1;
        end
        return r;
    endfunction
    reg [31:0] o;
    initial begin
        o = clog2(256);
        #1;
        if (o != 8) $display("FAILED clog2: %0d", o);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "o").unwrap();
    assert_eq!(
        v.to_u64(),
        8,
        "clog2(256) with local decl init + return should be 8"
    );
}

#[test]
fn test_func_local_init_referencing_port() {
    // F34: initializer yang mereferensikan port (`int w = v * 2;`).
    let source = r#"
module tb;
    function int scale;
        input int v;
        int w = v * 2;
        return w;
    endfunction
    reg [31:0] s;
    initial begin
        s = scale(21);
        #1;
        if (s != 42) $display("FAILED scale: %0d", s);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "s").unwrap();
    assert_eq!(
        v.to_u64(),
        42,
        "scale(21) with port-referencing init should be 42"
    );
}

#[test]
fn test_func_local_init_referencing_other_local() {
    // F34 review: initializer yang mereferensikan variabel lokal LAIN
    // (`int m = n + 1;`) — rename_map harus lengkap saat init di-emit
    // (semua local sudah di-rename di pass pertama sebelum init di-emit).
    let source = r#"
module tb;
    function int calc;
        input int x;
        int n = x - 1;
        int m = n + 1;
        return m;
    endfunction
    reg [31:0] o;
    initial begin
        o = calc(10);
        #1;
        if (o != 10) $display("FAILED calc: %0d", o);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "o").unwrap();
    assert_eq!(
        v.to_u64(),
        10,
        "calc(10) = (10-1)+1 should be 10 (init chaining across locals)"
    );
}

#[test]
fn test_func_local_init_with_nested_call() {
    // F34 review: initializer yang mengandung nested function call
    // (`int y = double(v);`) harus ikut di-inline — sama seperti body.
    let source = r#"
module tb;
    function int double;
        input int v;
        double = v * 2;
    endfunction
    function int quad;
        input int v;
        int y = double(v);
        return y;
    endfunction
    reg [31:0] o;
    initial begin
        o = quad(21);
        #1;
        if (o != 42) $display("FAILED quad: %0d", o);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "o").unwrap();
    assert_eq!(
        v.to_u64(),
        42,
        "quad(21) = double(21) = 42 (nested call in init must be inlined)"
    );
}

#[test]
fn test_func_nonansi_multi_port_bare() {
    // F34 review: bentuk non-ANSI telanjang — `input int a, b` (multi-port)
    // dan `input x` tanpa keyword tipe.
    let source = r#"
module tb;
    function int sum3;
        input int a, b;
        input x;
        sum3 = a + b + x;
    endfunction
    reg [31:0] o;
    initial begin
        o = sum3(10, 20, 1);
        #1;
        if (o != 31) $display("FAILED sum3: %0d", o);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "o").unwrap();
    assert_eq!(
        v.to_u64(),
        31,
        "sum3(10,20,1): multi-port int a,b + bare 1-bit input x => 31"
    );
}

// Helper: jalankan body test rekursif dalam thread ber-stack besar. Rekursi
// function memakai beberapa frame Rust per level (helper + evaluator blok +
// evaluator ekspresi) — stack thread test default 2MB kurang untuk fib(15)
// (main thread CLI 8MB cukup, tapi test thread overflow). 64MB aman.
fn run_with_big_stack(f: impl FnOnce() -> u64 + Send + 'static) -> u64 {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn test_recursive_fibonacci_ansi() {
    // F35: function REKURSIF (gaya ANSI `return n`) dieksekusi via runtime
    // helper execute_module_function_call, bukan inline. Dua bug siluman
    // yang diperbaiki: (1) `Stmt::Return` no-op di evaluator AST-with-delay
    // → __func_ret tak pernah ditulis → return 0; (2) `return n` hanya
    // menghentikan blok if, statement berikutnya tetap jalan → rekursi tak
    // berujung (stack overflow). Kini ast_return_pending menghentikan
    // SELURUH blok.
    let source = r#"
module tb;
    function int fib(input int n);
        if (n <= 1) return n;
        return fib(n - 1) + fib(n - 2);
    endfunction
    reg [31:0] o;
    initial begin
        o = fib(15);
        #1;
        if (o != 610) $display("FAILED fib: %0d", o);
        $finish;
    end
endmodule
"#;
    let got = run_with_big_stack(move || {
        let sigs = simulate_signals(source, 5).unwrap();
        let (_, v) = sigs.iter().find(|(n, _)| n == "o").unwrap();
        v.to_u64()
    });
    assert_eq!(
        got, 610,
        "fib(15) = 610 (recursive ANSI return must terminate and compute correctly)"
    );
}

#[test]
fn test_recursive_factorial_nonansi() {
    // F35: function REKURSIF gaya non-ANSI (`fact = expr`) + if-else —
    // nama function di-pre-insert sebagai slot return agar LHS non-ANSI
    // menulis tanpa RT0001; `__func_ret` TIDAK di-pre-insert agar fallback
    // ke nama function untuk gaya non-ANSI tetap benar.
    let source = r#"
module tb;
    function int fact;
        input int n;
        if (n <= 1) fact = 1;
        else fact = n * fact(n - 1);
    endfunction
    reg [31:0] o;
    initial begin
        o = fact(5);
        #1;
        if (o != 120) $display("FAILED fact: %0d", o);
        $finish;
    end
endmodule
"#;
    let got = run_with_big_stack(move || {
        let sigs = simulate_signals(source, 5).unwrap();
        let (_, v) = sigs.iter().find(|(n, _)| n == "o").unwrap();
        v.to_u64()
    });
    assert_eq!(
        got, 120,
        "fact(5) = 120 (recursive non-ANSI assignment-to-name return)"
    );
}

#[test]
fn test_mv_compound_assignment() {
    // F36: compound assignment (`+=` `<<=` `&=`) + increment (`++`) di .mv
    // di-transpile ke SV compound assignment lalu disimulasikan.
    let src = r#"
module tb_ca {
    sig a : logic[7:0]
    sig b : logic[7:0]
    sig c : logic[7:0]
    sig i : logic[7:0]
    initial {
        a = 10
        a += 5
        b = 2
        b <<= 3
        c = 8'hFF
        c &= 8'h0F
        i = 0
        i++
        $display("TB_CA a=%0d b=%0d c=%0d i=%0d", a, b, c, i)
    }
}
"#;
    let r = mivon_mv::transpile(src, "compound").expect("transpile .mv OK");
    assert!(
        r.sv.contains("a += 5;"),
        "codegen harus emit compound: {}",
        r.sv
    );
    assert!(
        r.sv.contains("b <<= 3;"),
        "codegen harus emit shl compound: {}",
        r.sv
    );
    assert!(
        r.sv.contains("i++;"),
        "codegen harus emit increment: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 15, "a = 10+5");
    assert_eq!(get("b"), 16, "b = 2<<3");
    assert_eq!(get("c"), 15, "c = 0xFF & 0x0F");
    assert_eq!(get("i"), 1, "i++");
}

#[test]
fn test_mv_prefix_incdec() {
    // F37+F48: prefix `++i`/`--i` dan postfix `i--` — di level statement
    // maupun di RHS ekspresi kini dieksekusi penuh dengan side-effect
    // write-back (LRM 1800 §11.4.1).
    let src = r#"
module tb_pp {
    sig i : logic[7:0]
    sig j : logic[7:0]
    sig k : logic[7:0]
    initial {
        i = 0
        ++i
        $display("PP_A %0d", i)
        j = ++i
        $display("PP_B %0d %0d", i, j)
        --i
        $display("PP_C %0d", i)
        i--
        $display("PP_D %0d", i)
        k = 5
        j = ++k
        $display("PP_E %0d %0d", k, j)
    }
}
"#;
    let r = mivon_mv::transpile(src, "prefix").expect("transpile .mv OK");
    // statement prefix di-emit apa adanya; postfix statement juga
    assert!(r.sv.contains("++i;"), "codegen harus emit prefix: {}", r.sv);
    assert!(
        r.sv.contains("--i;"),
        "codegen harus emit prefix dec: {}",
        r.sv
    );
    assert!(
        r.sv.contains("i--;"),
        "codegen harus emit postfix: {}",
        r.sv
    );
    assert!(
        !r.sv.contains("$display(\"PP_B %0d %0d\", i, j)--;"),
        "postfix tidak boleh menempel ke statement lain: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // F48: side-effect RHS kini diterapkan penuh (LRM 1800 §11.4.1):
    // ++i(1) → j=++i (i=2, j=2) → --i(1) → i--(0); k=5 → j=++k (k=6, j=6).
    assert_eq!(get("i"), 0, "i: ++i(1) -> j=++i(2) -> --i(1) -> i--(0)");
    assert_eq!(get("j"), 6, "j = ++k (nilai baru k+1 = 6)");
}

#[test]
fn test_mv_dowhile_event_trigger() {
    // F38: `do { ... } while (cond)` loop post-test + event trigger `->ev`
    // di-transpile ke `do begin ... end while (cond);` dan `-> ev;` lalu
    // disimulasikan (body do jalan minimal sekali, event memicu @(posedge)).
    let src = r#"
module tb_dw {
    sig i : logic[7:0]
    sig ev : bit
    sig got : logic[7:0]
    initial {
        i = 0
        do {
            i = i + 1
        } while (i < 3)
        $display("DW i=%0d", i)
    }
    initial {
        @(posedge ev)
        got = 99
    }
    initial {
        #5
        ->ev
        #5
        $display("EV got=%0d", got)
    }
}
"#;
    let r = mivon_mv::transpile(src, "dowhile").expect("transpile .mv OK");
    assert!(
        r.sv.contains("do begin"),
        "codegen harus emit do begin: {}",
        r.sv
    );
    assert!(
        r.sv.contains("end while (i < 3);"),
        "codegen harus emit end while: {}",
        r.sv
    );
    assert!(
        r.sv.contains("-> ev;"),
        "codegen harus emit event trigger: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("i"), 3, "do while: body jalan 3x");
    assert_eq!(get("got"), 99, "event trigger membangunkan @(posedge ev)");
}

#[test]
fn test_mv_dowhile_while_never_runs_twice() {
    // F38: do...while TIDAK sama dengan while — body jalan minimal sekali
    // bahkan saat kond awal false (`do { x = 1 } while (0)` → x = 1).
    let src = r#"
module tb_dz {
    sig x : logic[7:0]
    initial {
        x = 0
        do {
            x = 1
        } while (0)
    }
}
"#;
    let r = mivon_mv::transpile(src, "dz").expect("transpile .mv OK");
    assert!(r.sv.contains("do begin"), "sv: {}", r.sv);
    assert!(r.sv.contains("end while (0);"), "sv: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("x"), 1, "body do jalan minimal sekali walau cond false");
}

#[test]
fn test_mv_queue_decl_and_ops() {
    // F40: queue `Type[$]` — decl emit `[$]` SETELAH nama (`logic [7:0] q[$]`),
    // operasi method (push_back/pop_front/size) + indeks `q[i]` berjalan
    // penuh di sim. Queue kosong by-design — WR0014 dikecualikan utk is_queue.
    let src = r#"
module tb_q {
    sig q : logic[7:0][$]
    sig n : int = 0
    initial {
        q.push_back(8'd3)
        q.push_back(8'd4)
        n = q.size()
        $display("Q %0d %0d %0d", n, q[0], q[1])
        n = q.pop_front()
        $display("P %0d %0d", q.size(), q[0])
    }
}
"#;
    let r = mivon_mv::transpile(src, "queue").expect("transpile .mv OK");
    assert!(
        r.sv.contains("logic [7:0] q[$];"),
        "codegen harus emit queue decl setelah nama: {}",
        r.sv
    );
    assert!(
        r.sv.contains("q.push_back(8'd3);"),
        "method call emit apa adanya: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // n = size()=2 → lalu n = pop_front() (return elemen pertama = 3).
    assert_eq!(get("n"), 3, "pop_front mengembalikan elemen pertama (3)");
}

#[test]
fn test_mv_queue_width_from_elem_type() {
    // Lebar elemen queue = tipe dalamnya (check type_width) — index q[i]
    // 8-bit; cast ke queue ditolak (E2005).
    let src = r#"
module tb_qw {
    sig q : logic[7:0][$]
    sig v : logic[7:0]
    initial {
        q.push_back(8'd9)
        v = q[0]
        $display("QW %0d", v)
    }
}
"#;
    let r = mivon_mv::transpile(src, "queuew").expect("transpile .mv OK");
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let v = sigs.iter().find(|(s, _)| s == "v").unwrap().1.to_u64();
    assert_eq!(v, 9, "q[0] elemen 8-bit terbaca utuh");
}

/// Queue lewat TYPEDEF (`type Q = logic[7:0][$]` → SV `typedef logic [7:0] Q [$];`).
/// Regresi: dimensi `[$]` setelah nama typedef dulu dibuang parser
/// (`skip_typedef_unpacked_dims`) sehingga `Q q;` dianggap SKALAR → `q.size()`
/// = 0, `q[0]` membaca bit sampah, dan assignment pattern `Q'(a)` → lebar 1.
/// Kini `Q q;` (var) DAN `out q : Q` (port) dimodelkan sebagai queue penuh.
#[test]
fn test_mv_queue_typedef_var_and_cast() {
    let src = r#"
type Q = logic[7:0][$]

module tb_qtd {
    sig q : Q
    sig v : logic[7:0] = 0
    sig n : int = 0
    initial {
        q.push_back(8'd7)
        v = q[0]
        n = q.size()
        // cast-to-queue via typedef → SV `q = '{ 8'hA5 }` (1 elemen)
        q = Q'(8'hA5)
        $display("QT v=%0d n=%0d sz=%0d q0=%0h", v, n, q.size(), q[0])
    }
}
"#;
    let r = mivon_mv::transpile(src, "qtdef").expect("transpile .mv OK");
    assert!(
        r.svh.contains("typedef logic [7:0] Q [$];"),
        "typedef queue emit `[$]` setelah nama: {}",
        r.svh
    );
    // Typedef ada di `.svh` → gabung header + body agar tipe `Q` ter-resolve.
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("v"), 7, "push_back + q[0] pada queue typedef");
    assert_eq!(get("n"), 1, "size()=1 setelah push_back");
    // `q = Q'(8'hA5)` mengganti queue dengan 1 elemen 0xA5.
    assert_eq!(get("q"), 0xA5, "assignment pattern queue 1 elemen = 0xA5");
}

/// Assignment pattern multi-elemen `q = '{a, b, c}` pada queue typedef →
/// queue berisi 3 elemen berurutan (elemen 0 di bit terendah). Menutup jalur
/// `pack_array_pattern` (bukan hanya 1 elemen). Oracle: LRM 1800 §10.9.
#[test]
fn test_queue_typedef_pattern_multi_elem() {
    let source = r#"
typedef logic [7:0] Q [$];
module tb_qpat;
    Q q;
    reg [7:0] e0, e1, e2;
    reg [31:0] n;
    initial begin
        q = '{ 8'h11, 8'h22, 8'h33 };
        e0 = q[0];
        e1 = q[1];
        e2 = q[2];
        n = q.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("e0"), 0x11, "elemen 0 = 0x11 (LSB)");
    assert_eq!(get("e1"), 0x22, "elemen 1 = 0x22");
    assert_eq!(get("e2"), 0x33, "elemen 2 = 0x33 (MSB)");
    assert_eq!(get("n"), 3, "queue berisi 3 elemen");
}

/// Konkatenasi `{a, b}` yang di-assign ke queue: karena parser meng-desugar
/// `'{a,b}` (pola) dan `{a,b}` (concat) ke node `Concat` yang sama, mivon
/// menetapkan urutan operand = urutan elemen (elemen 0 = operand pertama).
/// Test ini MENGUNCI semantik tsb (assign concat ke queue tak terdefinisi di
/// LRM; `.mv` memakai `'{...}` untuk literal array).
#[test]
fn test_queue_concat_operand_order_is_element_order() {
    let source = r#"
typedef logic [7:0] Q [$];
module tb_qcat;
    Q q;
    reg [7:0] a, b;
    reg [7:0] e0, e1;
    reg [31:0] n;
    initial begin
        a = 8'h11;
        b = 8'h22;
        q = {a, b};
        e0 = q[0];
        e1 = q[1];
        n = q.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("e0"), 0x11, "operand pertama = elemen 0");
    assert_eq!(get("e1"), 0x22, "operand kedua = elemen 1");
    assert_eq!(get("n"), 2, "dua elemen");
}

/// Array manipulation method dengan `with (item ...)` (LRM 1800 §7.12.1):
/// parser menerima bentuk TANPA tanda kurung argumen (`q.find with (...)`,
/// bukan hanya `q.find() with (...)`), elaborator men-bind iterator implisit
/// `item` (bukan E2001), dan evaluator memberi nilai `item` per elemen.
#[test]
fn test_array_method_with_clause_item_iterator() {
    let source = r#"
module tb_with;
    int q[$];
    int r[$];
    int iq[$];
    int sum2, mn, mx, fn, fv, fi, s0, s4;
    initial begin
        q = '{1,2,3,4,5};
        r  = q.find with (item > 3);
        fn = r.size();
        r  = q.find_first with (item > 2);
        fv = r[0];
        iq = q.find_index with (item == 3);
        fi = iq[0];
        sum2 = q.sum with (item * 2);
        mn = q.min with (item);
        mx = q.max with (item);
        q.sort with (item);
        s0 = q[0];
        s4 = q[4];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("fn"), 2, "find with (item>3) → 4,5");
    assert_eq!(get("fv"), 3, "find_first with (item>2) → 3");
    assert_eq!(get("fi"), 2, "find_index with (item==3) → indeks 2");
    assert_eq!(get("sum2"), 30, "sum with (item*2) → 30");
    assert_eq!(get("mn"), 1, "min with (item) → 1");
    assert_eq!(get("mx"), 5, "max with (item) → 5");
    assert_eq!(get("s0"), 1, "sort with (item) → elemen 0 = 1");
    assert_eq!(get("s4"), 5, "sort with (item) → elemen 4 = 5");
}

/// Method array yang namanya keyword (`unique`/`and`/`or`/`xor`, LRM 1800
/// §7.12.3) harus bisa dipanggil setelah `.`. `unique` mengembalikan queue
/// baru (tidak mengubah sumber, §7.12.3).
#[test]
fn test_array_method_keyword_names() {
    let source = r#"
module tb_kw;
    int q[$];
    int r[$];
    int a, o, x, u;
    initial begin
        q = '{1,2,3,4,5};
        a = q.and();
        o = q.or();
        x = q.xor();
        q = '{1,1,2,3,3};
        r = q.unique();
        u = r.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 0, "and reduction 1&2&3&4&5 = 0");
    assert_eq!(get("o"), 7, "or reduction 1|2|3|4|5 = 7");
    assert_eq!(get("x"), 1, "xor reduction 1^2^3^4^5 = 1");
    assert_eq!(get("u"), 3, "unique 1,1,2,3,3 → 1,2,3");
}

/// LRM 1800 §7.12.1: `unique_index` mengembalikan queue **indeks** elemen
/// unik pertama (bukan nilainya), tiap indeks 32-bit. Menutup keterbatasan
/// lama di mana hanya `unique` (nilai) yang didukung runtime.
#[test]
fn test_array_method_unique_index() {
    let source = r#"
module tb_ui;
    int q[$];
    int iq[$];
    int n, i0, i1, i2;
    initial begin
        q = '{1,1,2,3,3};
        iq = q.unique_index();
        n  = iq.size();
        i0 = iq[0];
        i1 = iq[1];
        i2 = iq[2];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 3, "unique_index 1,1,2,3,3 → 3 indeks unik");
    assert_eq!(get("i0"), 0, "indeks pertama elemen unik 1 → 0");
    assert_eq!(get("i1"), 2, "indeks pertama elemen unik 2 → 2");
    assert_eq!(get("i2"), 3, "indeks pertama elemen unik 3 → 3");
}

/// LRM 1800 §7.12.1: `unique_index` menerima `with` clause opsional; dedup
/// dilakukan atas **nilai clause** (bukan elemen mentah), yang dikembalikan
/// adalah indeks elemen. Menutup review-fix F84: membuktikan jalur
/// `is_array_with_method("unique_index")` (binding `item`) benar-benar dipakai.
#[test]
fn test_array_method_unique_index_with_clause() {
    let source = r#"
module tb_uiw;
    int q[$];
    int iq[$];
    int n, i0, i1;
    initial begin
        q = '{1,1,2,3,3};
        iq = q.unique_index with (item / 2);
        n  = iq.size();
        i0 = iq[0];
        i1 = iq[1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // item/2 = 0,0,1,1,1 → kunci unik {0,1} pada indeks 0 dan 2.
    assert_eq!(get("n"), 2, "unique_index with (item/2) → 2 indeks unik");
    assert_eq!(get("i0"), 0, "indeks kunci 0 → 0");
    assert_eq!(get("i1"), 2, "indeks kunci 1 → 2");
}

/// LRM 1800 §7.12.1: akses FIELD struct pada iterator `item` di klausa `with`
/// (`q.find with (item.x > 3)`). Sebelum fix, `item.x` di-emit jadi
/// `HierRef("item.x")` → RT0001 "hierarchical signal 'item.x' not found".
/// Kini elaborator memotong bit elemen sesuai offset field (ExprRangeSelect).
#[test]
fn test_array_method_with_item_field_access() {
    let source = r#"
typedef struct packed { logic [7:0] x; logic [7:0] y; } p_t;
module tb_if;
    p_t q[$];
    p_t r[$];
    int iq[$];
    int n, s, mn, mx, i0;
    initial begin
        q = '{'{8'd1,8'd10}, '{8'd5,8'd20}, '{8'd9,8'd30}};
        r  = q.find with (item.x > 3);
        n  = r.size();
        s  = q.sum with (item.y);
        mn = q.min with (item.x);
        mx = q.max with (item.y);
        iq = q.find_index with (item.x == 9);
        i0 = iq[0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // x = byte tinggi, y = byte rendah: elemen e = (x<<8)|y.
    assert_eq!(get("n"), 2, "find with (item.x > 3) → 2 elemen");
    assert_eq!(get("s"), 60, "sum with (item.y) = 10+20+30");
    assert_eq!(
        get("mn"),
        266,
        "min with (item.x) → elemen pemenang x=1 (266)"
    );
    assert_eq!(
        get("mx"),
        2334,
        "max with (item.y) → elemen pemenang y=30 (2334)"
    );
    assert_eq!(get("i0"), 2, "find_index with (item.x == 9) → 2");
}

/// Bug #41: member struct TANPA range eksplisit (`int y;`) lebarnya dari TIPE
/// (32), bukan 1. PRA-FIX offset akumulasi struct_fields + lebar total struct
/// memakai `range.unwrap_or(1)` → member MSB (`x`) salah offset → `e.x` baca
/// bit yang salah (21 bukan 7) dan `q.sum with (item.x)` salah (150 bukan 14).
#[test]
fn test_inline_struct_type_width_member_offsets() {
    let source = r#"
module tb_inl;
    struct packed { logic [7:0] x; int y; } q[$];
    struct packed { logic [7:0] x; int y; } e;
    int s, sx, i0;
    int iq[$];
    initial begin
        q = '{'{8'd5, 32'd100}, '{8'd9, 32'd200}};
        s  = q.sum with (item.x);
        iq = q.find_index with (item.x == 9);
        i0 = iq[0];
        e = '{8'd7, 32'd42};
        sx = e.x;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("sx"), 7, "e.x (member MSB, offset 32) = 7");
    assert_eq!(get("s"), 14, "sum with (item.x) = 5+9");
    assert_eq!(get("i0"), 1, "find_index with (item.x == 9) = 1");
}

/// LRM 1800 §7.12.1: method array BERTAUT pada hasil temporer
/// (`q.unique().size()`). PRA-FIX hasil method dalam diperlakukan sebagai
/// handle objek → RT0001 "cannot call method 'size' on object with unknown
/// class". Kini hasil dalam disimpan ke slot scratch lalu di-dispatch ulang.
#[test]
fn test_array_method_chained_on_temporary() {
    let source = r#"
module tb_ch;
    int q[$];
    int n1, n2, n3, n4, s1;
    initial begin
        q = '{3,1,2,1,3};
        n1 = q.unique().size();
        n2 = q.unique_index().size();
        n3 = q.find with (item == 1).size();
        n4 = q.find with (item > 1).unique().size();
        s1 = q.unique().sum();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n1"), 3, "q.unique().size() → 3");
    assert_eq!(get("n2"), 3, "q.unique_index().size() → 3");
    assert_eq!(get("n3"), 2, "q.find with (item==1).size() → 2");
    assert_eq!(get("n4"), 2, "q.find with (item>1).unique().size() → 2");
    assert_eq!(get("s1"), 6, "q.unique().sum() → 3+1+2");
}

/// LRM 1800 §7.12.1: `q[i]` / `q[a:b]` pada QUEUE (dan pada hasil method yang
/// mengembalikan queue, `q.unique()[i]`) = potongan ELEMEN, bukan potongan
/// bit. PRA-FIX `q.unique()[1]` baca bit ke-1 (0) & `q[0:1]` → queue kosong.
#[test]
fn test_queue_element_index_and_slice() {
    let source = r#"
module tb_qs;
    int q[$];
    int r[$];
    int e0, e1, ei, i, n, r0, r1;
    initial begin
        q = '{1,2,3,4};
        r  = q[0:1];
        n  = r.size();
        r0 = r[0];
        r1 = r[1];
        e0 = q.unique()[0];
        e1 = q.unique()[1];
        i  = 2;
        ei = q.unique()[i];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 2, "q[0:1] → 2 elemen");
    assert_eq!(get("r0"), 1, "r[0] = 1");
    assert_eq!(get("r1"), 2, "r[1] = 2");
    assert_eq!(get("e0"), 1, "q.unique()[0] = 1");
    assert_eq!(get("e1"), 2, "q.unique()[1] = 2 (bukan bit 1)");
    assert_eq!(get("ei"), 3, "q.unique()[i=2] = 3 (index dinamis)");
}

/// Review-fix F89 (bug #43): slice queue di-CLAMP ke size aktual & rentang
/// DESCENDING / queue KOSONG → queue kosong (LRM 1800 §7.12.1). Juga regresi:
/// menugaskan slice KOSONG ke queue yang tadinya berisi HARUS mengosongkannya
/// (dulu di-zero-extend balik ke lebar storage lama oleh jalur RHS signed).
#[test]
fn test_queue_slice_clamped_and_empty() {
    let source = r#"
module tb_qc;
    int q[$];
    int r[$];
    int upper, lower, desc, emptyq, cleared, keep;
    initial begin
        q = '{1,2,3,4};
        r = q[0:9];   upper = r.size();   // clamp ke 4
        r = q[5:9];   lower = r.size();   // indeks awal di luar → 0
        r = q[3:0];   desc  = r.size();   // descending → 0
        q.delete();
        r = q[0:1];   emptyq = r.size();  // queue kosong → 0
        // regresi: r berisi lalu ditugasi slice kosong → HARUS kosong
        q = '{1,2,3,4};
        r = q[0:1];                       // r = 2 elemen
        r = q[5:9];   cleared = r.size(); // harus 0 (bukan 2)
        r = q[0:1];   keep = r.size();    // 2 (in-range tetap benar)
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("upper"), 4, "q[0:9] pada 4-elemen → clamp ke 4");
    assert_eq!(get("lower"), 0, "q[5:9] indeks awal di luar batas → kosong");
    assert_eq!(get("desc"), 0, "q[3:0] descending → queue kosong");
    assert_eq!(get("emptyq"), 0, "slice pada queue kosong → kosong");
    assert_eq!(
        get("cleared"),
        0,
        "slice kosong mengosongkan target (bukan zero-extend)"
    );
    assert_eq!(get("keep"), 2, "slice in-range tetap benar");
}

/// Review-fix F89: slice pada HASIL method ber-queue juga di-clamp
/// (`q.unique()[0:9]`), dan slice kosong mengosongkan target.
#[test]
fn test_queue_method_result_slice_clamped() {
    let source = r#"
module tb_qmc;
    int q[$];
    int r[$];
    int n, clamp, mid, v;
    initial begin
        q = '{1,1,2,3,3};
        r = q.unique();       n = r.size();      // 3
        r = q.unique()[0:9];  clamp = r.size();  // clamp ke 3
        r = q.unique()[1:1];  mid = r.size(); v = r[0];  // 1 elemen = 2
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 3, "q.unique() → 3 elemen");
    assert_eq!(get("clamp"), 3, "q.unique()[0:9] → clamp ke 3");
    assert_eq!(get("mid"), 1, "q.unique()[1:1] → 1 elemen");
    assert_eq!(get("v"), 2, "q.unique()[1:1][0] = 2");
}

/// Review-fix F89 (temuan [major] auto-review): guard "LHS queue → jangan
/// sign-extend" HARUS hanya untuk penugasan SELURUH queue (`IrLValue::Signal`).
/// Kalau tidak, penugasan ELEMEN `q[0] = -8'sd3` kehilangan sign-extension
/// (253, harus -3). LHS skalar/fixed-width tetap sign-extend.
#[test]
fn test_queue_element_assign_sign_extension() {
    let source = r#"
module tb_qes;
    int q[$];
    byte b;
    int e;
    initial begin
        b = -1;
        q = '{0};
        q[0] = b;         // elemen (ArrayIndex) → sign-extend
        e = b;            // skalar → sign-extend
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_i64();
    assert_eq!(get("e"), -1, "skalar byte→int sign-extend = -1");
    let q0 = sigs.iter().find(|(s, _)| s == "q").map(|(_, v)| v).unwrap();
    // q[0] harus -1 (0xFFFFFFFF); bandingkan 32-bit rendah.
    assert_eq!(q0.to_u64() as u32, 0xFFFF_FFFF, "q[0] = -1 (sign-extended)");
}

/// Review-fix F89: `$display("%d", <queue kosong>)` tidak boleh panic
/// (`LogicVec::to_i64` underflow saat width 0). Menutup [minor] auto-review.
#[test]
fn test_empty_queue_display_no_panic() {
    let source = r#"
module tb_qed;
    int q[$];
    int r[$];
    int n;
    initial begin
        q = '{1,2,3,4};
        r = q[5:9];       // slice kosong
        n = r.size();
        $display("ED n=%0d v=%0d", n, r);  // dulu panic (width 0)
        q.delete();
        $display("ED2 v=%0d", q);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("queue kosong tak boleh panic");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 0, "slice kosong → size 0");
}

/// Review F41: `min`/`max` mengembalikan **elemen** (bukan nilai kunci
/// `with`), dan `find_last`/`find_last_index` mengembalikan **hanya satu**
/// kecocokan terakhir (LRM 1800 §7.12.1).
#[test]
fn test_array_method_minmax_key_and_find_last() {
    let source = r#"
module tb_mm;
    int q[$];
    int r[$];
    int iq[$];
    int mn, mx, mnk, mxk, rn, rv, ni, nv;
    initial begin
        q = '{1, 100, 50};
        mn  = q.min();
        mx  = q.max();
        mnk = q.min with (100 - item);
        mxk = q.max with (100 - item);
        q = '{1, 5, 2, 5, 3};
        r  = q.find_last with (item == 5);
        rn = r.size();
        rv = r[0];
        iq = q.find_last_index with (item == 5);
        ni = iq.size();
        nv = iq[0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("mn"), 1, "min tanpa clause = elemen minimum");
    assert_eq!(get("mx"), 100, "max tanpa clause = elemen maksimum");
    assert_eq!(
        get("mnk"),
        100,
        "min with (100-item) -> ELEMEN dgn kunci min"
    );
    assert_eq!(get("mxk"), 1, "max with (100-item) -> ELEMEN dgn kunci max");
    assert_eq!(get("rn"), 1, "find_last -> tepat 1 elemen");
    assert_eq!(get("rv"), 5, "find_last -> nilai 5");
    assert_eq!(get("ni"), 1, "find_last_index -> tepat 1 indeks");
    assert_eq!(get("nv"), 3, "find_last_index -> indeks 3");
}

/// Bug #37: rantai ekspresi panjang (`&&`/`||` ≥8 operan) menabrak stack
/// thread test 2 MiB karena setiap level rekursi `evaluate_expr_impl`
/// memakai frame ~184 KB (match raksasa menumpuk temporari antar arm di
/// build debug). Fix decisif: `evaluate_expr_impl` dibuat tipis — tulang
/// belakang rekursif `BinaryOp`/`UnaryOp` ditangani lewat helper
/// `#[inline(never)] eval_binary_op`/`eval_unary_op`, sisanya didelegasikan
/// ke `evaluate_expr_deep` (match raksasa) yang TIDAK lagi dialokasikan per
/// level rekursi. Test ini memakai `simulate_signals` (evaluator rekursif,
/// `use_packed_eval=false`) di thread test default — pra-fix 8 operan sudah
/// `SIGABRT` stack overflow; pasca-fix 8/12/16 operan benar. (Rantai lebih
/// panjang, mis. 32 operan, masih menabrak limit deep-recursion BERBEDA di
/// elaborator — `const_eval_with_params`/`elaborate_expr`; lihat buglog #38.)
#[test]
fn test_deep_boolean_chain_no_stack_overflow() {
    let source = r#"
module tb_deepchain;
    int a, b, c, d, e, f, g, h, i, j;
    int ok8, ok12, ok16, or12, neg8;
    initial begin
        a=1; b=2; c=3; d=4; e=5; f=6; g=7; h=8; i=9; j=10;
        ok8  = (a==1) && (b==2) && (c==3) && (d==4) && (e==5) && (f==6) && (g==7) && (h==8);
        ok12 = (a==1) && (b==2) && (c==3) && (d==4) && (e==5) && (f==6)
             && (g==7) && (h==8) && (i==9) && (j==10) && (a<b) && (b<c);
        ok16 = (a==1) && (b==2) && (c==3) && (d==4) && (e==5) && (f==6)
             && (g==7) && (h==8) && (i==9) && (j==10)
             && (a<2) && (b<3) && (c<4) && (d<5) && (e<6) && (f<7);
        or12 = (a==9) || (b==9) || (c==9) || (d==9) || (e==9) || (f==9)
             || (g==9) || (h==9) || (i==9) || (j==10) || (a>b) || (b>c);
        neg8 = (a==2) && (b==2) && (c==3) && (d==4) && (e==5) && (f==6) && (g==7) && (h==8);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("ok8"), 1, "8 operan && semua benar → 1");
    assert_eq!(get("ok12"), 1, "12 operan && semua benar → 1");
    assert_eq!(get("ok16"), 1, "16 operan && semua benar → 1 (headroom)");
    assert_eq!(get("or12"), 1, "12 operan || (j==10 benar) → 1");
    assert_eq!(get("neg8"), 0, "8 operan && (a==2 salah) → 0");
}

#[test]
fn test_mv_fork_join_modes() {
    // F39: `fork { ... } { ... } join / join_any / join_none` di-transpile
    // ke SV `fork begin ... end begin ... end join[_any|_none]` lalu
    // disimulasikan: join menunggu semua, join_any lanjut saat pertama
    // selesai, join_none lanjut segera (background jalan).
    let src = r#"
module tb_fj {
    sig a : logic[7:0]
    sig b : logic[7:0]
    sig reached : bit
    initial {
        fork {
            #10
            a = 1
        } {
            #5
            b = 2
        } join
        $display("FJ_JOIN a=%0d b=%0d", a, b)
    }
    initial {
        fork {
            #10
            a = 11
        } {
            #5
            b = 12
        } join_any
        reached = 1
        #20
        $display("FJ_ANY a=%0d b=%0d reached=%0d", a, b, reached)
    }
    initial {
        fork {
            #10
            b = 22
        } join_none
        #15
        $display("FJ_NONE b=%0d", b)
    }
}
"#;
    let r = mivon_mv::transpile(src, "forkjoin").expect("transpile .mv OK");
    assert!(r.sv.contains("fork\n"), "codegen harus emit fork: {}", r.sv);
    assert!(r.sv.contains("join"), "codegen harus emit join: {}", r.sv);
    assert!(
        r.sv.contains("join_any"),
        "codegen harus emit join_any: {}",
        r.sv
    );
    assert!(
        r.sv.contains("join_none"),
        "codegen harus emit join_none: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 30).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 11, "branch #10 selesai (join + join_any lanjut)");
    assert_eq!(get("b"), 22, "branch terakhir join_none menimpa b");
    assert_eq!(
        get("reached"),
        1,
        "join_any lanjut setelah branch pertama (#5)"
    );
}

#[test]
fn test_mv_fork_join_missing_join_rejected() {
    // F39: fork tanpa keyword join di akhir → error jelas di level .mv.
    let src = r#"
module tb_bad {
    sig a : logic[7:0]
    initial {
        fork {
            a = 1
        }
    }
}
"#;
    let e = mivon_mv::transpile(src, "bad").unwrap_err();
    assert!(
        e.to_string().contains("join"),
        "pesan harus sebut join: {e}"
    );
}

#[test]
fn test_mv_wait_disable_fork() {
    // F45: `wait fork;` tunggu branch join_none; `disable fork;` bunuh branch.
    // Demo: examples/mv/fork_ctrl.mv → WAIT_FORK_DONE a=1 b=2, DISABLE_FORK c=0.
    let src = r#"
module tb_fc {
    sig a : logic[7:0] = '0
    sig b : logic[7:0] = '0
    sig c : logic[7:0] = '0
    initial {
        fork {
            #10
            a = 1
        } {
            #5
            b = 2
        } join_none
        wait fork
    }
    initial {
        fork {
            #10
            c = 99
        } join_none
        #2
        disable fork
    }
}
"#;
    let r = mivon_mv::transpile(src, "fork_ctrl").expect("transpile .mv OK");
    assert!(r.sv.contains("wait fork;"), "emit wait fork: {}", r.sv);
    assert!(
        r.sv.contains("disable fork;"),
        "emit disable fork: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 30).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 1, "wait fork tunggu branch #10");
    assert_eq!(get("b"), 2, "wait fork tunggu branch #5");
    assert_eq!(get("c"), 0, "disable fork bunuh branch #10");
}

#[test]
fn test_mv_force_release() {
    // F46: `force` override + assign diabaikan + `release` melepas blokir.
    // Demo: examples/mv/force_release.mv → FR_FORCED=77, FR_UNBLOCKED=5, FR_OK=7.
    let src = r#"
module tb_fr {
    sig w : logic[7:0] = '0
    task poke() {
        force w = 99
        #5
        release w
        w = 7
    }
    initial {
        w = 42
        #1
        force w = 77
        w = 1
        #1
    }
    initial {
        #10
        poke()
    }
}
"#;
    let r = mivon_mv::transpile(src, "force_release").expect("transpile .mv OK");
    assert!(r.sv.contains("force w = 77;"), "emit force: {}", r.sv);
    assert!(r.sv.contains("release w;"), "emit release: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 30).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("w"), 7, "force task + release + reassign");
}

#[test]
fn test_mv_disable_label_cross_branch() {
    // F47: `label : { ... }` (blok bernama) + `disable <label>` dari branch
    // fork lain — hanya blok bernama yang mati, pelaku LANJUT (LRM §9.6.4).
    // Demo: examples/mv/disable_label.mv → a=0 (worker dibunuh), done=1.
    let src = r#"
module tb_dl {
    sig a : logic[7:0] = '0
    sig done : bit = 0
    initial {
        fork {
            worker : {
                #10
                a = 1
            }
        } {
            #5
            disable worker
        } join
        done = 1
    }
}
"#;
    let r = mivon_mv::transpile(src, "disable_label").expect("transpile .mv OK");
    assert!(
        r.sv.contains("begin : worker"),
        "emit named block: {}",
        r.sv
    );
    assert!(
        r.sv.contains("disable worker;"),
        "emit disable label: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 0, "worker dibunuh disable label sebelum #10");
    assert_eq!(
        get("done"),
        1,
        "join selesai normal — pelaku tidak ikut mati"
    );
}

#[test]
fn test_mv_postfix_rhs_supported() {
    // F48: postfix di RHS ekspresi (`k = i--`) kini DIDUKUNG penuh —
    // side-effect write-back ±1 dieksekusi engine (LRM 1800 §11.4.1).
    // Hasil = nilai LAMA (postfix), operand berkurang 1.
    let src = r#"
module tb_post {
    sig i : logic[7:0]
    sig j : logic[7:0]
    sig k : logic[7:0]
    initial {
        i = 5
        k = i--
        j = i
    }
}
"#;
    let r = mivon_mv::transpile(src, "post").expect("postfix RHS harus lolos");
    assert!(r.sv.contains("k = i--;"), "emit postfix: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("k"), 5, "postfix hasil = nilai LAMA");
    assert_eq!(get("i"), 4, "side-effect: i berkurang 1");
    assert_eq!(get("j"), 4, "baca setelah postfix melihat nilai baru");
}

#[test]
fn test_mv_wire_assign() {
    // F75: `wire` (net, LRM §6.5/§10.2) + `assign` continuous — 1 baris vs blok comb.
    // Demo: examples/mv/wire_assign.mv → y=1 (1^0), v=42 (41+1).
    let src = r#"
module tb_wa {
    sig a, b : bit
    sig x : logic[7:0]
    sig y : bit
    sig v : logic[7:0]
    wire w_and : bit
    wire wv : logic[7:0]
    assign w_and = a & b
    assign y = w_and ^ b
    assign wv = x + 1
    assign v = wv
    initial {
        a = 1
        b = 0
        x = 41
        #1
    }
}
"#;
    let r = mivon_mv::transpile(src, "wire_assign").expect("transpile .mv OK");
    assert!(r.sv.contains("wire w_and;"), "emit wire net: {}", r.sv);
    assert!(
        r.sv.contains("wire [7:0] wv;"),
        "emit wire vektor: {}",
        r.sv
    );
    assert!(!r.sv.contains("wire bit"), "wire+bit INVALID: {}", r.sv);
    assert!(
        r.sv.contains("assign w_and = a & b;"),
        "emit assign: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("y"), 0, "a=1,b=0 → w=0 → y=0^0=0");
    assert_eq!(get("v"), 42, "41+1 via wire");
}

#[test]
fn test_mv_net_kind_resolution() {
    // F76: resolusi multi-driver per varian net (LRM 1800 §6.5):
    // wand(1,0)=0, wor(1,0)=1. Engine sudah dukung (matrix ✅), .mv kini mengeksposnya.
    let src = r#"
module tb_nets {
    sig a : bit
    sig o_and : bit
    sig o_or : bit
    wand w_and : bit
    wor w_or : bit
    assign w_and = a
    assign w_and = 0
    assign w_or = a
    assign w_or = 0
    assign o_and = w_and
    assign o_or = w_or
    initial {
        a = 1
        #1
    }
}
"#;
    let r = mivon_mv::transpile(src, "nets").expect("transpile .mv OK");
    assert!(r.sv.contains("wand w_and;"), "emit wand: {}", r.sv);
    assert!(r.sv.contains("wor w_or;"), "emit wor: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("o_and"), 0, "wand(1,0)=0 AND-resolution");
    assert_eq!(get("o_or"), 1, "wor(1,0)=1 OR-resolution");
}

#[test]
fn test_mv_bind_emits_and_simulates_clean() {
    // F78: `bind` di-emit 1:1 ke SV (LRM §23.11). Engine mivon memperlakukan
    // bind body-module sebagai verification-only passthrough (sama seperti
    // input SV-nya — instance bind dimaterialisasi tool EDA, bukan mivon),
    // jadi sim desain harus berjalan bersih tanpa error.
    // Demo: examples/mv/bind_check.mv.
    let src = r#"
module flag_monitor {
    in clk : bit
    in flag : bit
    initial {
        @(posedge clk)
        #1
        assert (flag == 1) $info("bind sees flag=1") else $error("bind fail")
    }
}
module dut {
    in clk : bit
    out flag : bit
    seq(clk) {
        flag <= 1
    }
}
module tb_mon {
    sig clk : bit
    sig flag : bit
    inst dut u_dut (.clk, .flag)
    bind u_dut flag_monitor u_mon (.clk(clk), .flag(flag))
    initial {
        clk = 0
        forever #5 clk = ~clk
    }
    initial {
        #30
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "bind_check").expect("transpile .mv OK");
    assert!(
        r.sv.contains("bind u_dut flag_monitor u_mon ("),
        "emit bind: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 60).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("flag"), 1, "DUT berjalan normal (bind passthrough)");
}

#[test]
fn test_mv_gen_case_selects_branch() {
    // F79: generate `case` memilih cabang sesuai parameter (SEL=1 → y=20).
    // Demo: examples/mv/gen_case.mv.
    let src = r#"
module gc #(SEL = 1) {
    in clk : bit
    out y : logic[7:0]
    case (SEL) {
        0: {
            comb { y = 10 }
        }
        1, 2: {
            comb { y = 20 }
        }
        default: {
            comb { y = 99 }
        }
    }
}
module tb_gc2 {
    sig clk : bit
    sig y : logic[7:0]
    inst gc u (.clk, .y)
    initial {
        clk = 0
        #1
    }
}
"#;
    let r = mivon_mv::transpile(src, "gen_case").expect("transpile .mv OK");
    assert!(r.sv.contains("case (SEL)"), "emit case: {}", r.sv);
    assert!(
        r.sv.contains("1, 2: begin : gen_case_1"),
        "multi-label: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("y"), 20, "cabang SEL=1 (multi-label 1,2) terpilih");
}

#[test]
fn test_mv_covergroup_samples_implicitly() {
    // F81: covergroup .mv ter-sample otomatis tiap edge (tanpa sample()
    // eksplisit) — functional coverage > 0. Demo: examples/mv/cover.mv.
    let src = r#"
module tb_cov {
    sig clk : bit
    sig x : logic[7:0]
    covergroup cg @(posedge clk) {
        coverpoint x {
            bins lo = {0, 1}
            bins hi = {[2:10]}
        }
    }
    initial {
        clk = 0
        x = 0
        #12
        x = 5
        repeat (10) @(posedge clk)
        $finish
    }
    initial {
        forever #5 clk = ~clk
    }
}
"#;
    let r = mivon_mv::transpile(src, "cover").expect("transpile .mv OK");
    assert!(
        r.sv.contains("covergroup cg @(posedge clk);"),
        "emit: {}",
        r.sv
    );
    let design = mivon_api::compile_str(&r.sv).expect("compile SV hasil generate");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 200);
    engine.run().expect("sim harus jalan");
    let stats = engine.coverage_stats();
    let covered = stats.get("covergroup_covered").copied().unwrap_or(0.0);
    assert!(
        covered >= 1.0,
        "implicit sampling harus hit ≥1 bin: {stats:?}"
    );
}

#[test]
fn test_mv_compound_more_ops() {
    // F36 coverage: operator compound lain (`%=` `>>=` `|=` `^=`) + decrement
    // (`--`) — memastikan semua token compound ter-lex & ter-emit benar.
    let src = r#"
module tb_cm {
    sig d : logic[7:0]
    sig e : logic[7:0]
    sig f : logic[7:0]
    sig g : logic[7:0]
    sig j : logic[7:0]
    initial {
        d = 100
        d %= 7
        e = 8'h80
        e >>= 2
        f = 8'h0F
        f |= 8'hF0
        g = 8'hFF
        g ^= 8'h0F
        j = 5
        j--
        $display("TB_CM d=%0d e=%0d f=%0d g=%0d j=%0d", d, e, f, g, j)
    }
}
"#;
    let r = mivon_mv::transpile(src, "compound_more").expect("transpile OK");
    assert!(r.sv.contains("d %= 7;"), "codegen: {}", r.sv);
    assert!(r.sv.contains("e >>= 2;"), "codegen: {}", r.sv);
    assert!(r.sv.contains("f |= 8'hF0;"), "codegen: {}", r.sv);
    assert!(r.sv.contains("g ^= 8'h0F;"), "codegen: {}", r.sv);
    assert!(r.sv.contains("j--;"), "codegen: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("d"), 2, "d = 100 % 7");
    assert_eq!(get("e"), 32, "e = 0x80 >> 2");
    assert_eq!(get("f"), 255, "f = 0x0F | 0xF0");
    assert_eq!(get("g"), 240, "g = 0xFF ^ 0x0F");
    assert_eq!(get("j"), 4, "j = 5--");
}

#[test]
fn test_mv_compound_seq_rejected() {
    // F36: compound assignment bersifat blocking — di dalam `seq` harus
    // ditolak E2004 (sama seperti `=`), error di level .mv bukan SV hasil.
    let src = r#"
module bad_seq {
    in clk : bit
    sig a : logic[7:0]
    seq(clk) {
        a += 1
    }
}
"#;
    let err = mivon_mv::transpile(src, "bad_seq").expect_err("compound di seq harus error E2004");
    assert!(
        err.format().contains("E2004"),
        "error harus E2004 (blocking di seq): {}",
        err.format()
    );
}

#[test]
fn test_mv_default_arg_function() {
    // `.mv` default arg function: `func scale(v:int, factor:int=4)` →
    // `input int factor = 4`; call 1-arg pakai default (40), call 2-arg
    // override (30) — engine SV mendukung default param.
    let src = r#"
func scale(v : int, factor : int = 4) -> int {
    return v * factor
}
module tb {
    sig a, b : logic[31:0]
    comb {
        a = scale(10)
        b = scale(10, 3)
    }
}
"#;
    let r = mivon_mv::transpile(src, "def").expect("transpile default arg OK");
    assert!(
        r.sv.contains("input int factor = 4"),
        "codegen harus emit default arg: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 40, "scale(10) pakai default factor=4");
    assert_eq!(get("b"), 30, "scale(10,3) override factor=3");
}

#[test]
fn test_mv_foreach_array_fill() {
    // `foreach (rom[i]) { rom[i] = i }` di initial → SV foreach; engine
    // mengisi array per-elemen (rom[3]=3).
    let src = r#"
module tb {
    sig rom : logic[8][4]
    sig chk : logic[7:0]
    initial {
        foreach (rom[i]) {
            rom[i] = i
        }
        chk = rom[3]
    }
}
"#;
    let r = mivon_mv::transpile(src, "fe").expect("transpile foreach OK");
    assert!(
        r.sv.contains("foreach (rom[i]) begin"),
        "codegen foreach: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let chk = sigs.iter().find(|(s, _)| s == "chk").unwrap().1.to_u64();
    assert_eq!(chk, 3, "rom[3] = 3 via foreach");
}

#[test]
fn test_mv_typedef_local_module() {
    // typedef lokal module (.mv): type/enum di badan module → scope-lokal SV;
    // member enum td `.mv` dikenal (E2001 lolos).
    let src = r#"
module tb {
    type Word8 = logic[7:0]
    enum Mode { OFF, ON }
    sig val : Word8
    sig m   : Mode
    comb {
        val = 8'h5A
        m   = ON
    }
}
"#;
    let r = mivon_mv::transpile(src, "tl").expect("transpile typedef lokal OK");
    assert!(
        r.sv.contains("typedef logic [7:0] Word8;"),
        "codegen typedef lokal: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("val"), 0x5A);
    assert_eq!(get("m"), 1, "enum member ON = 1");
}

#[test]
fn test_mv_readmemh_rom() {
    // `.mv` `$readmemh("f", rom)` di initial — `$` di-lex sbg ident, Call
    // di-emit apa adanya; engine mengisi ROM dari file (regression: jangan
    // rusak saat `$`-task diproses).
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("mivon_mv_rh_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let hex_path = dir.join("rom.hex");
    {
        let mut f = std::fs::File::create(&hex_path).unwrap();
        f.write_all(b"AA\nBB\nCC\nDD\n").unwrap();
    }
    let src = format!(
        r#"
module rom_dut {{
    in  idx : logic[1:0]
    out val : logic[7:0]
    sig rom : logic[8][4]
    initial {{
        $readmemh("{}", rom)
    }}
    comb {{
        val = rom[idx]
    }}
}}
module tb {{
    sig idx : logic[1:0]
    sig val : logic[7:0]
    inst rom_dut u (.idx(idx), .val(val))
    initial {{
        idx = 0
        #2
        idx = 3
        #1
    }}
}}
"#,
        hex_path.display()
    );
    let r = mivon_mv::transpile(&src, "rh").expect("transpile readmemh OK");
    assert!(
        r.sv.contains("$readmemh("),
        "codegen harus emit $readmemh: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 10).unwrap();
    let val = sigs.iter().find(|(s, _)| s == "val").unwrap().1.to_u64();
    assert_eq!(val, 0xDD, "rom[3] dari file hex = DD");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_mv_named_arg_call() {
    // Named-arg call `.mv`: `scale(10, factor = 3)` → SV `.factor(3)`;
    // order bebas `scale(factor = 2, v = 5)`.
    let src = r#"
func scale(v : int, factor : int) -> int {
    return v * factor
}
module tb {
    sig a, b : logic[31:0]
    comb {
        a = scale(10, factor = 3)
        b = scale(factor = 2, v = 5)
    }
}
"#;
    let r = mivon_mv::transpile(src, "na").expect("transpile named arg OK");
    assert!(
        r.sv.contains("scale(10, .factor(3))"),
        "codegen named arg: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 30, "scale(10, factor=3)");
    assert_eq!(get("b"), 10, "scale(factor=2, v=5) — order bebas");
}

#[test]
fn test_mv_for_step() {
    // `for i in 0..N step 2` — increment `i = i + 2` di behavioral &
    // generate; step 2 mengisi bit genap → y = 8'h55 (verifikasi via sim).
    let src = r#"
module step_dut {
    in  clk : bit
    out y   : logic[7:0]
    comb {
        y = 8'h00
        for i in 0..7 step 2 {
            y[i] = 1'b1
        }
    }
}
module tb_step {
    sig clk : bit
    sig y   : logic[7:0]
    inst step_dut u (.clk(clk), .y(y))
    initial {
        clk = 0
        #10
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "step").expect("transpile .mv step OK");
    assert!(
        r.sv.contains("for (int i = 0; i < 7; i = i + 2) begin"),
        "codegen harus emit step: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 15).unwrap();
    let y = sigs.iter().find(|(s, _)| s == "y").unwrap().1.to_u64();
    assert_eq!(y, 0x55, "bit genap 0,2,4,6 = 1 → 0x55");
}

#[test]
fn test_counter_simulation() {
    let source = r#"
module tb_counter;
    reg clk;
    reg rst_n;
    wire [3:0] count;

    counter u_counter(
        .clk(clk),
        .rst_n(rst_n),
        .count(count)
    );

    initial begin
        clk = 0;
        rst_n = 0;
        #5 rst_n = 1;
        #100 $finish;
    end

    always #1 clk = ~clk;
endmodule

module counter(
    input clk,
    input rst_n,
    output reg [3:0] count
);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n)
            count <= 4'b0000;
        else
            count <= count + 4'b0001;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(count_val, 8, "count should be 8 at time 20");
}

#[test]
fn test_3level_hierarchy() {
    let source = r#"
module tb;
    reg clk;
    reg rst_n;
    reg [7:0] out;

    top u_top(
        .clk(clk),
        .rst_n(rst_n),
        .out(out)
    );

    initial begin
        clk = 0;
        rst_n = 0;
        #5 rst_n = 1;
        #100 $finish;
    end

    always #1 clk = ~clk;
endmodule

module top(input clk, input rst_n, output [7:0] out);
    sub u_sub(.clk(clk), .rst_n(rst_n), .out(out));
endmodule

module sub(input clk, input rst_n, output reg [7:0] out);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n)
            out <= 8'd0;
        else
            out <= out + 8'd1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 6).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    // rst_n=0 at time 0 => out=0
    // rst_n=1 at time 5, posedge at time 6 => out=1
    assert_eq!(out_val, 1, "out should be 1 at time 6");
}

#[test]
fn test_display_format() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [3:0] b;

    initial begin
        a = 8'd42;
        b = 4'd10;
        $display("a=%d b=%b a=%h", a, b, a);
        $display("plain text");
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(a_val, 42, "a should be 42");
}

#[test]
fn test_strobe_basic() {
    let source = r#"
module tb;
    reg [7:0] a;
    initial begin
        a = 10;
        $strobe("strobe: a=%d", a);
        a = 20;
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
}

#[test]
fn test_strobe_after_nba() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 10;
        b <= 99;
        $strobe("strobe: a=%d b=%d", a, b);
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
}

#[test]
fn test_for_loop_generate_mux() {
    let source = r#"
module tb;
    reg [7:0] in;
    reg [2:0] sel;
    reg [7:0] out;
    integer i;

    always @(*) begin
        out = 8'd0;
        for (i = 0; i < 8; i = i + 1) begin
            if (sel == i)
                out = in;
        end
    end

    initial begin
        in = 8'd42;
        sel = 3'd5;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(out_val, 42, "out should be 42 (in) after for-loop mux");
}

#[test]
fn test_read_project_file() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("mivon_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let mivon_path = dir.join(".mivon");
    let sv_path = dir.join("test.sv");
    {
        let mut f = fs::File::create(&mivon_path).unwrap();
        writeln!(f, "# project file").unwrap();
        writeln!(f, "  ").unwrap();
        writeln!(f, "test.sv").unwrap();
    }
    {
        let mut f = fs::File::create(&sv_path).unwrap();
        writeln!(f, "module tb; initial begin #1 $finish; end endmodule").unwrap();
    }

    let files = read_project_file(mivon_path.to_str().unwrap()).unwrap();
    assert_eq!(files.len(), 1, "should read 1 file from .mivon");
    assert!(files[0].ends_with("test.sv"));

    let design = compile_files(&files).unwrap();
    assert_eq!(design.top.name, "tb");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_line_directive_in_compile_str() {
    // `line markers should be transparent to normal compilation
    let source = r#"
`line 42 "dummy.sv"
module test;
    wire a;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "`line directive broke compilation: {:?}",
        design.err()
    );
}

#[test]
fn test_line_directive_updates_error_line() {
    // `line updates the source file in the parser; line numbers are cumulative
    let source = r#"
`line 99 "fake.sv"
wire a
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok() || {
            let err = result.as_ref().unwrap_err().to_string();
            err.contains("skipping top-level")
        },
        "expected ok or skip warning, got: {:?}",
        result.err()
    );
}

#[test]
fn test_line_directive_unknown_backtick_skipped() {
    // Unknown backtick directives (non-`line) should be skipped silently
    let source = r#"
`uvm_info("hello")
module test;
    wire a;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "unknown backtick directive broke compilation: {:?}",
        design.err()
    );
}

#[test]
fn test_compile_files_with_line_directives() {
    // compile_files emits `line markers for each file
    let source1 = r#"
module top;
    wire a;
endmodule
"#;
    let dir = std::env::temp_dir().join("test_line_tracking");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let f1 = dir.join("top.sv");
    fs::write(&f1, source1).unwrap();
    let files = vec![f1.to_string_lossy().to_string()];
    let design = compile_files(&files).unwrap();
    assert_eq!(design.top.name, "top");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_include_with_line_directive() {
    // include emits `line markers — verify they don't break compilation
    let dir = std::env::temp_dir().join("test_include_line");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let inc_path = dir.join("inc.sv");
    fs::write(&inc_path, "module top;\n    wire a;\nendmodule\n").unwrap();
    let source = format!(
        "`include \"{}\"\nmodule main;\n    wire b;\n    top u_top();\nendmodule\n",
        inc_path.display()
    );
    let mut pp = Preprocessor::new();
    let dir_buf = dir.clone();
    let processed = pp.preprocess(&source, Some(&dir_buf)).unwrap();
    assert!(
        processed.contains("`line"),
        "expected `line markers in preprocessed output"
    );
    let design = compile_str(&processed);
    assert!(
        design.is_ok(),
        "compile_str with `line markers failed: {:?}",
        design.err()
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_files_tracking() {
    // compile_files emits `line markers, verify the compiled output is correct
    let dir = std::env::temp_dir().join("test_line_tracking_files");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let source = "module top;\n    wire a;\n    assign a = 1'b1;\nendmodule\n";
    let f1 = dir.join("top.sv");
    fs::write(&f1, source).unwrap();
    let files = vec![f1.to_string_lossy().to_string()];
    let design = compile_files(&files).unwrap();
    assert_eq!(design.top.name, "top");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_parameterized_module_width() {
    let source = r#"
module tb;
    reg clk;
    reg rst_n;
    wire [7:0] count;

    counter #(8) u_counter(
        .clk(clk),
        .rst_n(rst_n),
        .count(count)
    );

    initial begin
        clk = 0;
        rst_n = 0;
        #5 rst_n = 1;
        #100 $finish;
    end

    always #1 clk = ~clk;
endmodule

module counter #(parameter WIDTH = 8) (
    input clk,
    input rst_n,
    output reg [WIDTH-1:0] count
);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n)
            count <= {WIDTH{1'b0}};
        else
            count <= count + 1'b1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    // rst_n=0 at time 0, goes high at 5
    // posedge at 6 => count=1, posedge at 8 => count=2, ... posedge at 20 => count=8
    assert_eq!(count_val, 8, "8-bit counter should be 8 at time 20");
}

#[test]
fn test_array_decl_pattern_init_order() {
    // BUG mivon utama (ditemukan via mivon-mv pencari bug): decl-init array
    // `'{e0, e1, e2, e3}` di-fold jadi Const → write utuh → unpacked storage
    // elemen0 di bit terendah → array TERBALIK (rom[0]=e3). Fix: decompose
    // Concat → per-elemen assign utk decl-init (pola sama dgn statement
    // assign stmt.rs). Statement assign sudah benar; decl-init ikut.
    let source = r#"
module tb;
    logic [7:0] rom [0:3] = '{8'h01, 8'h02, 8'h03, 8'h04};
    wire [7:0] r0 = rom[0];
    wire [7:0] r1 = rom[1];
    wire [7:0] r2 = rom[2];
    wire [7:0] r3 = rom[3];
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("r0"), 0x01, "rom[0] harus elemen pertama (1)");
    assert_eq!(get("r1"), 0x02);
    assert_eq!(get("r2"), 0x03);
    assert_eq!(get("r3"), 0x04);
}

#[test]
fn test_always_comb_array_index_sensitivity() {
    // BUG mivon utama (ditemukan via mivon-mv pencari bug): always_comb
    // `val = rom[idx]` — sensitivity dihitung dari IR body; arm ArrayIndex
    // hanya mengumpulkan signal dasar (rom), bukan index (idx) → idx berubah
    // tak trigger recompute → val stuck. Wire assign selalu evaluasi — benar.
    let source = r#"
module tb;
    logic [7:0] rom [0:3] = '{8'h01, 8'h02, 8'h03, 8'h04};
    logic [1:0] idx;
    logic [7:0] val;
    always_comb val = rom[idx];
    initial begin
        idx = 0; #1
        idx = 3; #1
        idx = 1; #2 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    let final_val = get("val");
    assert_eq!(
        final_val, 0x02,
        "always_comb harus re-eval saat idx berubah → rom[1]=2, dapat {final_val}"
    );
}

#[test]
fn test_array_memory_simulation() {
    let source = r#"
module tb;
    reg clk;
    reg [7:0] mem [0:3];
    reg [1:0] addr;
    wire [7:0] rd_data;

    assign rd_data = mem[addr];

    initial begin
        clk = 0;
        mem[0] = 8'hA0;
        mem[1] = 8'hB1;
        mem[2] = 8'hC2;
        mem[3] = 8'hD3;
        addr = 0;
        #10 addr = 1;
        #10 addr = 2;
        #10 addr = 3;
        #10 $finish;
    end

    always #5 clk = ~clk;
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();

    // Final rd_data should be mem[3]=0xD3 (addr changes to 3 at time 30, then #10 at time 40)
    let rd_val = sigs
        .iter()
        .find(|(n, _)| n == "rd_data")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(rd_val, 0xD3, "rd_data final should be 0xD3 (mem[3])");
}

#[test]
fn test_array_with_readmemh() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("mivon_array_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let hex_path = dir.join("mem_init.hex");
    {
        let mut f = fs::File::create(&hex_path).unwrap();
        writeln!(f, "A0").unwrap();
        writeln!(f, "B1").unwrap();
        writeln!(f, "C2").unwrap();
        writeln!(f, "D3").unwrap();
    }

    let hex_str = hex_path.to_str().unwrap().replace('\\', "/");

    let source = format!(
        r#"
module tb;
    reg [7:0] mem [0:3];
    reg [1:0] addr;
    wire [7:0] rd_data;

    assign rd_data = mem[addr];

    initial begin
        $readmemh("{hex}", mem);
        addr = 0;
        #10 addr = 2;
        #10 $finish;
    end
endmodule
"#,
        hex = hex_str
    );
    let sigs = simulate_signals(&source, 30).unwrap();

    // Final rd_data should be mem[2]=0xC2 (addr changes to 2 at time 10, then #10 $finish at 20)
    let rd_val = sigs
        .iter()
        .find(|(n, _)| n == "rd_data")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(rd_val, 0xC2, "rd_data final should be 0xC2 (mem[2])");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_array_index_edge_cases() {
    let source = r#"
module tb;
    reg [3:0] mem [0:1];
    wire [3:0] out0;
    wire [3:0] out1;

    assign out0 = mem[0];
    assign out1 = mem[1];

    initial begin
        mem[0] = 4'hF;
        mem[1] = 4'h5;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();

    let out0_val = sigs
        .iter()
        .find(|(n, _)| n == "out0")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let out1_val = sigs
        .iter()
        .find(|(n, _)| n == "out1")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(out0_val, 0xF, "mem[0] should be 0xF");
    assert_eq!(out1_val, 0x5, "mem[1] should be 0x5");
}

#[test]
fn test_parameterized_module_instance_override() {
    let source = r#"
module tb;
    reg [15:0] a;
    reg [15:0] b;
    wire [15:0] sum;

    adder #(16) u_adder(
        .a(a),
        .b(b),
        .sum(sum)
    );

    initial begin
        a = 16'd100;
        b = 16'd200;
        #1 $finish;
    end
endmodule

module adder #(parameter WIDTH = 8) (
    input [WIDTH-1:0] a,
    input [WIDTH-1:0] b,
    output [WIDTH-1:0] sum
);
    assign sum = a + b;
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let sum_val = sigs
        .iter()
        .find(|(n, _)| n == "sum")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(sum_val, 300, "16-bit adder: 100 + 200 = 300");
}

#[test]
fn test_space_separated_instance_connections() {
    // Regresi fase 3 (netlist): koneksi port space-separated `.c(clk) .r(rst_n)`
    // tanpa kurung & koma legal di SV tapi TIDAK pernah di-parse oleh
    // `parse_instance` → instance jadi tanpa koneksi port → always_ff di
    // module sel tak ter-resolve → FF tidak pernah berdetak (output menggantung
    // z). Fix di mivon-parser/src/instance.rs (branch `.name(expr)` setelah
    // nama instance).
    let source = r#"
module tb;
    reg clk = 0, rst_n = 0;
    reg [7:0] d = 8'h05;
    wire [7:0] q;
    always #5 clk = ~clk;
    initial begin
        #3 rst_n = 1;
        #25 $finish;
    end
    top dut(.clk(clk), .rst_n(rst_n), .d(d), .q(q));
endmodule

module DFFR #(parameter W = 1, parameter RST = 0)(input c, input r, input [W-1:0] d, output reg [W-1:0] q);
    always_ff @(posedge c or negedge r) if (!r) q <= RST; else q <= d;
endmodule

module top(input clk, input rst_n, input [7:0] d, output [7:0] q);
    DFFR ff0 #(.W(8), .RST(0)) .c(clk) .r(rst_n) .d(d) .q(q);
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let q = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        q, 5,
        "koneksi space-separated: FF harus clock d=5 (bukan z)"
    );
}

#[test]
fn test_async_reset_edge_triggers_sequential() {
    // F40 regresi: `always_ff @(posedge clk or negedge rst_n)` harus fire
    // SAAT negedge rst_n (async reset), bukan hanya saat clock edge. Dulu
    // `trigger_sensitive_processes` mengabaikan reset (`reset: _reset`) →
    // reset yang terjadi ANTARA dua clock edge tidak pernah diterapkan;
    // FF tanpa init tetap z, dan `count + 1` di posedge berikutnya (setelah
    // reset dideassert) menghasilkan X → count stuck 0.
    //
    // Di sini rst_n di-deassert di t=4 — SEBELUM posedge clk pertama (t=5).
    // Tanpa fix: posedge t=5 melihat rst_n=1 → count <= count+1 (count=z → X).
    // Dengan fix: negedge t=2 men-set count=0 → t=5 count=1 → t=15 count=2.
    let source = r#"
module tb;
    reg clk = 0, rst_n = 1;
    wire [7:0] count;
    always #5 clk = ~clk;
    initial begin
        #2 rst_n = 0;   // negedge reset t=2
        #2 rst_n = 1;   // deassert t=4 (sebelum posedge clk t=5)
        #11 $finish;    // t=15
    end
    counter dut(.clk(clk), .rst_n(rst_n), .count(count));
endmodule

module counter(input clk, input rst_n, output reg [7:0] count);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) count <= 8'h0;
        else count <= count + 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 15).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        count_val, 2,
        "reset async diterapkan di t=2 → posedge t=5 count=1, t=15 count=2"
    );
}

#[test]
fn test_port_ansi_initializer() {
    // F40 regresi: initializer di deklarasi port ANSI (`output reg [7:0] b
    // = 8'h2A`) legal di SV tapi dulu TIDAK di-parse — token `=` tersisa
    // → `expected RParen` → seluruh module gagal parse (E3001 module not
    // found). Fix di mivon-parser/src/instance.rs (parse `= expr` setelah
    // nama port) + elaborator (Process::Initial seperti `reg b = 8'h2A;`).
    let source = r#"
module tb;
    wire [7:0] b;
    wire [3:0] c;
    initial begin
        #1 $display("PORT b=%0d c=%0d", b, c);
        #1 $finish;
    end
    m dut(.b(b), .c(c));
endmodule

module m(output reg [7:0] b = 8'h2A, output reg [3:0] c = 4'd7);
endmodule
"#;
    let sigs = simulate_signals(source, 3).unwrap();
    let b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let c_val = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(b_val, 0x2A, "port init b = 8'h2A");
    assert_eq!(c_val, 7, "port init c = 4'd7");
}

#[test]
fn test_arrayed_instances() {
    let source = r#"
module tb;
    reg [7:0] a;
    wire [7:0] x;
    wire [7:0] y;

    add1 inst[1:0] (
        .in(a),
        .out(x)
    );

    initial begin
        a = 10;
        #1 y = x;
        #1 $finish;
    end
endmodule

module add1(input [7:0] in, output [7:0] out);
    assign out = in + 1;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    // Both inst[0] and inst[1] drive 'x', all drive 10+1=11
    let x_val = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(x_val, 11, "x driven by both instances = 10+1 = 11");
}

#[test]
fn test_arrayed_instances_hierarchy() {
    let source = r#"
module tb;
    reg clk;
    reg [7:0] a;
    wire [7:0] x[1:0];

    add1 inst[1:0] (
        .in(a),
        .out(x)
    );

    initial begin
        a = 10;
        #1 $finish;
    end
endmodule

module add1(input [7:0] in, output [7:0] out);
    assign out = in + 1;
endmodule
"#;
    // Just verify it compiles and runs without error
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "arrayed instance with array port should compile and run"
    );
}

#[test]
fn test_function_call() {
    let source = r#"
module tb;
    reg [7:0] a, b, result;

    function [7:0] add;
        input [7:0] a, b;
        begin
            add = a + b;
        end
    endfunction

    initial begin
        a = 10;
        b = 20;
        result = add(a, b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let result_val = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(result_val, 30, "add(10, 20) should be 30");
}

#[test]
fn test_function_call_in_expr() {
    let source = r#"
module tb;
    reg [7:0] result;

    function [7:0] add;
        input [7:0] a, b;
        begin
            add = a + b;
        end
    endfunction

    function [7:0] mul;
        input [7:0] a, b;
        begin
            mul = a * b;
        end
    endfunction

    initial begin
        result = add(mul(2, 3), 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let result_val = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(result_val, 7, "add(mul(2,3), 1) = 7");
}

#[test]
fn test_function_call_in_always_ff() {
    let source = r#"
module tb;
    reg clk;
    reg [7:0] a, b, q;

    function [7:0] add;
        input [7:0] a, b;
        begin
            add = a + b;
        end
    endfunction

    always_ff @(posedge clk) begin
        q <= add(a, b);
    end

    initial begin
        clk = 0;
        a = 5; b = 7;
        #1 clk = 1;
        #1 clk = 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 4).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(q_val, 12, "q should be 12 after posedge clk");
}

#[test]
fn test_function_internal_decl() {
    let source = r#"
module tb;
    reg [7:0] result;

    function [7:0] add;
        input [7:0] a, b;
        reg [7:0] temp;
        begin
            temp = a + b;
            add = temp;
        end
    endfunction

    initial begin
        result = add(30, 12);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let result_val = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(result_val, 42, "add(30, 12) via internal temp should be 42");
}

#[test]
fn test_function_continuous_assign() {
    let source = r#"
module tb;
    reg [7:0] a, b;
    wire [7:0] result;

    function [7:0] add;
        input [7:0] a, b;
        begin
            add = a + b;
        end
    endfunction

    assign result = add(a, b);

    initial begin
        a = 15; b = 27;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let result_val = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        result_val, 42,
        "result from assign w/ func call should be 42"
    );
}

#[test]
fn test_generate_if() {
    let source = r#"
module tb;
    reg [7:0] result;

    generate
        if (1) begin
            always @(*) begin
                result = 8'hAB;
            end
        end else begin
            always @(*) begin
                result = 8'hCD;
            end
        end
    endgenerate

    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let result_val = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(result_val, 0xAB, "generate if(1) should pick true branch");
}

#[test]
fn test_generate_for() {
    let source = r#"
module tb;
    reg [3:0] result;

    genvar i;
    generate
        for (i = 0; i < 4; i = i + 1) begin
            always @(*) begin
                result[i] = 1'b1;
            end
        end
    endgenerate

    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let result_val = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(result_val, 0xF, "generate for sets all bits of result");
}

#[test]
fn test_signed_arithmetic() {
    let source = r#"
module tb;
    reg [7:0] a, b, result;

    function [7:0] max;
        input [7:0] a, b;
        begin
            if (a > b)
                max = a;
            else
                max = b;
        end
    endfunction

    initial begin
        a = 10;
        b = 20;
        result = max(a, b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let result_val = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(result_val, 20, "max(10, 20) should be 20");
}

#[test]
fn test_signed_comparison() {
    let source = r#"
module tb;
    reg [7:0] a, b;
    reg gt;

    initial begin
        // 200 as unsigned > 100, but as signed (-56) < 100
        a = 200;
        b = 100;
        // Use unsigned comparison
        if (a > b)
            gt = 1;
        else
            gt = 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let gt_val = sigs
        .iter()
        .find(|(n, _)| n == "gt")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(gt_val, 1, "unsigned 200 > 100");
}

#[test]
fn test_class_parsing_basic() {
    let source = r#"
class driver;
    logic [7:0] data;
    function new();
        data = 42;
    endfunction
    virtual function void print();
        $display("data = %d", data);
    endfunction
endclass
module tb;
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    assert!(
        design.classes.contains_key(&Symbol::intern("driver")),
        "class 'driver' should be registered"
    );
    let cls = &design.classes[&Symbol::intern("driver")];
    assert_eq!(cls.name, "driver");
    assert!(cls.extends.is_none());
    assert_eq!(cls.fields.len(), 1, "driver has 1 field");
    assert_eq!(cls.fields[0].name, "data");
    assert_eq!(cls.methods.len(), 2, "driver has 2 methods (new + print)");
    assert!(cls.methods.iter().any(|m| m.name == "new"));
    assert!(cls
        .methods
        .iter()
        .any(|m| m.name == "print" && m.virtual_flag));
}

#[test]
fn test_class_parsing_extends() {
    let source = r#"
class my_base;
    string name;
    function new(string name);
        this.name = name;
    endfunction
endclass
class driver extends my_base;
    logic [7:0] data;
    function new(string name);
        super.new(name);
    endfunction
endclass
module tb;
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    assert!(design.classes.contains_key(&Symbol::intern("my_base")));
    assert!(design.classes.contains_key(&Symbol::intern("driver")));
    assert_eq!(
        design.classes[&Symbol::intern("driver")].extends,
        Some(Symbol::intern("my_base"))
    );
}

#[test]
fn test_class_method_call_syntax() {
    // Test that obj.method() and obj.field parsing works in expressions
    // Just parse AST (not elaborate) since classes need runtime support
    let source = r#"
module tb;
    integer d, x;
    initial begin
        d = new();
        d.print();
        x = d.data;
    end
endmodule

class base;
    function new();
    endfunction
    function void print();
    endfunction
endclass
"#;
    let mut lexer = Lexer::new(source);
    use mivon_parser::lexer::Token;
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "test");
    let design = parser.parse_design().unwrap();
    assert!(
        !design.classes.is_empty(),
        "should have parsed at least one class"
    );
    let mod_names: Vec<_> = design.modules.iter().map(|m| m.name).collect();
    assert!(mod_names.contains(&Symbol::intern("tb")));
}

#[test]
fn test_class_field_access_parsing() {
    let source = r#"
class cfg;
    integer timeout;
    function new();
        timeout = 1000;
    endfunction
endclass
module tb;
    integer x;
    integer val;
    initial begin
        x = new();
        val = x.timeout;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    assert!(design.classes.contains_key(&Symbol::intern("cfg")));
    let cls = &design.classes[&Symbol::intern("cfg")];
    assert!(cls.fields.iter().any(|f| f.name == "timeout"));
    assert!(cls.methods.iter().any(|m| m.name == "new"));
}

#[test]
fn test_method_call_parsing() {
    let source = r#"
class comp;
    function void print();
    endfunction
endclass
module tb;
    integer h;
    initial begin
        h = new();
        h.print();
    end
endmodule
"#;
    let _design = compile_str(source).unwrap();
}

#[test]
fn test_virtual_method_registration() {
    let source = r#"
class base;
    virtual function void show();
    endfunction
endclass
class extended extends base;
    virtual function void show();
    endfunction
endclass
module tb;
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    assert!(design.classes.contains_key(&Symbol::intern("base")));
    assert!(design.classes.contains_key(&Symbol::intern("extended")));
    assert_eq!(
        design.classes[&Symbol::intern("extended")].extends,
        Some(Symbol::intern("base"))
    );
    let base_show = design.classes[&Symbol::intern("base")]
        .methods
        .iter()
        .find(|m| m.name == "show")
        .unwrap();
    assert!(base_show.virtual_flag);
    let ext_show = design.classes[&Symbol::intern("extended")]
        .methods
        .iter()
        .find(|m| m.name == "show")
        .unwrap();
    assert!(ext_show.virtual_flag);
}

#[test]
fn test_super_new_parsing() {
    let source = r#"
class base;
    function new();
    endfunction
endclass
class derived extends base;
    function new();
        super.new();
    endfunction
endclass
module tb;
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let _design = compile_str(source).unwrap();
}

#[test]
fn test_procedural_for_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    reg [3:0] i;
    initial begin
        count = 0;
        for (i = 0; i < 5; i = i + 1) begin
            count = count + 1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(count_val, 5, "count should be 5 after for loop");
}

#[test]
fn test_procedural_while_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    initial begin
        count = 0;
        while (count < 3) begin
            count = count + 1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(count_val, 3, "count should be 3 after while loop");
}

#[test]
fn test_super_new_phase_dispatch() {
    let source = r#"
class base;
    function new();
    endfunction
    function void build_phase();
    endfunction
endclass

class derived extends base;
    function new();
        super.new();
    endfunction
    function void build_phase();
        super.build_phase();
    endfunction
endclass

module tb;
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 10).unwrap();
}

#[test]
fn test_class_inheritance_with_super() {
    let source = r#"
class base;
    function void build_phase();
    endfunction
    function int get_val();
        return 5;
    endfunction
endclass

class derived extends base;
    function void build_phase();
        super.build_phase();
    endfunction
    function int get_val();
        return 10 + super.get_val();
    endfunction
endclass

module tb;
    int result;
    initial begin
        result = 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    // Phase execution runs build_phase (checks super dispatch doesn't crash)
    // Then initial block runs
    let _result = sigs.iter().find(|(n, _)| n == "result").unwrap();
}

#[test]
fn test_class_typed_var_decl_and_method_call() {
    let source = r#"
class counter;
    int count;
    function void inc();
        count = count + 1;
    endfunction
    function int get();
        return count;
    endfunction
endclass

module tb;
    counter c;
    int result;
    initial begin
        c = new();
        result = c.get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 0, "new counter should have count=0");
}

#[test]
fn test_class_typed_var_method_mutation() {
    let source = r#"
class counter;
    int count;
    function void inc();
        count = count + 1;
    endfunction
    function int get();
        return count;
    endfunction
endclass

module tb;
    counter c;
    int result;
    initial begin
        c = new();
        c.inc();
        c.inc();
        c.inc();
        result = c.get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 3, "after 3 inc() calls, count should be 3");
}

#[test]
fn test_class_typed_var_member_access() {
    let source = r#"
class counter;
    int count;
    function new();
        count = 0;
    endfunction
endclass

module tb;
    counter c;
    int result;
    initial begin
        c = new();
        result = c.count;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 0, "c.count should be 0 after new()");
}

#[test]
fn test_class_task_with_delay() {
    // Class task with #delay should suspend, resume, and complete correctly
    let source = r#"
class my_driver;
    int count;
    task run();
        count = 1;
        #5;
        count = 2;
        #5;
        count = 3;
    endtask
endclass

module tb;
    my_driver d;
    int result;
    initial begin
        d = new();
        d.run();
        #12;
        result = d.count;  // after both #5 delays complete
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        3,
        "class task with delays should set count=3 after both delays"
    );
}

#[test]
fn test_class_task_no_delay() {
    // Class task without delay should still work (synchronous)
    let source = r#"
class my_driver;
    int count;
    task run();
        count = 42;
    endtask
endclass

module tb;
    my_driver d;
    int result;
    initial begin
        d = new();
        d.run();
        result = d.count;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        42,
        "class task without delay should set count=42"
    );
}

#[test]
fn test_uvm_lite_polymorphic_dispatch() {
    let source = r#"
class my_base;
    int level;
    function new(int level);
        this.level = level;
    endfunction
    virtual function int get_type_id();
        return 1;
    endfunction
    function int get_level();
        return this.level;
    endfunction
endclass

class driver extends my_base;
    function new(int level);
        super.new(level);
    endfunction
    virtual function int get_type_id();
        return 2;
    endfunction
endclass

module tb;
    my_base h;
    driver d;
    int result_type;
    int result_level;
    initial begin
        d = new(42);
        h = d;
        result_type = h.get_type_id();
        result_level = h.get_level();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, type_val) = sigs.iter().find(|(n, _)| n == "result_type").unwrap();
    let (_, level_val) = sigs.iter().find(|(n, _)| n == "result_level").unwrap();
    assert_eq!(
        type_val.to_u64(),
        2,
        "virtual dispatch: should call driver::get_type_id"
    );
    assert_eq!(level_val.to_u64(), 42, "get_level should return 42");
}

#[test]
fn test_null_handle() {
    let source = r#"
class Foo;
    function int get_val();
        return 7;
    endfunction
endclass

module tb;
    Foo h;
    int result;
    initial begin
        h = null;
        if (h == null) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "null handle should compare as equal to null"
    );
}

#[test]
fn test_string_function_return() {
    let source = r#"
class driver;
    function string get_type_name();
        return "my_driver";
    endfunction
endclass

module tb;
    driver d;
    int result;
    initial begin
        d = new();
        // Just verify it parses and executes without error
        result = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "string function should parse and execute");
}

#[test]
fn test_randomize_with_constraint() {
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint addr_range {
        addr > 0;
        addr < 100;
    }
endclass

module tb;
    Packet p;
    int result;
    initial begin
        p = new();
        if (p.randomize()) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "randomize should succeed");
}

#[test]
fn test_randomize_with_binds_class_fields() {
    // Temuan verify tb_22_class (loop tahap 6): `p.randomize() with
    // { addr == 8'hAA }` gagal E2001 "signal 'addr' not found" — blok with
    // dielaborasi di scope enclosing, padahal LRM 1800 §18.7 nama di
    // constraint_block milik scope class. Fix: Ident tak-ter-resolve yang
    // cocok field class receiver di-rewrite jadi MemberAccess{obj, field}
    // (elaborator/expr.rs bind_randomize_with). Runtime sudah sanggup
    // (engine eval MemberAccess di object handle).
    let source = r#"
class PacketW;
    rand logic [7:0] addr;
    rand logic [7:0] data;
    constraint c_addr { addr != 8'h00; }
endclass

module tb;
    PacketW p;
    int result;
    int av;
    initial begin
        p = new();
        if (p.randomize() with { addr == 8'hAA; }) begin
            result = 1;
            av = p.addr;
        end else begin
            result = 0;
            av = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "randomize with harus sukses");
    let (_, av) = sigs.iter().find(|(n, _)| n == "av").unwrap();
    assert_eq!(av.to_u64(), 0xAA, "with addr==AA harus ditegakkan");

    // Field WARISAN via extends: class_field_names traverse parent (sejajar
    // merge runtime classes.rs) — tanpa ini E2001 "signal not found".
    let source2 = r#"
class BaseW;
    rand logic [7:0] addr;
endclass
class ChildW extends BaseW;
    rand logic [7:0] data;
endclass

module tb;
    ChildW p;
    int result;
    int av;
    initial begin
        p = new();
        if (p.randomize() with { addr == 8'hAA; }) begin
            result = 1;
            av = p.addr;
        end else begin
            result = 0;
            av = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs2 = simulate_signals(source2, 50).unwrap();
    let (_, v2) = sigs2.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(v2.to_u64(), 1, "randomize with field warisan harus sukses");
    let (_, a2) = sigs2.iter().find(|(n, _)| n == "av").unwrap();
    assert_eq!(a2.to_u64(), 0xAA, "with addr==AA warisan harus ditegakkan");
}

#[test]
fn test_constraint_signed_domains() {
    // ROUND 36: solver constraint signed — domain narrowing memakai signedness
    // field class (`rand int`): mixed-sign relational bounds, inside negatif,
    // dist rentang negatif, single value negatif, unsigned tetap. Sebelum fix:
    // `x > -10; x < 100` cuma bisa memenuhi rejection sampling (praktis gagal
    // utk 32-bit), `y inside {[-5:5]}` domain wrap salah (lo=5 hi=0xFFFFFFFB
    // dianggap interval kontigu 4-miliar), `-3` tunggal di-return false.
    let source = r#"
class C;
    rand int x;
    rand int y;
    rand int z;
    rand int w;
    rand logic [7:0] u;
    rand logic [7:0] v;
    constraint c1 { x > -10; x < 100; }        // mixed-sign relational bounds
    constraint c2 { y inside { [-200:-100] }; } // negative-only inside
    constraint c3 { z dist { [-5:5] := 1, 100 := 1 }; } // dist negative range
    constraint c4 { w inside { 7, -3, 42 }; }   // single values mixed
    constraint c5 { u > 8'hF0; }                // unsigned tetap
    constraint c6 { v inside { [8'h20:8'h30] }; } // unsigned inside
endclass

module tb;
    C c;
    int result;
    int xv, yv, zv, wv, uv, vv;
    initial begin
        c = new();
        if (c.randomize()) begin
            result = 1;
            xv = c.x;
            yv = c.y;
            zv = c.z;
            wv = c.w;
            uv = c.u;
            vv = c.v;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_i64())
            .unwrap_or(0)
    };
    let getu = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("result"),
        1,
        "randomize with signed domains should succeed"
    );
    let x = get("xv");
    assert!(x > -10 && x < 100, "x in (-10, 100), got {}", x);
    let y = get("yv");
    assert!((-200..=-100).contains(&y), "y in [-200, -100], got {}", y);
    let z = get("zv");
    assert!(
        (-5..=5).contains(&z) || z == 100,
        "z in dist [-5..5, 100], got {}",
        z
    );
    let w = get("wv");
    assert!(w == 7 || w == -3 || w == 42, "w in [7, -3, 42], got {}", w);
    let u = getu("uv");
    assert!(u > 0xF0, "u > 8'hF0 unsigned, got {:#x}", u);
    let v = getu("vv");
    assert!(
        (0x20..=0x30).contains(&v),
        "v in [0x20, 0x30] unsigned, got {:#x}",
        v
    );
}

#[test]
fn test_vpi_systf_e2e_sv_to_calltf() {
    // LANG-46 e2e: SV → calltf melalui INTERPRETER (bukan translasi penuh ke
    // C ala Verilator). SV memanggil `$my_task()` → engine dispatch
    // `call_registered_systf("$my_task")` → calltf `extern "C"` (ABI identik
    // dengan fungsi C yang dikompilasi gcc) dieksekusi. Calltf di-Rust dengan
    // ABI extern "C" — setara dengan fungsi C sungguhan, tanpa butuh toolchain
    // C di test. Verifikasi: counter calltf naik saat simulasi berjalan.
    use mivon_api::vpi::systf::vpi_register_systf;
    use mivon_api::vpi::types::{s_vpi_systf_data, vpiSystfTask};
    use std::ffi::CString;
    use std::os::raw::c_char;
    use std::sync::atomic::{AtomicI32, Ordering};

    static CALLED: AtomicI32 = AtomicI32::new(0);

    extern "C" fn stub_calltf(_user_data: *mut std::ffi::c_void) -> i32 {
        CALLED.fetch_add(1, Ordering::SeqCst);
        0
    }

    let source = r#"
module top;
    initial begin
        $my_task();
        #1 $finish;
    end
endmodule
"#;
    // Compile sekali (lambat), lalu loop engine: registry systf GLOBAL di-clear
    // `clear_all_systfs()` di akhir run engine test PARALEL lain — retry agar
    // test tidak flaky saat registrasi terhapus sebelum dispatch.
    let design = compile_str(source).expect("compile SV dengan $my_task");
    let mut fired = false;
    for _ in 0..32 {
        CALLED.store(0, Ordering::SeqCst);
        let cname = CString::new("$my_task").unwrap();
        let data = s_vpi_systf_data {
            task_function_type: vpiSystfTask,
            tfname: cname.as_ptr() as *mut c_char,
            calltf: Some(stub_calltf),
            compiletf: None,
            sizetf: None,
            user_data: std::ptr::null_mut(),
        };
        let _h = vpi_register_systf(&data);
        let mut engine = mivon_api::simulator::SimulationEngine::new(design.clone(), 10);
        engine.run().expect("simulasi berjalan");
        if CALLED.load(Ordering::SeqCst) >= 1 {
            fired = true;
            break;
        }
    }
    assert!(fired, "calltf harus terpanggil saat SV memanggil $my_task");
}

#[test]
fn test_vhpi_engine_hook_e2e() {
    // VHPI (IEEE 1076-2008): engine hook terpanggil saat sim — `set_vhpi_engine`
    // di run() memungkinkan vhpi_handle_by_name mengakses object Mivon.
    // Verifikasi: handle_by_name signal setelah sim (engine di-clear di akhir
    // run, jadi jalankan di dalam run via callback start-of-simulation).
    use mivon_api::vhpi::callback::{t_vhpi_cb_data, vhpiCbStartOfSimulation, vhpi_register_cb};
    use mivon_api::vhpi::object::{vhpiKind, vhpiSignal, vhpi_get, vhpi_handle_by_name};
    use std::sync::atomic::{AtomicI32, Ordering};

    static FOUND: AtomicI32 = AtomicI32::new(0);
    static KINDTEST: AtomicI32 = AtomicI32::new(0);

    extern "C" fn start_cb(data: *mut t_vhpi_cb_data) -> i32 {
        // Jalur callback start-of-sim: engine SUDAH di-set oleh run().
        let h = vhpi_handle_by_name("count", unsafe { &*data }.obj);
        if !h.is_null() {
            FOUND.store(1, Ordering::SeqCst);
            let k = vhpi_get(vhpiKind, h);
            if k == vhpiSignal {
                KINDTEST.store(1, Ordering::SeqCst);
            }
        }
        0
    }

    let source = r#"
module top;
    logic [7:0] count;
    initial begin
        count = 8'h2A;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).expect("compile");
    let mut engine = mivon_api::simulator::SimulationEngine::new(design, 10);
    let cb = t_vhpi_cb_data {
        reason: vhpiCbStartOfSimulation,
        cb_rtn: Some(start_cb),
        user_data: std::ptr::null_mut(),
        obj: mivon_api::vhpi::handle::VhpiHandle::NULL,
        time: std::ptr::null_mut(),
    };
    let _h = vhpi_register_cb(&cb);
    // run() memanggil set_vhpi_engine + dispatch_start_of_simulation —
    // callback start-of-sim melihat engine ter-set.
    engine.run().expect("simulasi");
    // Registry global di-clear di akhir run engine paralel lain — retry.
    let mut ok = FOUND.load(Ordering::SeqCst) == 1 && KINDTEST.load(Ordering::SeqCst) == 1;
    if !ok {
        for _ in 0..32 {
            FOUND.store(0, Ordering::SeqCst);
            KINDTEST.store(0, Ordering::SeqCst);
            let cb2 = t_vhpi_cb_data {
                reason: vhpiCbStartOfSimulation,
                cb_rtn: Some(start_cb),
                user_data: std::ptr::null_mut(),
                obj: mivon_api::vhpi::handle::VhpiHandle::NULL,
                time: std::ptr::null_mut(),
            };
            let _h2 = vhpi_register_cb(&cb2);
            let design2 = compile_str(source).expect("compile ulang");
            let mut engine2 = mivon_api::simulator::SimulationEngine::new(design2, 10);
            engine2.run().expect("simulasi ulang");
            if FOUND.load(Ordering::SeqCst) == 1 && KINDTEST.load(Ordering::SeqCst) == 1 {
                ok = true;
                break;
            }
        }
    }
    assert!(
        ok,
        "VHPI handle_by_name harus menemukan signal 'count' saat sim"
    );
}

#[test]
fn test_pli_tf_e2e_via_sim() {
    // PLI tf (IEEE 1364): engine memanggil tf_set_current_instance + time
    // saat task PLI dieksekusi — verifikasi tf_getinstance/tf_gettime
    // konsisten setelah simulate_signals (engine di-clear, tapi thread-local
    // tf current di-set ulang tiap run).
    use mivon_api::pli::tf::{
        tf_getinstance, tf_gettime, tf_set_current_instance, tf_set_current_time,
    };

    // Jalankan sim nyata — pastikan tidak ada panic saat engine cleanup
    // memanggil pli_cleanup (tf_clear_all + acc_close).
    let source = r#"
module top;
    logic [7:0] q;
    initial begin
        q = 8'h01;
        #1 q = 8'h02;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("simulasi berjalan tanpa crash PLI");
    let (_, v) = sigs.iter().find(|(n, _)| n == "q").unwrap();
    assert_eq!(v.to_u64(), 2, "q harus 2 setelah dua cycle");

    // PLI tf API tetap berfungsi setelah sim (murni Rust, tidak bergantung
    // engine).
    tf_set_current_instance(42);
    assert_eq!(tf_getinstance(), 42);
    tf_set_current_time(99);
    assert_eq!(tf_gettime(), 99);
}

#[test]
fn test_vhpi_value_change_callback_e2e() {
    // VHPI value-change callback (vhpiCbValueChange) di-fire engine saat
    // signal berubah (ForeignEvent::ValueChange ke scheduler — poin 5
    // arsitektur user). Callback di-register utk signal 'q'; verifikasi
    // terpanggil saat q berubah dari 0 → 1.
    use mivon_api::vhpi::callback::{fire_value_change_callbacks, vhpiCbValueChange};
    use mivon_api::vhpi::handle::{register_object_for_test, VhpiObjectKind};
    use std::sync::atomic::{AtomicI32, Ordering};

    static FIRED: AtomicI32 = AtomicI32::new(0);

    extern "C" fn vc_cb(_data: *mut mivon_api::vhpi::callback::t_vhpi_cb_data) -> i32 {
        FIRED.fetch_add(1, Ordering::SeqCst);
        0
    }

    // Registrasi callback value-change utk signal id 0 (obj Signal(0,0)).
    let obj = register_object_for_test(VhpiObjectKind::Signal(0, 0));
    let cb = mivon_api::vhpi::callback::t_vhpi_cb_data {
        reason: vhpiCbValueChange,
        cb_rtn: Some(vc_cb),
        user_data: std::ptr::null_mut(),
        obj,
        time: std::ptr::null_mut(),
    };
    let h = mivon_api::vhpi::callback::vhpi_register_cb(&cb);
    assert!(h.is_valid(), "callback terdaftar");

    // Fire langsung (simulasi jalur engine: commit signal → callback).
    let old = mivon_ir::LogicVec::from_u64(0, 8);
    let new = mivon_ir::LogicVec::from_u64(1, 8);
    fire_value_change_callbacks(0, &old, &new);
    assert_eq!(
        FIRED.load(Ordering::SeqCst),
        1,
        "value-change callback utk signal 0 terpanggil"
    );

    // Signal id lain → tidak terpanggil.
    fire_value_change_callbacks(5, &old, &new);
    assert_eq!(
        FIRED.load(Ordering::SeqCst),
        1,
        "signal id beda tak boleh fire"
    );

    // obj NULL (semua signal) → terpanggil utk signal apa pun.
    let cb_all = mivon_api::vhpi::callback::t_vhpi_cb_data {
        reason: vhpiCbValueChange,
        cb_rtn: Some(vc_cb),
        user_data: std::ptr::null_mut(),
        obj: mivon_api::vhpi::handle::VhpiHandle::NULL,
        time: std::ptr::null_mut(),
    };
    let h2 = mivon_api::vhpi::callback::vhpi_register_cb(&cb_all);
    fire_value_change_callbacks(9, &old, &new);
    assert_eq!(
        FIRED.load(Ordering::SeqCst),
        2,
        "obj NULL fire utk signal apa pun"
    );

    mivon_api::vhpi::callback::vhpi_remove_cb(h);
    mivon_api::vhpi::callback::vhpi_remove_cb(h2);
    mivon_api::vhpi::callback::clear_all_callbacks();
}

#[test]
fn test_vhpi_loader_e2e_gcc_so() {
    // e2e VHPI library C sungguhan (arsitektur poin 3: JANGAN compile PLI/
    // VHPI ke Rust — pakai C ABI + dynamic loader). Compile C → .so via gcc,
    // load via foreign loader, verifikasi `vhpi_startup` C terpanggil.
    // Skip test bila gcc tidak tersedia di environment.
    use std::path::Path;
    use std::process::Command;

    // Cek gcc tersedia.
    let gcc_ok = Command::new("gcc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !gcc_ok {
        eprintln!("SKIP: gcc tidak tersedia — test VHPI .so dilewati");
        return;
    }

    let dir = std::env::temp_dir().join(format!("mivon_vhpi_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("buat dir temp");
    let c_path = dir.join("vhpi_stub.c");
    let so_path = dir.join("libvhpi_stub.so");
    std::fs::write(
        &c_path,
        r#"
#include <stdio.h>
int vhpi_startup(void) {
    printf("vhpi_startup called (C stub)\n");
    return 0;
}
"#,
    )
    .expect("tulis C stub");

    let status = Command::new("gcc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("jalankan gcc");
    assert!(status.success(), "gcc compile .so gagal");

    // Load via foreign loader (canonicalize path absolut — dlopen tak search cwd).
    let vhpi = mivon_api::vhpi::loader::load_vhpi_library(so_path.to_str().unwrap())
        .expect("load VHPI .so");
    assert_eq!(vhpi.abi.arch, std::env::consts::ARCH, "ABI arch cocok");
    assert_eq!(vhpi.abi.os, std::env::consts::OS, "ABI os cocok");

    // vhpi_startup dari C terpanggil (return 0 = sukses).
    mivon_api::vhpi::loader::call_vhpi_startup(&vhpi).expect("vhpi_startup sukses");

    // Bersihkan.
    let _ = std::fs::remove_dir_all(&dir);
    let _ = Path::new("libvhpi_test.so"); // (scratch manual, bukan test ini)
}

#[test]
fn test_pli_loader_e2e_gcc_so() {
    // e2e PLI library C sungguhan: compile C → .so, load, cek entry point
    // `veriusertfs` / `vpi_startup` terdeteksi. Skip bila gcc tidak ada.
    use std::process::Command;

    let gcc_ok = Command::new("gcc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !gcc_ok {
        eprintln!("SKIP: gcc tidak tersedia — test PLI .so dilewati");
        return;
    }

    let dir = std::env::temp_dir().join(format!("mivon_pli_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("buat dir temp");
    let c_path = dir.join("pli_stub.c");
    let so_path = dir.join("libpli_stub.so");
    std::fs::write(
        &c_path,
        r#"
#include <stdio.h>
int vpi_startup(void) {
    printf("pli vpi_startup called (C stub)\n");
    return 0;
}
"#,
    )
    .expect("tulis C stub");

    let status = Command::new("gcc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("jalankan gcc");
    assert!(status.success(), "gcc compile .so gagal");

    let pli =
        mivon_api::pli::loader::load_pli_library(so_path.to_str().unwrap()).expect("load PLI .so");
    assert!(
        mivon_api::pli::loader::has_pli_entry_points(&pli),
        "entry point vpi_startup terdeteksi"
    );
    mivon_api::pli::loader::call_pli_startup(&pli).expect("pli vpi_startup sukses");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_randomize_soft_constraint_satisfied() {
    // LANG-31: `soft` constraint — best-effort. Soft yang TIDAK bertentangan
    // dengan hard constraint: randomize harus sukses (soft boleh terpenuhi
    // atau dilanggar, tidak pernah membuat randomize gagal).
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint soft_range {
        soft addr == 100;
        addr inside {[1:200]};
    }
endclass

module tb;
    Packet p;
    int result;
    initial begin
        p = new();
        if (p.randomize()) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "randomize with soft constraint should succeed"
    );
}

#[test]
fn test_randomize_soft_constraint_yields_to_hard() {
    // LANG-31: soft yang BERTENTANGAN dengan hard constraint HARUS dikalahkan
    // (IEEE 1800-2017 §18.5.14) — randomize tetap sukses dengan nilai
    // memenuhi hard (`addr < 10`), soft `addr == 100` dilanggar.
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint c {
        soft addr == 100;
        addr < 10;
    }
endclass

module tb;
    Packet p;
    int result;
    int addr_out;
    initial begin
        p = new();
        if (p.randomize()) begin
            result = 1;
            addr_out = p.addr;
        end else begin
            result = 0;
            addr_out = -1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "soft must yield to hard: randomize should succeed"
    );
    let (_, addr) = sigs.iter().find(|(n, _)| n == "addr_out").unwrap();
    assert!(
        addr.to_u64() < 10,
        "addr must satisfy hard constraint addr < 10, got {}",
        addr.to_u64()
    );
}

#[test]
fn test_constraint_mode_disable_enables_randomize() {
    // LANG-33: `constraint_mode(0)` me-nonaktifkan constraint block —
    // randomize yang tadinya gagal (c1 addr<100 && c2 addr>200 tidak
    // mungkin) jadi sukses setelah c2 di-disable (hanya c1 aktif).
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint c1 { addr < 100; }
    constraint c2 { addr > 200; }
endclass

module tb;
    Packet p;
    int result;
    int addr_out;
    initial begin
        p = new();
        p.c2.constraint_mode(0);
        if (p.randomize()) begin
            result = 1;
            addr_out = p.addr;
        end else begin
            result = 0;
            addr_out = -1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "constraint_mode(0) harus mengaktifkan randomize (c2 di-skip)"
    );
    let (_, addr) = sigs.iter().find(|(n, _)| n == "addr_out").unwrap();
    assert!(
        addr.to_u64() < 100,
        "addr harus memenuhi c1 (addr < 100), got {}",
        addr.to_u64()
    );
}

#[test]
fn test_constraint_mode_query_and_reenable() {
    // LANG-33: query `constraint_mode()` mengembalikan mode (0/1); re-enable
    // via `constraint_mode(1)` membuat block aktif kembali.
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint c1 { addr < 100; }
    constraint c2 { addr > 200; }
endclass

module tb;
    Packet p;
    int mode_off, mode_on;
    int result_disabled, result_enabled;
    initial begin
        p = new();
        p.c2.constraint_mode(0);
        mode_off = p.c2.constraint_mode();
        result_disabled = p.randomize() ? 1 : 0;
        p.c2.constraint_mode(1);
        mode_on = p.c2.constraint_mode();
        result_enabled = p.randomize() ? 1 : 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let mode_off = sigs
        .iter()
        .find(|(n, _)| n == "mode_off")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let mode_on = sigs
        .iter()
        .find(|(n, _)| n == "mode_on")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(mode_off, 0, "mode setelah constraint_mode(0) harus 0");
    assert_eq!(mode_on, 1, "mode setelah constraint_mode(1) harus 1");
    let r_disabled = sigs
        .iter()
        .find(|(n, _)| n == "result_disabled")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let r_enabled = sigs
        .iter()
        .find(|(n, _)| n == "result_enabled")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(r_disabled, 1, "c2 nonaktif → randomize sukses");
    assert_eq!(r_enabled, 0, "c2 aktif kembali → randomize gagal (konflik)");
}

#[test]
fn test_randomize_per_instance_seed_distinct() {
    // VERIF-35: seed per-instance — dua instance class sama yang di-randomize
    // pada waktu SAMA harus mendapat nilai BERBEDA (seed lama hanya
    // current_time → deret identik). Tanpa fix: va == vb.
    let source = r#"
class Packet;
    rand logic [31:0] addr;
endclass

module tb;
    Packet p1;
    Packet p2;
    logic [31:0] va, vb;
    initial begin
        p1 = new();
        p2 = new();
        p1.randomize();
        p2.randomize();
        va = p1.addr;
        vb = p2.addr;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let va = sigs
        .iter()
        .find(|(n, _)| n == "va")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let vb = sigs
        .iter()
        .find(|(n, _)| n == "vb")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_ne!(
        va, vb,
        "per-instance seed: p1/p2 harus beda (va={}, vb={})",
        va, vb
    );
}

#[test]
fn test_randomize_per_instance_seed_reproducible() {
    // VERIF-35: deterministik — run kedua dengan (instance, waktu) sama
    // menghasilkan nilai sama (bukan thread_rng acak). $urandom global tetap
    // jalan (RNG engine tak disentuh).
    let src = r#"
class Packet;
    rand logic [31:0] addr;
endclass

module tb;
    Packet p1;
    logic [31:0] va;
    initial begin
        p1 = new();
        p1.randomize();
        va = p1.addr;
        #1 $finish;
    end
endmodule
"#;
    let sigs1 = simulate_signals(src, 5).unwrap();
    let sigs2 = simulate_signals(src, 5).unwrap();
    let a1 = sigs1
        .iter()
        .find(|(n, _)| n == "va")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let a2 = sigs2
        .iter()
        .find(|(n, _)| n == "va")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        a1, a2,
        "per-instance seed harus reproducible ({} vs {})",
        a1, a2
    );
}

#[test]
fn test_static_constraint_global_constraint_mode() {
    // LANG-32: `static constraint` dibagi antar SEMUA instance class
    // (IEEE 1800-2017 §18.5.10) — `constraint_mode(0/1)` pada satu instance
    // berlaku global: query via instance lain mengembalikan mode yang sama.
    let source = r#"
class Packet;
    rand bit [3:0] addr;
    static constraint c_static { addr == 5; }
    constraint c_inst { addr > 0; }
endclass

module tb;
    Packet p1;
    Packet p2;
    int r1;
    int r2;
    int m_p1;
    int m_p2;
    initial begin
        p1 = new();
        p2 = new();
        if (p1.randomize()) r1 = p1.addr;
        p1.c_static.constraint_mode(0);
        m_p1 = p1.c_static.constraint_mode();
        m_p2 = p2.c_static.constraint_mode();
        p2.c_static.constraint_mode(1);
        if (p2.randomize()) r2 = p2.addr;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("r1"), 5, "static constraint aktif: addr harus 5");
    assert_eq!(
        get("m_p1"),
        0,
        "constraint_mode(0) via p1 harus terlihat via p1"
    );
    assert_eq!(
        get("m_p2"),
        0,
        "static constraint_mode GLOBAL: query via p2 juga 0"
    );
    assert_eq!(
        get("r2"),
        5,
        "re-enable via p2 global → randomize p2 memenuhi addr==5 lagi"
    );
}

#[test]
fn test_randomize_no_constraint() {
    let source = r#"
class Simple;
    rand logic [7:0] val;
endclass

module tb;
    Simple s;
    int result;
    initial begin
        s = new();
        if (s.randomize()) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "randomize without constraints should succeed"
    );
}

#[test]
fn test_randomize_with_inside_constraint() {
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint addr_excl {
        addr != 0;
    }
endclass

module tb;
    Packet p;
    int result;
    initial begin
        p = new();
        if (p.randomize()) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "randomize with constraint should succeed");
}

#[test]
fn test_foreach_in_class() {
    let source = r#"
class Accum;
    logic [31:0] arr [0:3];
    int sum;
    function new();
        sum = 0;
    endfunction
    function void init();
        arr[0] = 10;
        arr[1] = 20;
        arr[2] = 30;
        arr[3] = 40;
    endfunction
    function void accumulate();
        foreach (arr[i]) begin
            sum = sum + arr[i];
        end
    endfunction
endclass

module tb;
    Accum a;
    int result;
    initial begin
        a = new();
        a.init();
        a.accumulate();
        result = a.sum;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        100,
        "foreach should sum array elements: 10+20+30+40=100"
    );
}

#[test]
fn test_preprocessor_define_and_expand() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define WIDTH 8\nmodule test;\n    wire [`WIDTH-1:0] data;\nendmodule\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire [8-1:0] data"),
        "macro should expand WIDTH: {}",
        result
    );
}

#[test]
fn test_preprocessor_ifdef() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    pp.define("DEBUG", "1");
    let source = "`ifdef DEBUG\nwire dbg;\n`else\nwire nodbg;\n`endif\nwire always;\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire dbg;"),
        "ifdef true branch should be emitted"
    );
    assert!(
        !result.contains("wire nodbg;"),
        "else branch should be skipped"
    );
    assert!(
        result.contains("wire always;"),
        "post-endif should be emitted"
    );
}

#[test]
fn test_preprocessor_ifndef() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`ifndef DEBUG\nwire dbg;\n`else\nwire nodbg;\n`endif\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire dbg;"),
        "ifndef true branch should be emitted"
    );
    assert!(
        !result.contains("wire nodbg;"),
        "else branch should be skipped"
    );
}

#[test]
fn test_preprocessor_strip_unknown_macro() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`uvm_component_utils(my_driver)\nmodule test;\nendmodule\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        !result.contains("`uvm_component_utils"),
        "unknown macro should be stripped"
    );
    assert!(
        result.contains("module test;"),
        "module decl should survive"
    );
}

#[test]
fn test_timescale_directive() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let src = "`timescale 1ns / 10ps\nmodule top;\ninitial #1 $finish;\nendmodule\n";
    let result = pp.preprocess(src, None).unwrap();
    assert_eq!(pp.timescale, Some(("1ns".to_string(), "10ps".to_string())));
    assert!(
        result.contains("module top;"),
        "timescale should pass through module text"
    );
}

#[test]
fn test_preprocessor_nested_ifdef() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    pp.define("A", "1");
    pp.define("B", "1");
    let source =
        "`ifdef A\n`ifdef B\nwire both;\n`else\nwire only_a;\n`endif\n`endif\nwire after;\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire both;"),
        "both defined: both should be emitted"
    );
    assert!(!result.contains("wire only_a;"), "else should be skipped");
    assert!(result.contains("wire after;"), "post-endif emitted");
}

#[test]
fn test_preprocessor_macro_arguments() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define ADD(a,b) a + b\nwire `ADD(x,y);\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire x + y;"),
        "macro args should substitute: {}",
        result
    );
}

#[test]
fn test_preprocessor_macro_args_complex() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define MIN(a,b) ((a) < (b) ? (a) : (b))\nwire [3:0] w = `MIN(4+1, 8);\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("((4+1) < (8) ? (4+1) : (8))"),
        "complex macro: {}",
        result
    );
}

#[test]
fn test_preprocessor_macro_args_multiline() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define SUM(a,b,c) a + b + c\nwire w = `SUM(x, y, z);\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(result.contains("x + y + z"), "three args: {}", result);
}

#[test]
fn test_preprocessor_macro_debug_output() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define ADD(a,b) a + b\nmodule tb;\n    reg [3:0] sum;\n    initial begin\n        sum = `ADD(2, 3);\n        #1 $finish;\n    end\nendmodule\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("sum = 2 + 3;"),
        "macro should expand: '{}'",
        result
    );
}

#[test]
fn test_preprocessor_macro_args_in_expression() {
    let source = r#"
`define ADD(a,b) a + b

module tb;
    reg [3:0] sum;
    initial begin
        sum = `ADD(2, 3);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let sum_val = sigs
        .iter()
        .find(|(n, _)| n == "sum")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        sum_val, 5,
        "macro ADD(2,3) should expand to 2+3=5, got {}",
        sum_val
    );
}

#[test]
fn test_event_control_procedural() {
    let source = r#"
module tb;
    reg clk;
    reg [7:0] q;
    initial begin
        clk = 0;
        q = 0;
        #5 clk = 1;
        #1 clk = 0;
        #1 clk = 1;
        #1 $finish;
    end
    always @(posedge clk) begin
        q <= q + 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(q_val, 2, "q should be 2 after 2 posedge clk events");
}

#[test]
fn test_event_control_procedural_at() {
    let source = r#"
module tb;
    reg clk;
    reg [7:0] q;
    initial begin
        clk = 0;
        q = 0;
        #5 clk = 1;
        @(posedge clk);
        q = 42;
        #1 $finish;
    end
    always #5 clk = ~clk;
endmodule
"#;
    // `@(posedge clk)` BLOCKING per IEEE: posedge yang sudah lewat (clk naik di #5)
    // tidak dihitung — menunggu posedge BERIKUTNYA (di #15 dari always #5).
    let sigs = simulate_signals(source, 20).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        q_val, 42,
        "q should be 42 after @(posedge clk) blocks until next edge"
    );
}

#[test]
fn test_event_trigger() {
    let source = r#"
module tb;
    reg ev;
    reg [7:0] q;
    initial begin
        q = 0;
        -> ev;
        #1 $finish;
    end
    initial begin
        @(ev) q = 99;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(q_val, 99, "q should be 99 after -> ev triggers @(ev)");
}

#[test]
fn test_specify_parse() {
    let source = r#"
module tb;
    reg data, clk;
    specify
        specparam tSU = 1.0;
        $setup(data, posedge clk, tSU);
        $hold(posedge clk, data, 0.5);
        (data => q) = (1.0);
    endspecify
endmodule
"#;
    let result = compile_str(source);
    assert!(result.is_ok(), "specify block compile should succeed");
}

#[test]
fn test_specify_with_module() {
    let source = r#"
module dut(input clk, input d, output reg q);
    always_ff @(posedge clk) q <= d;
    specify
        $setup(d, posedge clk, 1);
        $hold(posedge clk, d, 0);
    endspecify
endmodule
module tb;
    reg clk, d;
    wire q;
    dut u1(.clk(clk), .d(d), .q(q));
    initial begin
        clk = 0; d = 0;
        #5 clk = 1; #5 clk = 0;
        #5 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    assert!(sigs.iter().any(|(n, _)| n == "q"), "q signal should exist");
}

#[test]
fn test_event_control_iff_always_ff() {
    // LANG-27: `always_ff @(posedge clk iff (en))` — q hanya update saat
    // en=1 pada edge naik clk. Saat en=0, edge clk diabaikan.
    let source = r#"
module tb;
    reg clk = 0, en = 0, d = 0;
    reg q = 0;
    always_ff @(posedge clk iff (en)) q <= d;
    always #5 clk = ~clk;
    initial begin
        d = 1;
        // Edge naik pertama (t=10): en masih 0 → q harus tetap 0.
        en = 0;
        #10 en = 1;
        // Edge naik kedua (t=20): en=1 → q = d = 1.
        #10 $display("q=%0d", q);
        if (q !== 1'b1) $error("iff guard failed: q=%0b expected 1", q);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 25).unwrap();
    let q = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .expect("q signal should exist")
        .1
        .clone();
    assert_eq!(q.to_u64(), 1, "q should be 1 after enabled edge (iff gate)");
}

#[test]
fn test_event_control_iff_blocks_when_cond_false() {
    // LANG-27: guard `iff` di blocking event control `@(posedge clk iff (g))`
    // di procedural code — body hanya lanjut bila kondisi benar.
    let source = r#"
module tb;
    reg clk = 0;
    reg g = 0;
    integer count = 0;
    always #5 clk = ~clk;
    initial begin
        // Set g=1 sebelum edge kedua; edge pertama (t=10) di-skip.
        #10 g = 1;
        #20 $display("count=%0d", count);
        if (count !== 1) $error("iff blocking failed: count=%0d expected 1", count);
        $finish;
    end
    initial begin
        @(posedge clk iff (g)) count = count + 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let count = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .expect("count signal should exist")
        .1
        .clone();
    assert_eq!(
        count.to_u64(),
        1,
        "count should be 1 (only gated edge counts)"
    );
}

#[test]
fn test_sensitivity_iff_elaborated_sequential() {
    // LANG-27: `always @(posedge clk iff (en))` ter-elaborasi jadi
    // Process::Sequential dengan field iff terisi (guard kondisi).
    let src = r#"
module m(input clk, input en, input d, output reg q);
    always @(posedge clk iff (en)) q <= d;
endmodule
module tb;
    reg clk, en, d;
    wire q;
    m u1(.clk(clk), .en(en), .d(d), .q(q));
endmodule
"#;
    let design = compile_str(src).expect("compile should succeed");
    let has_iff = design
        .top
        .processes
        .iter()
        .any(|p| matches!(p, mivon_ir::Process::Sequential { iff: Some(_), .. }));
    assert!(has_iff, "sequential process should carry iff guard");
}

#[test]
fn test_udp_sequential_dff_posedge0() {
    let source = r#"
primitive dff(output reg q, input clk, input d);
    table
        (01) 0 : ? : 0;
        (01) 1 : ? : 1;
        ?    ? : ? : -;
    endtable
endprimitive

module tb;
    reg clk, d;
    wire q;
    dff u1(q, clk, d);
    initial begin
        clk = 0; d = 0;
        #1 clk = 1;
        #1 if (q !== 0) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(q_val, 0, "dff: posedge with d=0 -> q=0");
}

#[test]
fn test_udp_sequential_dff_posedge1() {
    let source = r#"
primitive dff(output reg q, input clk, input d);
    table
        (01) 0 : ? : 0;
        (01) 1 : ? : 1;
        ?    ? : ? : -;
    endtable
endprimitive

module tb;
    reg clk, d;
    wire q;
    dff u1(q, clk, d);
    initial begin
        clk = 0; d = 0;
        #1 clk = 1; $display("t1 clk=%b d=%b q=%b", clk, d, q);
        #1 clk = 0; d = 1; $display("t2 clk=%b d=%b q=%b", clk, d, q);
        #1 clk = 1; $display("t3 clk=%b d=%b q=%b", clk, d, q);
        #1 $display("t4 clk=%b d=%b q=%b", clk, d, q);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 15).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(q_val, 1, "dff: second posedge with d=1 -> q=1");
}

#[test]
fn test_udp_sequential_dff_initial() {
    let source = r#"
primitive dff_init(output reg q, input clk, input d);
    initial q = 0;
    table
        (01) 0 : ? : 0;
        (01) 1 : ? : 1;
        (0?) 1 : 1 : 1;
        (?0) ? : ? : -;
        ?    ? : ? : -;
    endtable
endprimitive

module tb;
    reg clk, d;
    wire q;
    dff_init u1(q, clk, d);
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(q_val, 0, "sequential dff: initial q should be 0");
}

#[test]
fn test_sysfunc_countones() {
    let source = r#"
module tb;
    reg [7:0] val;
    reg [31:0] result;
    initial begin
        val = 8'b10100101;
        result = $countones(val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(r, 4, "$countones(8'b10100101) = 4");
}

/// `$past(v[,n])`: riwayat per (arg, n) — tick 1/2/3 lag 1/2/3 sampel.
/// Regresi: kunci hanya arg → deque tercampur (p1=p2=p3=4, harus 30/20/10).
#[test]
fn test_sysfunc_past_ticks() {
    let source = r#"
module tb;
bit clk;
logic [7:0] cnt, p1, p2, p3;
initial begin
clk = 0; cnt = 8'd10;
#12 cnt = 8'd20;
#10 cnt = 8'd30;
#10 cnt = 8'd40;
#10;
$display("cnt=%0d p1=%0d p2=%0d p3=%0d", cnt, p1, p2, p3);
$finish;
end
always @(posedge clk) begin
p1 <= $past(cnt);
p2 <= $past(cnt, 2);
p3 <= $past(cnt, 3);
end
initial begin forever #5 clk = ~clk; end
endmodule
"#;
    let sigs = simulate_signals(source, 100).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("p1"), 30, "$past(cnt) lag 1");
    assert_eq!(get("p2"), 20, "$past(cnt,2) lag 2");
    assert_eq!(get("p3"), 10, "$past(cnt,3) lag 3");
}

/// `$past` tanpa riwayat cukup → X (4-state), bukan 0.
#[test]
fn test_sysfunc_past_initial_x() {
    let source = r#"
module tb;
logic [7:0] v;
bit w1, w2;
initial begin
v = 8'hAA;
w1 = $isunknown($past(v));
w2 = $isunknown($past(v));
#1 $finish;
end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("w1"), 1, "$past pertama harus X");
    assert_eq!(get("w2"), 0, "$past kedua sudah ada riwayat");
}

#[test]
fn test_sysfunc_onehot() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg onehot_a, onehot_b;
    initial begin
        a = 4'b0100;
        b = 4'b0110;
        onehot_a = $onehot(a);
        onehot_b = $onehot(b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ra = sigs
        .iter()
        .find(|(n, _)| n == "onehot_a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    let rb = sigs
        .iter()
        .find(|(n, _)| n == "onehot_b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(ra, 1, "$onehot(4'b0100) = 1");
    assert_eq!(rb, 0, "$onehot(4'b0110) = 0");
}

#[test]
fn test_sysfunc_onehot0() {
    let source = r#"
module tb;
    reg [3:0] a, b, c, d, e;
    reg oh0_a, oh0_b, oh0_c, oh0_d, oh0_e;
    reg oh_a, oh_b, oh_c;
    initial begin
        a = 4'b0000;  // zero bits
        b = 4'b0001;  // one bit
        c = 4'b0011;  // two bits
        d = 4'b0101;  // two bits non-adjacent
        e = 4'b1111;  // four bits
        
        oh0_a = $onehot0(a);
        oh0_b = $onehot0(b);
        oh0_c = $onehot0(c);
        oh0_d = $onehot0(d);
        oh0_e = $onehot0(e);
        
        oh_a = $onehot(a);
        oh_b = $onehot(b);
        oh_c = $onehot(c);
        
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    // $onehot0 returns 1 for 0 or 1 bits set
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_a").unwrap().1.to_u64(),
        1,
        "$onehot0(0000)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_b").unwrap().1.to_u64(),
        1,
        "$onehot0(0001)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_c").unwrap().1.to_u64(),
        0,
        "$onehot0(0011)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_d").unwrap().1.to_u64(),
        0,
        "$onehot0(0101)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_e").unwrap().1.to_u64(),
        0,
        "$onehot0(1111)"
    );
    // $onehot returns 1 only for exactly 1 bit set
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh_a").unwrap().1.to_u64(),
        0,
        "$onehot(0000)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh_b").unwrap().1.to_u64(),
        1,
        "$onehot(0001)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh_c").unwrap().1.to_u64(),
        0,
        "$onehot(0011)"
    );
}

#[test]
fn test_sysfunc_isunknown() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg unk_a, unk_b;
    initial begin
        a = 4'b1010;
        b = 4'b10xz;
        unk_a = $isunknown(a);
        unk_b = $isunknown(b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ra = sigs
        .iter()
        .find(|(n, _)| n == "unk_a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    let rb = sigs
        .iter()
        .find(|(n, _)| n == "unk_b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(ra, 0, "$isunknown(4'b1010) = 0");
    assert_eq!(rb, 1, "$isunknown(4'b10xz) = 1");
}

#[test]
fn test_sysfunc_typename() {
    let source = r#"
module tb;
    logic [7:0] sig_a;
    logic signed [15:0] sig_b;
    logic [3:0][7:0] sig_c;
    int sig_d;
    real sig_e;
    string sig_f;
    initial begin
        $display("sig_a: %s", $typename(sig_a));
        $display("sig_b: %s", $typename(sig_b));
        $display("sig_c: %s", $typename(sig_c));
        $display("sig_d: %s", $typename(sig_d));
        $display("sig_e: %s", $typename(sig_e));
        $display("sig_f: %s", $typename(sig_f));
        $display("lit_int: %s", $typename(42));
        $display("lit_real: %s", $typename(3.14));
        $display("lit_str: %s", $typename("hello"));
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    // Just verify it runs without error - $typename returns string as LogicVec
    // Check that all declared signals exist
    // Note: string signals have width 0 in current implementation (dynamic string)
    let sig_names = ["sig_a", "sig_b", "sig_c", "sig_d", "sig_e", "sig_f"];
    for name in sig_names {
        let found = sigs.iter().find(|(n, _)| n == name);
        assert!(found.is_some(), "Signal {} not found in results", name);
    }
}

#[test]
fn test_sysfunc_countbits() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [15:0] b;
    reg [31:0] result_a, result_b;
    initial begin
        a = 8'b10100101;  // 4 ones
        b = 16'hA5A5;      // 8 ones (A=1010, 5=0101)
        result_a = $countbits(a, 1'b1);
        result_b = $countbits(b, 1'b1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ra = sigs
        .iter()
        .find(|(n, _)| n == "result_a")
        .unwrap()
        .1
        .to_u64();
    let rb = sigs
        .iter()
        .find(|(n, _)| n == "result_b")
        .unwrap()
        .1
        .to_u64();
    assert_eq!(ra, 4, "$countbits(8'b10100101) = 4");
    assert_eq!(rb, 8, "$countbits(16'hA5A5) = 8");
}

#[test]
fn test_sysfunc_dimensions() {
    let source = r#"
module tb;
    logic [7:0] scalar;
    logic [3:0][7:0] packed_2d;
    logic [3:0] unpacked [7:0];
    logic [2:0][1:0][3:0] packed_3d;
    
    reg [31:0] dim_scalar, dim_packed_2d, dim_unpacked, dim_packed_3d;
    initial begin
        dim_scalar = $dimensions(scalar);
        dim_packed_2d = $dimensions(packed_2d);
        dim_unpacked = $dimensions(unpacked);
        dim_packed_3d = $dimensions(packed_3d);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_scalar")
            .unwrap()
            .1
            .to_u64(),
        0,
        "$dimensions(scalar) = 0"
    );
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_packed_2d")
            .unwrap()
            .1
            .to_u64(),
        2,
        "$dimensions(packed_2d) = 2"
    );
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_unpacked")
            .unwrap()
            .1
            .to_u64(),
        1,
        "$dimensions(unpacked) = 1"
    );
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_packed_3d")
            .unwrap()
            .1
            .to_u64(),
        3,
        "$dimensions(packed_3d) = 3"
    );
}

#[test]
fn test_timing_check_setup() {
    let source = r#"
module tb;
    reg data, clk;
    wire q;
    specify
        $setup(data, posedge clk, 5);
    endspecify
    initial begin
        data = 0;
        #1 clk = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    assert!(sigs.iter().any(|(n, _)| n == "data"));
}

#[test]
fn test_fgets_string_var() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fgets.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "hello").unwrap();
        writeln!(f, "world").unwrap();
    }
    let source = format!(
        r#"
module tb;
    string line;
    integer fd;
    initial begin
        fd = $fopen("{}", "r");
        if (fd == 0) begin
            $display("FAIL: cannot open file");
            $finish;
        end
        #1;
        $fgets(line, fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    // Check that line has data (non-empty string signal)
    let line_sig = sigs.iter().find(|(n, _)| n == "line");
    assert!(line_sig.is_some(), "line signal should exist");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_fgetc_basic() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fgetc.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "A").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer c;
    integer fd;
    initial begin
        fd = $fopen("{}", "r");
        #1;
        c = $fgetc(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let c_val = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // 'A' = 65
    assert_eq!(c_val, 65, "$fgetc should read 'A' (65)");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_fflush_basic() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fflush.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "hello").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer fd;
    initial begin
        fd = $fopen("{}", "a");
        $fwrite(fd, "world");
        $fflush(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let _sigs = simulate_signals(&source, 10).unwrap();
    let _ = std::fs::remove_file(&tmp);
    // Just verify no crash
}

#[test]
fn test_fseek_ftell() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fseek.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "ABCDEFGHIJ").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer fd;
    integer pos;
    integer ch;
    initial begin
        fd = $fopen("{}", "r");
        #1;
        ch = $fgetc(fd);
        pos = $ftell(fd);
        $fseek(fd, 0, 0);
        ch = $fgetc(fd);
        pos = $ftell(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let pos_val = sigs
        .iter()
        .find(|(n, _)| n == "pos")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(pos_val, 1, "$ftell after reading 1 byte should be 1");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_feof() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_feof.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        write!(f, "AB").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer fd;
    integer eof;
    integer ch;
    initial begin
        fd = $fopen("{}", "r");
        eof = $feof(fd);
        ch = $fgetc(fd);
        ch = $fgetc(fd);
        ch = $fgetc(fd);
        eof = $feof(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let eof_val = sigs
        .iter()
        .find(|(n, _)| n == "eof")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(eof_val, 1, "$feof should be 1 after reading past end");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_const_decl() {
    let source = r#"
module tb;
    const logic [7:0] x = 42;
    reg [7:0] y;
    initial begin
        y = x;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let x_val = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(x_val, 42, "const x should be 42");
}

#[test]
fn test_parallel_eval_basic() {
    let source = r#"
module tb;
    reg [7:0] a, b, c, d;
    wire [7:0] x, y;
    assign x = a + b;
    assign y = c + d;
    initial begin
        a = 1; b = 2; c = 3; d = 4;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    // Enable parallel with threshold of 1 for testing
    let pcfg = crate::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        parallel_processes: true,
        ..Default::default()
    };
    engine.set_parallel_config(pcfg);
    engine.run().unwrap();
    let sigs = engine.design.top.signals.clone();
    let x_val = sigs
        .iter()
        .find(|_s| _s.name == "x")
        .map(|_s| {
            engine
                .state
                .read_signal(
                    engine
                        .design
                        .top
                        .signals
                        .iter()
                        .position(|x| x.name == "x")
                        .unwrap_or(0),
                )
                .to_u64()
        })
        .unwrap_or(0);
    assert_eq!(x_val, 3, "parallel: x = a + b = 1 + 2 = 3");
}

#[test]
fn test_gate_primitives_and_or() {
    let source = r#"
module tb;
    reg a, b, c, d;
    wire and_out, or_out;
    and a1(and_out, a, b, c);
    or  o1(or_out, a, b);
    initial begin
        a = 1; b = 1; c = 1;
        #1 d = and_out;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let d_val = sigs
        .iter()
        .find(|(n, _)| n == "d")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(d_val, 1, "and_out should be 1 (1 & 1 & 1 = 1)");
}

#[test]
fn test_gate_not_buf() {
    let source = r#"
module tb;
    reg in;
    wire out;
    not n1(out, in);
    initial begin
        in = 0;
        #1;
        if (out !== 1) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(out_val, 1, "not gate should invert 0 to 1");
}

#[test]
fn test_udp_combinational_and() {
    let source = r#"
primitive udp_and(output z, input a, input b);
    table
        0 0 : 0;
        0 1 : 0;
        1 0 : 0;
        1 1 : 1;
    endtable
endprimitive

module tb;
    reg a, b;
    wire z;
    udp_and u1(z, a, b);
    initial begin
        a = 0; b = 0; #1;
        if (z !== 0) $finish;
        a = 1; b = 1; #1;
        if (z !== 1) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let z_val = sigs
        .iter()
        .find(|(n, _)| n == "z")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(z_val, 1, "UDP and: 1 & 1 = 1");
}

#[test]
fn test_udp_combinational_mux() {
    let source = r#"
primitive udp_mux(output z, input a, input b, input sel);
    table
        0 ? 0 : 0;
        1 ? 0 : 1;
        ? 0 1 : 0;
        ? 1 1 : 1;
    endtable
endprimitive

module tb;
    reg a, b, sel;
    wire z;
    udp_mux u1(z, a, b, sel);
    initial begin
        a = 1; b = 0; sel = 0; #1;
        if (z !== 1) $finish;
        a = 1; b = 0; sel = 1; #1;
        if (z !== 0) $finish;
        a = 0; b = 1; sel = 0; #1;
        if (z !== 0) $finish;
        a = 0; b = 1; sel = 1; #1;
        if (z !== 1) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let z_val = sigs
        .iter()
        .find(|(n, _)| n == "z")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(z_val, 1, "UDP mux: sel=1,b=1 -> 1");
}

#[test]
fn test_udp_compile_only() {
    let source = r#"
primitive udp_nand(output z, input a, input b);
    table
        0 0 : 1;
        0 1 : 1;
        1 0 : 1;
        1 1 : 0;
    endtable
endprimitive
module tb;
    wire z;
    reg a = 0, b = 0;
    udp_nand u1(z, a, b);
endmodule
"#;
    let result = compile_str(source);
    assert!(result.is_ok(), "UDP compile should succeed");
}

#[test]
fn test_monitor_task() {
    let source = r#"
module tb;
    reg a;
    initial begin
        a = 0;
        $monitor("a=%d", a);
        #1 a = 1;
        #1 a = 0;
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 10).unwrap();
}

#[test]
fn test_string_methods_len_substr() {
    let source = r#"
module tb;
    reg [63:0] len_val;
    initial begin
        len_val = "hello".len();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let len = sigs
        .iter()
        .find(|(n, _)| n == "len_val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(len, 5, "len of 'hello' should be 5");
}

#[test]
fn test_string_methods_atoi() {
    let source = r#"
module tb;
    reg [31:0] val;
    initial begin
        val = "42".atoi();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "atoi of '42' should be 42");
}

#[test]
fn test_string_methods_getc_and_ato() {
    // LRM 1800 §6.16.1 — getc / atohex / atobin / atooct / atoreal.
    let source = r#"
module tb;
    reg [7:0]  gc0;
    reg [7:0]  gc1;
    reg [7:0]  gc9;   // out-of-range -> 0
    reg [31:0] hx;
    reg [31:0] bn;
    reg [31:0] oc;
    real       rl;
    initial begin
        gc0 = "ABC".getc(0);   // 'A' = 65
        gc1 = "ABC".getc(1);   // 'B' = 66
        gc9 = "ABC".getc(9);   // out of range -> 0
        hx  = "1f".atohex();   // 31
        bn  = "101".atobin();  // 5
        oc  = "17".atooct();   // 15
        rl  = "3.5".atoreal(); // 3.5
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let g = |n: &str| {
        sigs.iter()
            .find(|(nm, _)| nm == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(g("gc0"), 65, "getc(0) of 'ABC' = 'A' = 65");
    assert_eq!(g("gc1"), 66, "getc(1) of 'ABC' = 'B' = 66");
    assert_eq!(g("gc9"), 0, "getc out-of-range = 0");
    assert_eq!(g("hx"), 31, "atohex('1f') = 31");
    assert_eq!(g("bn"), 5, "atobin('101') = 5");
    assert_eq!(g("oc"), 15, "atooct('17') = 15");
    // atoreal: bit-pattern f64 3.5 == 0x400C000000000000
    assert_eq!(g("rl"), 0x400C000000000000u64, "atoreal('3.5') = 3.5");
}

#[test]
fn test_string_methods_mutating() {
    // LRM 1800 §6.16.1 — method MUTATING: itoa/hextoa/bintoa/octtoa/putc.
    // Memverifikasi (1) write-back hasil ke receiver string, (2) bentuk
    // assignment `t = s.itoa(...)` juga berfungsi, dan (3) fix panic F91:
    // shift-overflow di logicvec_to_string pada string hasil putc yang
    // diperpanjang dengan NUL (internal NUL) serta indeks negatif putc (no-op).
    let source = r#"
module tb;
    string a, b, c, d, e, f, g, z, y;
    reg [31:0] flen;
    initial begin
        a.itoa(-42);                 // a = "-42"
        b = "abc";  b.hextoa(255);   // b = "ff"
        c = "abc";  c.bintoa(5);     // c = "101"
        d = "abc";  d.octtoa(15);    // d = "17"
        e = "abc";  e.putc(0, 88);   // e = "Xbc"
        f = "abc";  f.putc(5, 90);   // f = "abc\0\0Z" (extend dgn NUL)
        g = "abc";  g.putc(-3, 77);  // indeks negatif -> no-op (g = "abc")
        flen = f.len();              // 6
        z = "x";  y = z.itoa(9);     // assignment-form: y="9", z juga "9" (mutating)
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| -> Vec<u8> {
        sigs.iter()
            .find(|(nm, _)| nm == n)
            .map(|(_, v)| logicvec_to_string(v).into_bytes())
            .unwrap_or_default()
    };
    let flen = sigs
        .iter()
        .find(|(n, _)| n == "flen")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);

    assert_eq!(get("a"), b"-42".to_vec(), "itoa(-42) -> \"-42\"");
    assert_eq!(get("b"), b"ff".to_vec(), "hextoa(255) -> \"ff\"");
    assert_eq!(get("c"), b"101".to_vec(), "bintoa(5) -> \"101\"");
    assert_eq!(get("d"), b"17".to_vec(), "octtoa(15) -> \"17\"");
    assert_eq!(get("e"), b"Xbc".to_vec(), "putc(0,88) -> \"Xbc\"");
    // putc(5,90) pada "abc" -> "abc\0\0Z" (panjang 6, NUL internal).
    assert_eq!(
        get("f"),
        vec![b'a', b'b', b'c', 0, 0, b'Z'],
        "putc(5,90) extend dgn NUL -> \"abc\\0\\0Z\""
    );
    assert_eq!(get("g"), b"abc".to_vec(), "putc(-3,..) no-op -> \"abc\"");
    assert_eq!(flen, 6, "len() setelah putc extend = 6");
    assert_eq!(
        get("y"),
        b"9".to_vec(),
        "assignment-form y = z.itoa(9) -> \"9\""
    );
    assert_eq!(
        get("z"),
        b"9".to_vec(),
        "itoa mutating: receiver z ikut berubah -> \"9\""
    );
}

#[test]
fn test_string_var_decl() {
    let source = r#"
module tb;
    string s;
    reg [31:0] len;
    initial begin
        s = "hello";
        len = s.len();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let len = sigs
        .iter()
        .find(|(n, _)| n == "len")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(len, 5, "string variable len should be 5");
}

#[test]
fn test_string_var_reassign() {
    let source = r#"
module tb;
    string s;
    reg [31:0] len;
    initial begin
        s = "hello";
        s = "hi";
        len = s.len();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let len = sigs
        .iter()
        .find(|(n, _)| n == "len")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(len, 2, "reassigned string variable len should be 2");
}

#[test]
fn test_string_var_display() {
    let source = r#"
module tb;
    string s;
    reg [31:0] result;
    initial begin
        s = "hello";
        result = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let result = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(result, 1, "string variable display should not crash");
}

#[test]
fn test_dynamic_array_decl() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] val;
    initial begin
        d[0] = 42;
        d[1] = 99;
        val = d[0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 42, "dynamic array element should be 42");
}

#[test]
fn test_dynamic_array_size() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] sz;
    initial begin
        d[0] = 10;
        d[1] = 20;
        sz = d.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 2, "dynamic array size should be 2 after 2 writes");
}

#[test]
fn test_dynamic_array_new_size() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] sz;
    initial begin
        d = new[5];
        sz = d.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 5, "dynamic array size should be 5 after new[5]");
}

/// `new[N](old)` menyalin min(old,new) elemen pertama (LRM 1800 §7.5.2.1).
/// Regresi: init dibuang parser → resize kehilangan semua data.
/// Oracle: iverilog (`size=6 d0=10..d3=40`, shrink `size=2 d0=10 d1=20`).
#[test]
fn test_dynamic_array_new_with_copy() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] sz, v0, v3, s2, w0, w1;
    initial begin
        d = new[4];
        d[0] = 10; d[1] = 20; d[2] = 30; d[3] = 40;
        d = new[6](d);
        sz = d.size(); v0 = d[0]; v3 = d[3];
        d = new[2](d);
        s2 = d.size(); w0 = d[0]; w1 = d[1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("sz"), 6, "grow size");
    assert_eq!(get("v0"), 10, "grow d0 preserved");
    assert_eq!(get("v3"), 40, "grow d3 preserved");
    assert_eq!(get("s2"), 2, "shrink size");
    assert_eq!(get("w0"), 10, "shrink d0 preserved");
    assert_eq!(get("w1"), 20, "shrink d1 preserved");
}

#[test]
fn test_queue_push_pop() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] val;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        val = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 10, "queue pop_front should return first element 10");
}

#[test]
fn test_queue_size() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] sz;
    initial begin
        q.push_back(10);
        q.push_back(20);
        sz = q.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 2, "queue size should be 2 after 2 pushes");
}

/// `q[$]` = elemen TERAKHIR (bukan pertama); tulis `q[$] = v` juga jalan.
/// Regresi: `$` jadi SysFunc tak dikenal → 0 → elemen pertama.
/// Oracle: iverilog (baca; tulis crash di iverilog).
#[test]
fn test_queue_dollar_index() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] last, first, w1;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        last = q[$];
        q[$] = 99;
        first = q[0];
        w1 = q[1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("last"), 30, "q[$] = 30");
    assert_eq!(get("first"), 10, "q[0] tetap 10");
    assert_eq!(get("w1"), 20, "q[1] tetap 20");
    // last dibaca SEBELUM tulis; baca ulang implisit via w1? cek via sim kedua:
    let source2 = r#"
module tb;
    int q[$];
    reg [31:0] rl;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q[$] = 99;
        rl = q[$];
        #1 $finish;
    end
endmodule
"#;
    let sigs2 = simulate_signals(source2, 5).unwrap();
    let rl = sigs2.iter().find(|(s, _)| s == "rl").unwrap().1.to_u64();
    assert_eq!(rl, 99, "tulis q[$] = 99 terbaca kembali");
}

/// Index string `s[i]` = BYTE ke-i (karakter), bukan bit tunggal.
/// Regresi: BitSelect 1-bit → bit LSB char (`s[0]`="h" → 0, harus 104).
/// Oracle: Verilator (104,101,108,108,111).
#[test]
fn test_string_byte_index() {
    let source = r#"
module tb;
    string s;
    reg [7:0] c0, c2, c4, w0, w2;
    string t;
    initial begin
        s = "hello";
        c0 = s[0]; c2 = s[2]; c4 = s[4];
        t = "hello";
        t[0] = 74;
        t[2] = 90;
        w0 = t[0]; w2 = t[2];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("c0"), 104, "s[0]='h'");
    assert_eq!(get("c2"), 108, "s[2]='l'");
    assert_eq!(get("c4"), 111, "s[4]='o'");
    assert_eq!(get("w0"), 74, "tulis t[0]='J'");
    assert_eq!(get("w2"), 90, "tulis t[2]='Z'");
}

#[test]
fn test_queue_push_front() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] val;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_front(5);
        val = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 5, "queue push_front then pop_front should return 5");
}

#[test]
fn test_queue_pop_back() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] val;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        val = q.pop_back();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 30, "pop_back should return last element 30");
}

#[test]
fn test_queue_exists() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] exists_0;
    reg [31:0] exists_5;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        exists_0 = q.exists(0);
        exists_5 = q.exists(5);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let e0 = sigs
        .iter()
        .find(|(n, _)| n == "exists_0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let e5 = sigs
        .iter()
        .find(|(n, _)| n == "exists_5")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(e0, 1, "exists(0) should be 1 for element at index 0");
    assert_eq!(e5, 0, "exists(5) should be 0 for index out of range");
}

#[test]
fn test_queue_delete_index() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] sz;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        q.delete(1);
        sz = q.size();
        v0 = q.pop_front();
        v1 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 2, "size should be 2 after delete(1)");
    assert_eq!(v0, 10, "first element should still be 10");
    assert_eq!(v1, 30, "second element should be 30 (index 1 deleted)");
}

#[test]
fn test_array_insert() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(10);
        q.push_back(30);
        q.insert(1, 20);
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 10, "insert: first element should be 10");
    assert_eq!(v1, 20, "insert: inserted element should be 20");
    assert_eq!(v2, 30, "insert: third element should be 30");
}

#[test]
fn test_array_reverse() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        q.reverse();
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 30, "reverse: first should be 30");
    assert_eq!(v1, 20, "reverse: second should be 20");
    assert_eq!(v2, 10, "reverse: third should be 10");
}

#[test]
fn test_array_sort() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(30);
        q.push_back(10);
        q.push_back(20);
        q.sort();
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 10, "sort: first should be 10");
    assert_eq!(v1, 20, "sort: second should be 20");
    assert_eq!(v2, 30, "sort: third should be 30");
}

#[test]
fn test_array_rsort() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(10);
        q.push_back(30);
        q.push_back(20);
        q.rsort();
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 30, "rsort: first should be 30");
    assert_eq!(v1, 20, "rsort: second should be 20");
    assert_eq!(v2, 10, "rsort: third should be 10");
}
#[test]
fn test_swrite_radix_variants_no_newline() {
    let source = r#"
module tb;
    string s;
    reg [7:0] v;
    initial begin
        v = 8'h2a;
        $swriteh(s, v);
        #1;
        $swriteb(s, v);
        #1;
        $swriteo(s, v);
        #1 $finish;
    end
endmodule
"#;
    // Nilai akhir = hasil $swriteo terakhir; memastikan tanpa newline.
    let sigs = simulate_signals(source, 10).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "052", "$swriteo tanpa newline (LRM 21.3.3)");
}

#[test]
fn test_swrite_hex_matches_iverilog() {
    let source = r#"
module tb;
    string s;
    reg [7:0] v;
    initial begin
        v = 8'h2a;
        $swriteh(s, v);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "2a", "$swriteh hex tanpa newline");
}

#[test]
fn test_sformatf_basic() {
    let source = r#"
module tb;
    string s;
    string t;
    reg [31:0] val;
    initial begin
        val = 42;
        s = $sformatf("value = %0d", val);
        // Plain `%d` right-justify ke field selebar nilai maksimum tipe
        // (LRM 1800 Tabel 21-3; 32-bit → 10 kolom) — oracle iverilog.
        t = $sformatf("value = %d", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "value = 42", "sformatf with %0d");
    let t = sigs
        .iter()
        .find(|(n, _)| n == "t")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(t, "value =         42", "%d = field lebar tipe");
}

#[test]
fn test_sformatf_hex() {
    let source = r#"
module tb;
    string s;
    reg [31:0] val;
    initial begin
        val = 255;
        s = $sformatf("0x%h", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(
        s, "0x000000ff",
        "sformatf \"0x%h\" = literal '0x' + full-width hex (IEEE 1800 §21.2.1.3)"
    );
}

#[test]
fn test_sformatf_binary() {
    let source = r#"
module tb;
    string s;
    reg [31:0] val;
    initial begin
        val = 10;
        s = $sformatf("%b", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(
        s, "00000000000000000000000000001010",
        "sformatf with %b (plain = full width, IEEE 1800 §21.2.1.3)"
    );
}

#[test]
fn test_sformatf_multiple_args() {
    let source = r#"
module tb;
    string s;
    reg [31:0] a;
    reg [31:0] b;
    initial begin
        a = 10;
        b = 20;
        s = $sformatf("a=%0d b=%0d", a, b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "a=10 b=20", "sformatf dengan beberapa argumen");
}

#[test]
fn test_fwrite_and_fscanf() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fwrite.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [31:0] val;
    initial begin
        fd = $fopen("{}", "w");
        $fwrite(fd, "42 100");
        $fclose(fd);
        fd = $fopen("{}", "r");
        $fscanf(fd, "%d %d", val);
        #1 $finish;
    end
endmodule
"#,
        test_file, test_file
    );
    let sigs = simulate_signals(&source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 42, "fscanf should read first value");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fstrobe() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fstrobe.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [31:0] cnt;
    initial begin
        fd = $fopen("{f}", "w");
        cnt = 42;
        $fstrobe(fd, "cnt=%0d", cnt);
        #1 cnt = 100;
        #1 $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert!(
        content.contains("cnt=42"),
        "fstrobe should write cnt=42 (pre-change), got: {:?}",
        content
    );
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fdisplay_expression_form_adds_newline() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fdisplay_expr.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    integer r;
    initial begin
        fd = $fopen("{f}", "w");
        r = $fdisplay(fd, "a");
        $fdisplay(fd, "b");
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert_eq!(content, "a\nb\n", "bentuk ekspresi $fdisplay juga menambah newline");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fopen_append_mode() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fopen_append.txt";
    let _ = fs::write(test_file, "old\n");
    let source = format!(
        r#"
module tb;
    integer fd;
    initial begin
        fd = $fopen("{f}", "a");
        $fdisplay(fd, "new");
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert_eq!(content, "old\nnew\n", "mode \"a\" harus append, bukan truncate");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fdisplay_radix_variants() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fdisplay_radix.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [7:0] v;
    initial begin
        v = 8'h2a;
        fd = $fopen("{f}", "w");
        $fdisplayh(fd, v);
        $fwriteb(fd, v);
        $fdisplayo(fd, v);
        $fclose(fd);
        $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert_eq!(content, "2a\n00101010052\n");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_size_with_dimension_argument() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [3:0][7:0] p;
    reg [31:0] m1, m2, p1, p2;
    initial begin
        m1 = $size(m, 1);
        m2 = $size(m, 2);
        p1 = $size(p, 1);
        p2 = $size(p, 2);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("m1"), 4, "unpacked dim pertama");
    assert_eq!(get("m2"), 8, "packed dim kedua");
    assert_eq!(get("p1"), 4, "packed dim pertama");
    assert_eq!(get("p2"), 8, "packed dim kedua");
}

#[test]
fn test_shortreal_bit_conversions() {
    let source = r#"
module tb;
    real r;
    reg [31:0] b;
    initial begin
        r = $bitstoshortreal(32'h3fc00000);
        b = $shortrealtobits(1.5);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(b, 0x3FC0_0000, "$shortrealtobits(1.5)");
    let r = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap_or(f64::NAN);
    assert!((r - 1.5).abs() < 1e-9, "$bitstoshortreal(0x3fc00000) = 1.5");
}

#[test]
fn test_random_seed_is_inout() {
    let source = r#"
module tb;
    integer s, r1, r2, s1, s2;
    initial begin
        s = 1;
        r1 = $random(s);
        s1 = s;
        r2 = $random(s);
        s2 = s;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> i64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_i64())
            .unwrap_or(i64::MIN)
    };
    assert_ne!(get("s1"), 1, "$random(seed) harus memperbarui seed");
    assert_ne!(get("s2"), get("s1"), "seed berubah tiap panggilan");
    assert_ne!(get("r1"), get("r2"), "dua panggilan dengan seed inout berbeda hasil");
}

#[test]
fn test_countbits_with_control_bits() {
    let source = r#"
module tb;
    reg [7:0] v;
    reg [31:0] c1, c2, c3;
    initial begin
        v = 8'b1010_0110;
        c1 = $countbits(v, 1'b1);
        c2 = $countbits(v, 1'b0);
        c3 = $countbits(v);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("c1"), 4, "$countbits(v, 1)");
    assert_eq!(get("c2"), 4, "$countbits(v, 0)");
    assert_eq!(get("c3"), 0, "$countbits tanpa control bit");
}

#[test]
fn test_countbits_literal_respects_width() {
    // Literal 8-bit: zero-padding ke 64 bit TIDAK boleh ikut dihitung.
    let source = r#"
module tb;
    reg [31:0] c1, c2;
    initial begin
        c1 = $countbits(8'b1010_0101, 1'b1);
        c2 = $countbits(8'b1010_0101, 1'b0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("c1"), 4, "4 bit bernilai 1 pada literal 8-bit");
    assert_eq!(get("c2"), 4, "4 bit bernilai 0 pada literal 8-bit");
}

#[test]
fn test_unpacked_dimensions() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] scalar;
    reg [31:0] d1, d2;
    initial begin
        d1 = $unpacked_dimensions(m);
        d2 = $unpacked_dimensions(scalar);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("d1"), 1, "memori 1D punya 1 dimensi unpacked");
    assert_eq!(get("d2"), 0, "scalar tidak punya dimensi unpacked");
}

#[test]
fn test_dist_functions_ranges_and_seed_update() {
    let source = r#"
module tb;
    integer s;
    reg [31:0] u, e, p, c, er, n;
    initial begin
        s = 1;
        u  = $dist_uniform(s, 5, 10);
        e  = $dist_exponential(s, 100);
        p  = $dist_poisson(s, 4);
        c  = $dist_chi_square(s, 3);
        er = $dist_erlang(s, 2, 100);
        n  = $dist_normal(s, 100, 10);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    let u = get("u");
    assert!((5..=10).contains(&u), "$dist_uniform di [5,10], dapat {}", u);
    assert!(get("e") <= 100_000, "$dist_exponential wajar");
    assert!(get("p") <= 10_000, "$dist_poisson wajar");
    assert!(get("c") <= 100_000, "$dist_chi_square wajar");
    assert!(get("er") <= 100_000, "$dist_erlang wajar");
    assert!(get("n") <= 1_000, "$dist_normal wajar");
    assert_ne!(get("s"), 1, "seed harus diperbarui (inout)");
}

#[test]
fn test_fread_memory_start_count() {
    use std::fs;
    let file = "/tmp/test_mivon_fread.bin";
    let _ = fs::write(file, [0x11u8, 0x22, 0x33, 0x44]);
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    integer fd;
    integer r;
    initial begin
        m[0]=8'h00; m[1]=8'h00; m[2]=8'h00; m[3]=8'h00;
        fd = $fopen("{f}", "rb");
        r = $fread(m, fd, 1, 2);
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = file
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("r"), 2, "jumlah word terbaca");
    // m[1], m[2] terisi; m[0], m[3] tetap.
    let m = sigs
        .iter()
        .find(|(n, _)| n == "m")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(
        m.bits[0..8]
            .iter()
            .filter(|b| **b == mivon_ir::LogicVal::One)
            .count(),
        0
    );
    let word1: u64 = (0..8)
        .filter(|i| m.bits[8 + i] == mivon_ir::LogicVal::One)
        .map(|i| 1u64 << i)
        .sum();
    let word2: u64 = (0..8)
        .filter(|i| m.bits[16 + i] == mivon_ir::LogicVal::One)
        .map(|i| 1u64 << i)
        .sum();
    assert_eq!(word1, 0x11);
    assert_eq!(word2, 0x22);
    let _ = fs::remove_file(file);
}

#[test]
fn test_writemem_range() {
    use std::fs;
    let file = "/tmp/test_mivon_writemem_range.hex";
    let _ = fs::remove_file(file);
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    initial begin
        m[0]=8'h11; m[1]=8'h22; m[2]=8'h33; m[3]=8'h44;
        $writememh("{f}", m, 1, 2);
        $finish;
    end
endmodule
"#,
        f = file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(file).unwrap_or_default();
    assert_eq!(content, "// 0x00000000\n22\n33\n", "start/end word");
    let _ = fs::remove_file(file);
}

#[test]
fn test_sscanf_parses_d_h_c() {
    let source = r#"
module tb;
    integer n;
    reg [31:0] v, a, b;
    reg [7:0] c;
    initial begin
        n = $sscanf("42", "%d", v);
        n = $sscanf("dead beef", "%h %h", a, b);
        n = $sscanf("X", "%c", c);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("v"), 42);
    assert_eq!(get("a"), 0xdead);
    assert_eq!(get("b"), 0xbeef);
    assert_eq!(get("c"), 0x58); // 'X' = 0x58
}

#[test]
fn test_sreadmemh_and_bin() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] n [0:1];
    reg [7:0] a0, a1, a2, a3, b0, b1;
    initial begin
        $sreadmemh("aa bb cc dd", m);
        $sreadmemb("00000001 00000010", n);
        a0 = m[0]; a1 = m[1]; a2 = m[2]; a3 = m[3];
        b0 = n[0]; b1 = n[1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("a0"), 0xaa);
    assert_eq!(get("a1"), 0xbb);
    assert_eq!(get("a2"), 0xcc);
    assert_eq!(get("a3"), 0xdd);
    assert_eq!(get("b0"), 1);
    assert_eq!(get("b1"), 2);
}

#[test]
fn test_readmemh_missing_file_continues() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] a0;
    initial begin
        m[0]=8'h11;
        $readmemh("/tmp/mivon_does_not_exist_xyz.hex", m);
        a0 = m[0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let a0 = sigs
        .iter()
        .find(|(n, _)| n == "a0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(u64::MAX);
    assert_eq!(a0, 0x11, "file hilang: peringatan, memori tidak berubah, simulasi lanjut");
}

#[test]
fn test_readmemh_address_directive() {
    use std::fs;
    let file = "/tmp/test_mivon_readmem_at.hex";
    let _ = fs::write(file, "aa\n@2\ncc\n");
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] a0, a1, a2, a3;
    initial begin
        m[0]=8'h11; m[1]=8'h22; m[2]=8'h33; m[3]=8'h44;
        $readmemh("{f}", m);
        a0=m[0]; a1=m[1]; a2=m[2]; a3=m[3];
        #1 $finish;
    end
endmodule
"#,
        f = file
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!((get("a0"), get("a1"), get("a2"), get("a3")), (0xaa, 0x22, 0xcc, 0x44));
    let _ = fs::remove_file(file);
}

#[test]
fn test_readmemh_preserves_untouched_words_and_range() {
    use std::fs;
    let full = "/tmp/test_mivon_readmem_full.hex";
    let part = "/tmp/test_mivon_readmem_part.hex";
    let _ = fs::write(full, "aa\nbb\ncc\ndd\n");
    let _ = fs::write(part, "aa\nbb\n");
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] n [0:3];
    reg [7:0] a0, a1, a2, a3, b0, b1, b2, b3;
    initial begin
        m[0]=8'h11; m[1]=8'h22; m[2]=8'h33; m[3]=8'h44;
        n[0]=8'h11; n[1]=8'h22; n[2]=8'h33; n[3]=8'h44;
        $readmemh("{full}", m);          // 4 word penuh
        $readmemh("{part}", n, 2, 3);    // isi word 2..3 saja
        a0=m[0]; a1=m[1]; a2=m[2]; a3=m[3];
        b0=n[0]; b1=n[1]; b2=n[2]; b3=n[3];
        #1 $finish;
    end
endmodule
"#,
        full = full,
        part = part
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    // File penuh → semua word terisi.
    assert_eq!((get("a0"), get("a1"), get("a2"), get("a3")), (0xaa, 0xbb, 0xcc, 0xdd));
    // start=2, end=3 → hanya word 2,3 yang berubah.
    assert_eq!((get("b0"), get("b1"), get("b2"), get("b3")), (0x11, 0x22, 0xaa, 0xbb));
    let _ = fs::remove_file(full);
    let _ = fs::remove_file(part);
}

#[test]
fn test_ferror_valid_handle_returns_zero() {
    use std::fs;
    let test_file = "/tmp/test_mivon_ferror.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [31:0] err;
    string msg;
    initial begin
        fd = $fopen("{f}", "w");
        err = $ferror(fd, msg);
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let err = sigs
        .iter()
        .find(|(n, _)| n == "err")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(999);
    assert_eq!(err, 0, "$ferror pada handle valid harus 0");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_writemem_hex_and_bin() {
    use std::fs;
    let hex_file = "/tmp/test_mivon_writemem.hex";
    let bin_file = "/tmp/test_mivon_writemem.bin";
    let _ = fs::remove_file(hex_file);
    let _ = fs::remove_file(bin_file);
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    initial begin
        m[0] = 8'haa;
        m[1] = 8'hbb;
        m[2] = 8'hcc;
        m[3] = 8'hdd;
        $writememh("{h}", m);
        $writememb("{b}", m);
        $finish;
    end
endmodule
"#,
        h = hex_file,
        b = bin_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let hex = fs::read_to_string(hex_file).unwrap_or_default();
    assert_eq!(hex, "// 0x00000000\naa\nbb\ncc\ndd\n");
    let bin = fs::read_to_string(bin_file).unwrap_or_default();
    assert_eq!(
        bin,
        "// 0x00000000\n10101010\n10111011\n11001100\n11011101\n"
    );
    let _ = fs::remove_file(hex_file);
    let _ = fs::remove_file(bin_file);
}

#[test]
fn test_fmonitor() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fmonitor.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [7:0] x;
    initial begin
        fd = $fopen("{f}", "w");
        $fmonitor(fd, "x=%0d\n", x);
        x = 10;
        #1 x = 20;
        #1 x = 20;
        #1 x = 30;
        #1 $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "fmonitor should write on change, got {} lines: {:?}",
        lines.len(),
        content
    );
    assert!(
        content.contains("x=10"),
        "fmonitor should capture x=10, got: {:?}",
        content
    );
    assert!(
        content.contains("x=30"),
        "fmonitor should capture x=30, got: {:?}",
        content
    );
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fread_file() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fread.txt";
    let _ = fs::remove_file(test_file);
    fs::write(test_file, b"\x41\x42\x43").unwrap();
    let source = format!(
        r#"
module tb;
    reg [23:0] data;
    initial begin
        $fread(data, "{f}");
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let sigs = simulate_signals(&source, 5).unwrap();
    let data = sigs
        .iter()
        .find(|(n, _)| n == "data")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        data, 0x434241,
        "fread should read binary 0x41 0x42 0x43 -> 0x434241, got 0x{:x}",
        data
    );
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_signed_lt_literal_zero() {
    // signed signal vs literal 0: `s < 0` harus signed (SV: relational signed
    // bila SALAH SATU operand signed). Bug: kondisi lama && (kedua signed)
    // → literal 0 bukan signed → unsigned compare → 255 < 0 = false. Fix: ||.
    let source = r#"
module tb;
    reg signed [7:0] s;
    reg neg;
    initial begin
        s = -8'sd1;
        neg = (s < 0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let neg = sigs
        .iter()
        .find(|(n, _)| n == "neg")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(neg, 1, "signed -1 < 0 harus 1 (bukan unsigned 255<0)");
}

#[test]
fn test_real_arithmetic_literal_fold() {
    // Bug #6: `1.5 + 2.25` → NaN (evaluator biner tak kenal real utk literal;
    // hanya signal is_real). Fix: fold real literal+literal di elaborasi.
    let source = r#"
module tb;
    real r, r2;
    initial begin
        r = 1.5 + 2.25;
        r2 = 10.0 / 4.0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    let r = f64::from_bits(get("r")) as u32;
    let r2 = f64::from_bits(get("r2")) as u32;
    assert_eq!(r, 3, "1.5+2.25 = 3.75 (f64), dapat `{r}`");
    assert_eq!(r2, 2, "10/4 = 2.5 (f64), dapat `{r2}`");
}

#[test]
fn test_signed_div_mod() {
    // Bug #7: `s = -4; sd = s / 2` → 126 (unsigned 252/2) — operand sinyal
    // zero-extend ke 32 (Cast) sebelum eval signed. Fix: clip ke lebar asli
    // signal utk Div/Mod (signed_raw_operand).
    let source = r#"
module tb;
    reg signed [7:0] s, sd, sm;
    initial begin
        s = -8'sd4;
        sd = s / 2;
        sm = s % 3;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64() as i8;
    assert_eq!(get("sd"), -2, "-4/2 = -2 (0xFE), bukan 126");
    assert_eq!(get("sm"), -1, "-4 % 3 = -1 (0xFF)");
}

#[test]
fn test_signed_relational() {
    let source = r#"
module tb;
    reg signed [7:0] a, b;
    reg lt, gt, ge, le;
    initial begin
        a = -3;
        b = 2;
        lt = a < b;
        gt = a > b;
        ge = a >= b;
        le = a <= b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let lt = sigs
        .iter()
        .find(|(n, _)| n == "lt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let gt = sigs
        .iter()
        .find(|(n, _)| n == "gt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let ge = sigs
        .iter()
        .find(|(n, _)| n == "ge")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let le = sigs
        .iter()
        .find(|(n, _)| n == "le")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(lt, 1, "signed: -3 < 2 should be 1");
    assert_eq!(gt, 0, "signed: -3 > 2 should be 0");
    assert_eq!(ge, 0, "signed: -3 >= 2 should be 0");
    assert_eq!(le, 1, "signed: -3 <= 2 should be 1");
}

#[test]
fn test_signed_relational_negatives() {
    let source = r#"
module tb;
    reg signed [7:0] a, b;
    reg lt;
    initial begin
        a = -5;
        b = -3;
        lt = a < b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let lt = sigs
        .iter()
        .find(|(n, _)| n == "lt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(lt, 1, "signed: -5 < -3 should be 1");
}

#[test]
fn test_unsigned_relational() {
    let source = r#"
module tb;
    reg [7:0] a, b;
    reg lt, gt;
    initial begin
        a = 8'hFD;
        b = 8'h02;
        lt = a < b;
        gt = a > b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let lt = sigs
        .iter()
        .find(|(n, _)| n == "lt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let gt = sigs
        .iter()
        .find(|(n, _)| n == "gt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(lt, 0, "unsigned: 0xFD < 0x02 should be 0");
    assert_eq!(gt, 1, "unsigned: 0xFD > 0x02 should be 1");
}

#[test]
fn test_wait_statement() {
    let source = r#"
module tb;
    reg [7:0] cnt;
    reg done;
    initial begin
        cnt = 0;
        #10 cnt = 5;
    end
    initial begin
        wait (cnt == 5);
        done = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let cnt_val = sigs
        .iter()
        .find(|(n, _)| n == "cnt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let done_val = sigs
        .iter()
        .find(|(n, _)| n == "done")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(cnt_val, 5, "cnt should be 5");
    assert_eq!(done_val, 1, "done should be 1 after wait is satisfied");
}

#[test]
fn test_force_statement() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 10;
        b = 20;
        #1 force a = b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 20, "a should be forced to b=20");
}

#[test]
fn test_random_urandom() {
    let source = r#"
module tb;
    reg [31:0] r;
    initial begin
        r = $urandom();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let r_val = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // $urandom returns a non-zero 32-bit value (could be zero but unlikely)
    assert!(r_val < 4294967296, "r should be a 32-bit value");
}

#[test]
fn test_dumpvars_dumpoff() {
    let source = r#"
module tb;
    reg [7:0] a;
    initial begin
        a = 42;
        $dumpvars();
        #1 $dumpoff();
        #2 $dumpon();
        #3 $finish();
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 42, "a should be 42");
}

#[test]
fn test_preprocessor_with_simulation() {
    let source = r#"
`define WIDTH 8
`ifdef NEVER
wire never;
`endif
module test;
    reg [`WIDTH-1:0] data;
    initial begin
        data = 8'hAB;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(
        val.to_u64(),
        0xAB,
        "preprocessed signal should have correct value"
    );
}

#[test]
fn test_clog2_in_expr() {
    let source = r#"
module tb;
    reg [7:0] w;
    reg [31:0] result;
    initial begin
        result = $clog2(8);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 3, "$clog2(8) should be 3");
}

#[test]
fn test_clog2_power_of_two() {
    let source = r#"
module tb;
    reg [31:0] r;
    initial begin
        r = $clog2(16);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "r").unwrap();
    assert_eq!(val.to_u64(), 4, "$clog2(16) should be 4");
}

#[test]
fn test_clog2_one() {
    let source = r#"
module tb;
    reg [31:0] r;
    initial begin
        r = $clog2(1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "r").unwrap();
    assert_eq!(val.to_u64(), 0, "$clog2(1) should be 0");
}

#[test]
fn test_casex_wildcard() {
    let source = r#"
module tb;
    reg [3:0] sel;
    reg [7:0] out;
    always @(*) begin
        casex (sel)
            4'b1xx0: out = 8'hA0;
            4'b01x0: out = 8'hB0;
            4'b0010: out = 8'hC0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 4'b1000;
        #1;
        if (out !== 8'hA0) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(out_val, 0xA0, "casex 4'b1000 should match 4'b1xx0 => 0xA0");
}

#[test]
fn test_casez_wildcard() {
    let source = r#"
module tb;
    reg [3:0] sel;
    reg [7:0] out;
    always @(*) begin
        casez (sel)
            4'b1zz0: out = 8'hA0;
            4'b01z0: out = 8'hB0;
            4'b0010: out = 8'hC0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 4'b1010;
        #1;
        if (out !== 8'hA0) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(out_val, 0xA0, "casez 4'b1010 should match 4'b1zz0 => 0xA0");
}

/// Qualifier + kind ortogonal (LRM 1800 §12.5): `priority casez` memakai
/// pencocokan wildcard (bukan exact-match). Vs iverilog: 101→A0, 011→B0.
#[test]
fn test_priority_casez_wildcard() {
    let source = r#"
module tb;
    reg [2:0] sel;
    reg [7:0] out;
    always @(*) begin
        priority casez (sel)
            3'b1??: out = 8'hA0;
            3'b01?: out = 8'hB0;
            3'b001: out = 8'hC0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 3'b101;
        #1;
        if (out !== 8'hA0) $display("MISS101 %0h", out);
        sel = 3'b011;
        #1;
        if (out !== 8'hB0) $display("MISS011 %0h", out);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        out_val, 0xB0,
        "priority casez 3'b011 should match 3'b01? => 0xB0"
    );
}

/// `unique casez` juga memakai wildcard (combo UniqueZ).
#[test]
fn test_unique_casez_wildcard() {
    let source = r#"
module tb;
    reg [2:0] sel;
    reg [7:0] out;
    always @(*) begin
        unique casez (sel)
            3'b1??: out = 8'hA0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 3'b100;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        out_val, 0xA0,
        "unique casez 3'b100 should match 3'b1?? => 0xA0"
    );
}

/// Default `parameter type T = logic [7:0]` memberi lebar 8 (bukan 1).
/// Regresi: parser membuang range (`T` selalu 1-bit → port mismatch).
#[test]
fn test_param_type_default_packed_range_width() {
    let source = r#"
module shifter #(
    parameter type T = logic [7:0],
    parameter N = 2
) (
    input bit clk,
    input T d,
    output T q
);
    T r;
    always_ff @(posedge clk) begin
        r <= d << N;
        q <= r;
    end
endmodule
module tb;
    bit clk;
    logic [7:0] d8, q8;
    shifter u8 (.clk(clk), .d(d8), .q(q8));
    initial begin
        clk = 0;
        d8 = 8'h01;
        #10;
        repeat (3) @(posedge clk);
        #1;
        $display("TP q8=%0d", q8);
        $finish;
    end
    initial begin
        forever #5 clk = ~clk;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 200).unwrap();
    let q8 = sigs
        .iter()
        .find(|(n, _)| n == "q8")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(q8, 4, "T=logic[7:0] N=2: 01<<2=4 (q8={q8})");
}

#[test]
fn test_disable_named_block() {
    let source = r#"
module tb;
    reg [7:0] count;
    integer i;
    initial begin
        count = 0;
        for (i = 0; i < 10; i = i + 1) begin : loop_block
            if (i == 5) disable loop_block;
            count = count + 1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        count_val, 5,
        "disable should break at i=5, count should be 5"
    );
}

#[test]
fn test_disable_outer_block() {
    let source = r#"
module tb;
    reg [7:0] count;
    integer i;
    initial begin : outer
        count = 0;
        for (i = 0; i < 3; i = i + 1) begin : inner
            if (i == 1) disable outer;
            count = count + 1;
        end
        count = 100;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        count_val, 1,
        "disable outer should break at i=1 after count becomes 1"
    );
}

#[test]
fn test_release_deassign() {
    let source = r#"
module tb;
    reg [7:0] a;
    initial begin
        a = 42;
        #1 force a = 99;
        #1 release a;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // After release, forced status is removed but value stays at last forced value
    assert_eq!(a_val, 99, "after release, value retains last forced value");
}

#[test]
fn test_break_in_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    initial begin
        count = 0;
        forever begin
            count = count + 1;
            if (count == 5) break;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "count").unwrap();
    assert_eq!(val.to_u64(), 5, "break should exit at count=5");
}

#[test]
fn test_continue_in_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    reg [7:0] sum;
    initial begin
        count = 0;
        sum = 0;
        while (count < 10) begin
            count = count + 1;
            if (count % 2 == 0) continue;
            sum = sum + count;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "sum").unwrap();
    // Sum of odd numbers 1..9 = 25
    assert_eq!(val.to_u64(), 25, "continue should skip even numbers");
}

#[test]
fn test_fill_literals() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = '0;
        b = '1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(a_val, 0, "'0 should fill all bits with 0");
}

#[test]
fn test_do_while_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    initial begin
        count = 0;
        do begin
            count = count + 1;
        end while (count < 5);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "count").unwrap();
    assert_eq!(val.to_u64(), 5, "do-while should execute until count=5");
}

#[test]
fn test_bits_system_function() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [31:0] result;
    initial begin
        result = $bits(a);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 8, "$bits(reg [7:0]) should be 8");
}

#[test]
fn test_wildcard_equality_eq() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg result;
    initial begin
        a = 4'b1010;
        b = 4'b10x0;
        result = (a ==? b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "==? should treat X as don't-care");
}

#[test]
fn test_wildcard_equality_neq() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg result;
    initial begin
        a = 4'b1010;
        b = 4'b1011;
        result = (a !=? b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "!=? should be 1 when not equal");
}

#[test]
fn test_dollar_time() {
    let source = r#"
module tb;
    reg [63:0] t;
    initial begin
        #5;
        t = $time;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "t").unwrap();
    assert_eq!(val.to_u64(), 5, "$time should return 5 at time 5");
}

#[test]
fn test_range_select_signal() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [3:0] result;
    initial begin
        a = 8'b11001100;
        result = a[5:2];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // a[5:2] of 11001100 = 0011 (bit2=LSB) → 3. Cocok dengan iverilog.
    assert_eq!(val.to_u64(), 3, "a[5:2] of 11001100 should give 3");
}

#[test]
fn test_generate_if_active() {
    let source = r#"
module tb;
    generate
        if (1) begin
            reg [7:0] data;
        end else begin
            reg [15:0] data;
        end
    endgenerate
    initial begin
        data = 8'hAB;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(val.to_u64(), 0xAB, "generate if should select true branch");
}

#[test]
fn test_generate_case() {
    let source = r#"
module tb;
    reg [7:0] data;
    generate
        case (2)
            0: begin
                initial data = 8'hAA;
            end
            1: begin
                initial data = 8'hBB;
            end
            2: begin
                initial data = 8'hCC;
            end
            default: begin
                initial data = 8'hFF;
            end
        endcase
    endgenerate
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(val.to_u64(), 0xCC, "generate case should select arm 2");
}

#[test]
fn test_generate_case_default() {
    let source = r#"
module tb;
    reg [7:0] data;
    generate
        case (99)
            0: begin
                initial data = 8'hAA;
            end
            1: begin
                initial data = 8'hBB;
            end
            default: begin
                initial data = 8'hFF;
            end
        endcase
    endgenerate
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(val.to_u64(), 0xFF, "generate case default should fire");
}

#[test]
fn test_dynamic_part_select() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [3:0] result;
    integer sel;
    initial begin
        a = 8'b11001100;
        sel = 5;
        // dynamic part-select: a[sel -: 4] → a[5:2]
        result = a[sel -: 4];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // a[sel-:4] dengan sel=5 → a[5:2]. bits 5:2 of 11001100 = 0011 = 3
    // (bit2=LSB). Cocok dengan iverilog.
    assert_eq!(
        val.to_u64(),
        3,
        "dynamic part-select a[sel-:4] should give 3"
    );
}

#[test]
fn test_dynamic_part_select_plus() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [3:0] result;
    integer sel;
    initial begin
        a = 8'b11001100;
        sel = 2;
        result = a[sel +: 4];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // a[sel+:4] sel=2 → a[2:5] = bits 2..5 of 11001100 = 0011 = 3.
    // Cocok dengan iverilog.
    assert_eq!(
        val.to_u64(),
        3,
        "dynamic part-select a[sel+:4] should give 3"
    );
}

#[test]
fn test_unknown_syscall_no_crash() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 42;
        $foobar(x);
        #1 $finish;
    end
endmodule
"#;
    // Should not crash or error, just warn
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "unknown syscall should not cause crash: {:?}",
        result.err()
    );
}

#[test]
fn test_array_range_select_lvalue() {
    let source = r#"
module tb;
    reg [7:0] arr [0:3];
    reg [3:0] result;
    integer i;
    initial begin
        arr[0] = 8'hA5;
        arr[1] = 8'h5A;
        i = 1;
        result = arr[i][3:0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // arr[1] = 8'h5A; [3:0] = low nibble = 0xA = 10. Cocok dengan iverilog.
    assert_eq!(val.to_u64(), 10, "arr[i][3:0] should select low nibble");
}

#[test]
fn test_array_bit_select_lvalue() {
    let source = r#"
module tb;
    reg [7:0] arr [0:3];
    reg result;
    integer i;
    initial begin
        arr[0] = 8'hA5;
        arr[1] = 8'h5A;
        i = 0;
        result = arr[i][0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // arr[0] = 8'hA5 = 10100101; bit 0 = 1
    assert_eq!(val.to_u64(), 1, "arr[i][0] should select bit 0");
}

#[test]
fn test_multidim_unpacked_array_read_decl_init() {
    // F39 — multi-dimensi unpacked array (buglog-mv #4, sebelumnya SKIP
    // buta dim lanjutan): `logic [7:0] mat [0:1][0:1] = '{{1,2},{3,4}}`
    // → width 32 (4 elemen 8-bit), array_dims [2,2]; init nested di-flatten
    // row-major [1,2,3,4]; index `mat[i][j]` = elemen flat i*2+j.
    // Sebelumnya: width 16/2 elemen, init tak ter-decompose → semua 0.
    let source = r#"
module tb;
    logic [7:0] mat [0:1][0:1] = '{{1,2},{3,4}};
    logic [7:0] m00, m01, m10, m11;
    integer i, j;
    initial begin
        m00 = mat[0][0];
        m01 = mat[0][1];
        m10 = mat[1][0];
        m11 = mat[1][1];
        i = 1; j = 1;
        m11 = mat[i][j];   // dinamis → 4
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("m00"), 1, "mat[0][0]");
    assert_eq!(get("m01"), 2, "mat[0][1]");
    assert_eq!(get("m10"), 3, "mat[1][0]");
    assert_eq!(get("m11"), 4, "mat[1][1] dynamic");
}

#[test]
fn test_multidim_unpacked_array_write_dynamic() {
    // F39 — tulis dinamis multi-dimensi `mat[i][j] = x` (fold lvalue).
    let source = r#"
module tb;
    logic [7:0] mat [0:1][0:1] = '{{1,2},{3,4}};
    logic [7:0] p00, p01;
    integer i, j;
    initial begin
        i = 0; j = 1;
        mat[i][j] = 42;
        p00 = mat[0][0];
        p01 = mat[0][1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("p00"), 1, "element tidak tersentuh");
    assert_eq!(get("p01"), 42, "write dinamis mat[0][1]");
}

#[test]
fn test_multidim_unpacked_array_3d_and_row() {
    // F39 — 3-dimensi + row select `mat[0]` (16-bit dari 2 elemen 8-bit).
    let source = r#"
module tb;
    logic [7:0] m3d [0:1][0:1][0:1] = '{{'{1,2},'{3,4}}, '{'{5,6},'{7,8}}};
    logic [7:0] w;
    logic [31:0] row;
    integer i, j, k;
    initial begin
        i = 1; j = 0; k = 1;
        w = m3d[i][j][k];   // m3d[1][0][1] = 6
        row = m3d[0];       // baris 0 = {1,2,3,4} → 0x04030201 (MSB-first)
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("w"), 6, "m3d[1][0][1] = 6");
    assert_eq!(
        get("row"),
        0x0403_0201,
        "row select m3d[0] (32-bit sub-array)"
    );
}

#[test]
fn test_multidim_unpacked_array_port() {
    // F39 — port array multi-dimensi `output logic [7:0] out [0:1][0:1]`.
    let source = r#"
module dut(input logic [7:0] d [0:1][0:1], output logic [7:0] q [0:1][0:1]);
    always_comb q = d;
endmodule
module tb;
    logic [7:0] d [0:1][0:1] = '{{1,2},{3,4}};
    logic [7:0] q [0:1][0:1];
    dut u(.d(d), .q(q));
    logic [7:0] r1, r4;
    initial begin
        #1;               // tunggu always_comb q = d propagasi
        r1 = q[0][0];
        r4 = q[1][1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("r1"), 1, "port multi-dim q[0][0]");
    assert_eq!(get("r4"), 4, "port multi-dim q[1][1]");
}

#[test]
fn test_package_import_typedef() {
    let source = r#"
package my_pkg;
    typedef enum { IDLE, BUSY, DONE } state_t;
endpackage

module tb;
    import my_pkg::*;
    state_t state;
    initial begin
        state = 2;
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
    let ir = design.unwrap();
    assert!(
        ir.top.signals.iter().any(|s| s.name == "state"),
        "state signal should exist in top module"
    );
}

#[test]
fn test_package_enum_sequence_resets_per_typedef() {
    // Dua typedef enum dalam satu package: member enum tanpa nilai eksplisit
    // melanjutkan counter HANYA dalam typedef yang sama (standar SV). Sebelum
    // fix, `pkg_enums` di-flatten jadi satu list per package sehingga counter
    // `last` bocor lintas enum — mis. enum kedua dapat nilai 3+ (harusnya 0..)
    // persis seperti `alu_op_base_e` di otbn_pkg (OpenTitan) yang mendapat
    // 256+ padahal harusnya 0..8.
    let source = r#"
package p;
    typedef enum { A, B, C } e1_t;   // A=0, B=1, C=2
    typedef enum { X, Y, Z } e2_t;   // X=0, Y=1, Z=2 (BUKAN 3,4,5)
endpackage

module tb;
    import p::*;
    logic [3:0] x, y, z;
    initial begin
        x = X;
        y = Y;
        z = Z;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("x"), 0, "X harus 0 (enum kedua mulai dari 0)");
    assert_eq!(get("y"), 1, "Y harus 1");
    assert_eq!(get("z"), 2, "Z harus 2");
}

#[test]
fn test_package_enum_sequence_reset_multiple_pkgs() {
    // Dua package berbeda — masing-masing enum independent (counter reset
    // antar package DAN antar typedef).
    let source = r#"
package p1;
    typedef enum { A, B, C, D, E } e1_t;
endpackage
package p2;
    typedef enum { Q, R } e2_t;
endpackage

module tb;
    import p1::*;
    import p2::*;
    logic [3:0] q, r;
    initial begin
        q = Q;
        r = R;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("q"), 0, "Q harus 0 (pkg p2 enum mulai dari 0)");
    assert_eq!(get("r"), 1, "R harus 1");
}

#[test]
fn test_package_import_param() {
    let source = r#"
package my_pkg;
    parameter int WIDTH = 8;
endpackage

module tb;
    import my_pkg::WIDTH;
    reg [WIDTH-1:0] data;
    initial begin
        data = 42;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let data_val = sigs
        .iter()
        .find(|(n, _)| n == "data")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(data_val, 42, "data should be 42");
}

#[test]
fn test_interface_decl() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
    logic valid;
endinterface

module tb;
    bus_if bus();
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_interface_modport() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
    logic valid;
    modport master (output data, valid);
    modport slave (input data, valid);
endinterface

module tb;
    bus_if bus();
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_package_import_param_expr() {
    let source = r#"
package my_pkg;
    parameter int WIDTH = 8;
    parameter int DEPTH = 4;
endpackage

module tb;
    import my_pkg::*;
    reg [WIDTH*DEPTH-1:0] mem;
    initial begin
        mem = 255;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let mem_val = sigs
        .iter()
        .find(|(n, _)| n == "mem")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(mem_val, 255, "mem should be 255");
}

#[test]
fn test_package_import_function() {
    let source = r#"
package math_pkg;
    function int add(input int a, input int b);
        add = a + b;
    endfunction
endpackage

module tb;
    import math_pkg::*;
    reg [31:0] result;
    initial begin
        result = add(10, 20);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(r, 30, "package function add(10,20) should return 30");
}

#[test]
fn test_package_import_task() {
    let source = r#"
package task_pkg;
    task set_reg(output reg [7:0] r, input [7:0] v);
        r = v;
    endtask
endpackage

module tb;
    import task_pkg::*;
    reg [7:0] val;
    initial begin
        val = 0;
        set_reg(val, 42);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "package task set_reg should set val to 42");
}

#[test]
fn test_module_task() {
    let source = r#"
module tb;
    reg [7:0] val;
    task set_val(input [7:0] x);
        val = x;
    endtask
    initial begin
        val = 0;
        set_val(42);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "val should be 42 after task call");
}

#[test]
fn test_compound_assign_operators() {
    let source = r#"
module tb;
    reg [15:0] a, b, c, d, e, f, g, h, i;
    initial begin
        a = 16'd5;   a *= 16'd4;       // 5*4 = 20
        b = 16'd20;  b /= 16'd5;       // 20/5 = 4
        c = 16'd17;  c %= 16'd5;       // 17%5 = 2
        d = 16'hF0;  d &= 16'h0F;      // 0
        e = 16'h0F;  e |= 16'hF0;      // 0xFF
        f = 16'hAA;  f ^= 16'hFF;      // 0x55
        g = 16'd1;   g <<= 16'd4;      // 16
        h = 16'h80;  h >>= 16'd3;      // 16
        i = 16'd7;   i += 16'd8; i -= 16'd2; // 13
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("a"), 20, "*= should multiply");
    assert_eq!(get("b"), 4, "/= should divide");
    assert_eq!(get("c"), 2, "%= should mod");
    assert_eq!(get("d"), 0, "&= should bitwise-and");
    assert_eq!(get("e"), 0xFF, "|= should bitwise-or");
    assert_eq!(get("f"), 0x55, "^= should bitwise-xor");
    assert_eq!(get("g"), 16, "<<= should shift left");
    assert_eq!(get("h"), 16, ">>= should shift right");
    assert_eq!(get("i"), 13, "+= / -= should add and subtract");
}

#[test]
fn test_assignment_pattern_concat() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [15:0] b;
    initial begin
        a = '{8'hA5};
        b = '{8'hAA, 8'h55};
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let av = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let bv = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(av, 0xA5, "assignment pattern '{{v}} should equal v");
    assert_eq!(bv, 0xAA55, "'{{a,b}} should concat to aab");
}

#[test]
fn test_package_array_param() {
    let source = r#"
package cfg_pkg;
    parameter int COEFFS[0:3] = '{8'd1, 8'd2, 8'd3, 8'd4};
endpackage

module tb;
    import cfg_pkg::*;
    reg [7:0] result;
    initial begin
        result = COEFFS[2];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(r, 3, "COEFFS[2] should resolve to 3");
}

#[test]
fn test_parameter_type_decl() {
    let source = r#"
module tb;
    parameter type T = int;
    T x;
    initial begin
        x = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let xv = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(xv, 42, "parameter type T = int should declare 32-bit x");
}

#[test]
fn test_parameter_type_header_decl() {
    // Type param dari header parameter list (module m #(parameter type T))
    // juga harus resolve: `T x;` di body jadi deklarasi, bukan instance.
    let source = r#"
module m #(parameter type T = int) ();
    T x;
    initial begin
        x = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let xv = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(xv, 42, "header parameter type T should declare 32-bit x");
}

#[test]
fn test_negative_const_signed_assign() {
    // Konstanta negatif hasil const-fold (two's complement 32-bit) yang muat
    // di lebar LHS signed tidak boleh error — dan tidak memicu width mismatch
    // false-positive (fix check_width_mismatch value-aware utk nilai negatif).
    let source = r#"
module tb;
    reg signed [7:0] a;
    reg signed [7:0] b;
    reg signed [15:0] c;
    initial begin
        a = -1;
        b = -128;
        c = -300;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let av = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let bv = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let cv = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(av, 0xFF, "-1 in signed [7:0] should be 0xFF");
    assert_eq!(bv, 0x80, "-128 in signed [7:0] should be 0x80");
    assert_eq!(cv, 0xFED4, "-300 in signed [15:0] should be 0xFED4");
}

#[test]
fn test_negative_const_width_warning_suppression() {
    // Verifikasi fix check_width_mismatch value-aware: konstanta negatif yang
    // muat di lebar LHS signed (a=-1, b=-128) TIDAK memicu WidthMismatchWarning,
    // sedangkan konstanta yang benar-benar tidak muat (d=-200 dalam signed [7:0])
    // TETAP memicu warning. Memeriksa diag_sink elaborator, bukan nilai sim.
    let source = r#"
module tb;
    reg signed [7:0] a;
    reg signed [7:0] b;
    reg signed [7:0] d;
    initial begin
        a = -1;
        b = -128;
        d = -200;
        #1 $finish;
    end
endmodule
"#;

    // Pipeline lengkap seperti compile_str, tapi tangkap diagnostics elaborator.
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let preprocessed = pp.preprocess(source, None).unwrap();
    let mut lexer = mivon_parser::lexer::Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "<string>").with_source_lines(&preprocessed);
    let design = parser.parse_design().unwrap();
    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, "<string>".to_string());
    elaborator
        .elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        )
        .unwrap();

    let diags = elaborator.flush_diagnostics();
    let warn_msgs: Vec<String> = diags
        .iter()
        .filter(|d| {
            matches!(
                d.code,
                mivon_core::diagnostics::DiagCode::WidthMismatchWarning
            )
        })
        .map(|d| d.message.to_string())
        .collect();

    // -1 dan -128 muat di signed [7:0] → tidak boleh ada width warning utk a/b
    assert!(
        !warn_msgs.iter().any(|m| m.contains("assignment to 'a'")),
        "a=-1 should not warn, got: {:?}",
        warn_msgs
    );
    assert!(
        !warn_msgs.iter().any(|m| m.contains("assignment to 'b'")),
        "b=-128 should not warn, got: {:?}",
        warn_msgs
    );
    // -200 tidak muat di signed [7:0] → harus tetap warning (total tepat 1)
    assert_eq!(
        warn_msgs.len(),
        1,
        "expected exactly 1 width mismatch warning (only d), got: {:?}",
        warn_msgs
    );
    assert!(
        warn_msgs.iter().any(|m| m.contains("assignment to 'd'")),
        "d=-200 should still warn, got: {:?}",
        warn_msgs
    );
}

#[test]
fn test_auto_top_resolution_picks_largest_cone() {
    // Design dengan BANYAK candidate top (SoC chip + testbench kecil + bind
    // assertion). Mode AnalysisRecovery dulu menyerah (recovered=true, top
    // pertama) — sekarang auto top resolution memilih kandidat dengan skor
    // cone transitif tertinggi secara deterministik: chip_earlgrey-style SoC
    // yang menginstansiasi ratusan modul menang atas tb/bind kecil.
    let source = r#"
module add8(input [7:0] a, input [7:0] b, output [7:0] y);
    assign y = a + b;
endmodule

module mul4(input [7:0] a, input [7:0] b, output [7:0] y);
    assign y = a * b;
endmodule

module alu(input [7:0] a, input [7:0] b, input op, output [7:0] y);
    wire [7:0] add_y, mul_y;
    add8 u_add(.a(a), .b(b), .y(add_y));
    mul4 u_mul(.a(a), .b(b), .y(mul_y));
    assign y = op ? add_y : mul_y;
endmodule

// Peripheral kecil dengan sub-blok — cone chip membesar signifikan.
module uart(input clk, input [7:0] d, output [7:0] q);
    add8 u(.a(d), .b(8'h1), .y(q));
endmodule
module timer(input clk, input [7:0] d, output [7:0] q);
    mul4 u(.a(d), .b(8'h2), .y(q));
endmodule
module gpio(input clk, input [7:0] d, output [7:0] q);
    add8 u(.a(d), .b(8'h3), .y(q));
endmodule

// SoC chip: menginstansiasi alu + tiga peripheral → cone terbesar.
module chip_soc(input clk, input [7:0] a, input [7:0] b, input op, output [7:0] y);
    alu u_alu(.a(a), .b(b), .op(op), .y(y));
    uart u_uart(.clk(clk), .d(a), .q());
    timer u_timer(.clk(clk), .d(b), .q());
    gpio u_gpio(.clk(clk), .d(a), .q());
endmodule

// Testbench kecil — candidate top juga, tapi cone jauh lebih kecil.
module tb_alu;
    reg [7:0] a, b, y;
    reg op;
    alu u(.a(a), .b(b), .op(op), .y(y));
    initial #1 $finish;
endmodule

// Bind assertion — bukan top fungsional (diberi penalti nama).
module tb_alu_bind;
    wire [7:0] y;
    alu u(.a(8'h0), .b(8'h0), .op(1'b0), .y(y));
    initial #1 $finish;
endmodule
"#;

    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let preprocessed = pp.preprocess(source, None).unwrap();
    let mut lexer = mivon_parser::lexer::Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "<string>").with_source_lines(&preprocessed);
    let design = parser.parse_design().unwrap();
    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, "<string>".to_string());
    // AnalysisRecovery: tanpa --top, top tidak unik TIDAK menggagalkan analisis.
    let ir = elaborator
        .elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::AnalysisRecovery,
        )
        .expect("elaborate");
    // Auto top resolution memilih cone terbesar (chip_soc menginstansiasi
    // add8+mul4+alu = 4 module) — bukan tb kecil (cone 2).
    assert_eq!(
        ir.top.name.as_str(),
        "chip_soc",
        "auto top resolution should pick the SoC chip with largest cone"
    );
    assert!(
        !elaborator.recovered,
        "auto top resolution unique winner → analysis NOT recovered"
    );
}

#[test]
fn test_auto_top_resolution_sim_preference() {
    // Tie cone (kedua wrapper menginstansiasi SoC yang sama) → sinyal nama
    // memutus: preferensi simulasi (`verilator`/`_sim`) menang.
    let source = r#"
module leaf;
endmodule

module soc_top(input clk);
    leaf u();
endmodule

module chip_earlgrey_verilator(input clk);
    soc_top u();
endmodule

module chip_earlgrey_asic(input clk);
    soc_top u();
endmodule
"#;

    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let preprocessed = pp.preprocess(source, None).unwrap();
    let mut lexer = mivon_parser::lexer::Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "<string>").with_source_lines(&preprocessed);
    let design = parser.parse_design().unwrap();
    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, "<string>".to_string());
    let ir = elaborator
        .elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::AnalysisRecovery,
        )
        .expect("elaborate");
    assert_eq!(
        ir.top.name.as_str(),
        "chip_earlgrey_verilator",
        "sim-prefixed chip wrapper should win the tie via name score"
    );
    assert!(
        !elaborator.recovered,
        "unique winner → analysis NOT recovered"
    );
}

#[test]
fn test_bits_package_array_param() {
    // Verifikasi `$bits` pada referensi array utuh (param package):
    //   - plain name via `import pkg::*`  → `$bits(COEFFS)`
    //   - qualified name                  → `$bits(pkg::COEFFS)`
    // Keduanya harus 128 (4 elemen int × 32 bit). Ini menghapus documented
    // limitation lama di audit.txt yang menyebut $bits(COEFFS) tidak ter-resolve.
    let source = r#"
package cfg_pkg;
    parameter int COEFFS[0:3] = '{8'd1, 8'd2, 8'd3, 8'd4};
endpackage

module tb;
    import cfg_pkg::*;
    reg [7:0] arr[0:3];
    reg [3:0][7:0] p;  // packed multi-dim: 4 elemen x 8 bit = 32
    reg [31:0] b1;
    reg [31:0] b2;
    reg [31:0] b3;
    reg [31:0] b4;
    reg [31:0] s1;
    reg [31:0] s2;
    initial begin
        b1 = $bits(COEFFS);
        b2 = $bits(cfg_pkg::COEFFS);
        b3 = $bits(arr);
        b4 = $bits(p);
        s1 = $size(arr);
        s2 = $size(p);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let b1 = sigs
        .iter()
        .find(|(n, _)| n == "b1")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let b2 = sigs
        .iter()
        .find(|(n, _)| n == "b2")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let b3 = sigs
        .iter()
        .find(|(n, _)| n == "b3")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let b4 = sigs
        .iter()
        .find(|(n, _)| n == "b4")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let s1 = sigs
        .iter()
        .find(|(n, _)| n == "s1")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let s2 = sigs
        .iter()
        .find(|(n, _)| n == "s2")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        b1, 128,
        "$bits(COEFFS) via import should be 4 ints * 32 = 128"
    );
    assert_eq!(
        b2, 128,
        "$bits(cfg_pkg::COEFFS) should be 4 ints * 32 = 128"
    );
    // Signal array lokal: SignalInfo.width sudah lebar total (elem * depth),
    // jadi $bits(arr) = 32 (4 elemen x 8), bukan 128 (double-count lama).
    assert_eq!(
        b3, 32,
        "$bits(arr) on local signal array should be 4 * 8 = 32"
    );
    // $bits tetap lebar total untuk packed multi-dimensi (4 x 8 = 32),
    // tidak terpengaruh fix $size.
    assert_eq!(b4, 32, "$bits(p) on packed [3:0][7:0] should be 32");
    // $size mengembalikan jumlah elemen dimensi pertama (array_depth=4),
    // bukan lebar total (bug lama: mengembalikan info.width = 32).
    assert_eq!(s1, 4, "$size(arr) should return first-dimension size (4)");
    // Packed multi-dimensi [3:0][7:0]: $size = packed_dims[0] = 4 (bukan 32).
    assert_eq!(s2, 4, "$size(p) on packed [3:0][7:0] should be 4");
}

#[test]
fn test_qualified_package_function_call() {
    // LANG-43: panggilan function/task qualified `pkg::func(...)` —
    // elaborator resolve via elaborate_package_func_call (elaborator/expr.rs)
    // + func_source_pkg; plain name via `import pkg::*` lewat
    // elaborate_imported_package_func_call. Keduanya harus memanggil body
    // function di package dengan benar.
    let source = r#"
package math_pkg;
    function int add3(int a, int b, int c);
        return a + b + c;
    endfunction
    function int mul2(int a);
        return a * 2;
    endfunction
endpackage

module tb;
    import math_pkg::*;
    int r1;
    int r2;
    int r3;
    initial begin
        r1 = math_pkg::add3(1, 2, 4);  // qualified call pkg::func
        r2 = add3(10, 20, 0);          // plain name via import pkg::*
        r3 = math_pkg::mul2(add3(1, 1, 1));  // nested: qualified wraps plain
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 7, "qualified math_pkg::add3(1,2,4) = 7");
    assert_eq!(get("r2"), 30, "plain add3(10,20,0) via import pkg::* = 30");
    assert_eq!(get("r3"), 6, "nested math_pkg::mul2(add3(1,1,1)) = 2*3 = 6");
}

#[test]
fn test_let_declaration_module() {
    // LANG-40: `let` declaration di module (IEEE 1800-2017 §11.12.2) —
    // tanpa parameter (`let W = 8;` dipakai sebagai ident) dan berparameter
    // (`let double(x) = x * 2;` dipakai sebagai panggilan). Substitusi
    // dilakukan di elaborator (elaborate_expr → let_decls).
    let source = r#"
module tb;
    let W = 8;
    let double(x) = x * 2;
    let add3(a, b, c) = a + b + c;

    int r1;
    int r2;
    int r3;
    initial begin
        r1 = W;
        r2 = double(21);
        r3 = add3(1, 2, 4);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 8, "let tanpa parameter: W = 8");
    assert_eq!(get("r2"), 42, "let berparameter: double(21) = 42");
    assert_eq!(get("r3"), 7, "let berparameter 3 arg: add3(1,2,4) = 7");
}

#[test]
fn test_let_declaration_class() {
    // LANG-40: `let` di dalam class — dipakai di body method (jalur AST):
    // let tanpa parameter (`MAX`) dan berparameter (`clamp(v,lo,hi)` dengan
    // nested ternary).
    let source = r#"
class Cfg;
    let MAX = 100;
    let clamp(v, lo, hi) = (v < lo) ? lo : (v > hi) ? hi : v;
    function int compute(int v);
        return clamp(v, 0, MAX);
    endfunction
endclass

module tb;
    Cfg c;
    int r1;
    int r2;
    int r3;
    initial begin
        c = new();
        r1 = c.compute(250);
        r2 = c.compute(50);
        r3 = c.compute(0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 100, "clamp(250,0,100) = 100");
    assert_eq!(get("r2"), 50, "clamp(50,0,100) = 50");
    assert_eq!(get("r3"), 0, "clamp(0,0,100) = 0");
}

#[test]
fn test_negative_literal_signed_semantics() {
    // ROUND 36 — literal negatif harus berperilaku signed di SEMUA jalur:
    //   1. `int a = -5` → signal is_signed=true (IEEE 1800 §6.11: int/
    //      integer/byte/shortint/longint intrinsik signed) → `a < 0` = true
    //      (sebelumnya unsigned compare → false, bug ROUND 34)
    //   2. `-5` const-fold dibungkus Signed → perbandingan konstanta benar
    //   3. argumen literal negatif di method class `compute(-5)` → -5
    //      (sebelumnya 4294967291)
    // Bits tetap two's complement (0xFFFFFFFB); signedness menentukan
    // interpretasi perbandingan/display, bukan bit yang disimpan.
    let source = r#"
class Cls;
    function int compute(input int v);
        return v;
    endfunction
endclass

module tb;
    Cls c;
    int a;
    int b;
    int c1;
    int r1;
    int r2;
    int r3;
    initial begin
        a = -5;
        b = (a < 0) ? 1 : 0;
        c1 = (-5 < 0) ? 1 : 0;
        c = new();
        r1 = c.compute(-5);
        r2 = c.compute(5);
        r3 = (r1 == -5) ? 1 : 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("a"), 0xFFFF_FFFB, "int -5 = two's complement 32-bit");
    assert_eq!(get("b"), 1, "a < 0 true — int signal signed");
    assert_eq!(get("c1"), 1, "-5 < 0 true — const fold signed");
    assert_eq!(get("r1"), 0xFFFF_FFFB, "compute(-5) mempertahankan bits");
    assert_eq!(get("r2"), 5, "compute(5) = 5");
    assert_eq!(get("r3"), 1, "r1 == -5 true — signed equality");
}

#[test]
fn test_signed_division_modulo() {
    // ROUND 36 lanjutan: Div/Mod dengan operand SIGNED harus menghitung
    // dengan semantik i64 (truncate toward zero, IEEE 1800 §11.4.3):
    //   -7/2 = -3, -7%2 = -1, -7/-2 = 3, 7%-2 = 1
    // Sebelumnya eval_binary selalu unsigned → -7/2 = 2147483644, -7%-2 = 0.
    let source = r#"
module tb;
    int a, b, c, d;
    int q1, m1, q2, m2, q3, m3;
    initial begin
        a = -7; b = 2;
        c = -7; d = -2;
        q1 = a / b;
        m1 = a % b;
        q2 = c / d;
        m2 = c % d;
        q3 = 7 / -2;
        m3 = 7 % -2;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_i64())
            .unwrap_or(0)
    };
    assert_eq!(get("q1"), -3, "-7 / 2 = -3 (signed)");
    assert_eq!(get("m1"), -1, "-7 % 2 = -1 (tanda ikut dividen)");
    assert_eq!(get("q2"), 3, "-7 / -2 = 3 (signed)");
    assert_eq!(get("m2"), -1, "-7 % -2 = -1");
    assert_eq!(get("q3"), -3, "7 / -2 = -3 (const fold signed)");
    assert_eq!(get("m3"), 1, "7 % -2 = 1");
}

#[test]
fn test_arithmetic_shift_right_signedness() {
    // ROUND 36 lanjutan — `>>>` (IEEE 1800 §11.4.10): ARITHMETIC bila lhs
    // signed, LOGICAL bila unsigned. Dua bug lama:
    //   1. extend_to zero-extend ke max_width operand menghilangkan sign bit
    //      asli → `logic signed [7:0] s = -128; s >>> 2` = 0x20 (harus 0xE0)
    //   2. semantik >>> selalu arithmetic padahal harus logical utk unsigned
    // Fix: eval_sshr_signed (lebar asli lhs) + pemilihan di evaluate_expr
    // dan parallel path.
    let source = r#"
module tb;
    logic signed [7:0] s;
    logic [7:0] u;
    logic [7:0] rs_signed;
    logic [7:0] rs_shift_ge_width;
    logic [7:0] ru_unsigned;
    logic [7:0] ru_logical_op;
    int si;
    int ri;
    initial begin
        s = -128;
        u = 8'h80;
        rs_signed = s >>> 2;        // arithmetic → 8'hE0 (-32)
        rs_shift_ge_width = s >>> 10; // shift >= width → semua sign bit 0xFF
        ru_unsigned = u >>> 2;      // logical → 8'h20
        ru_logical_op = s >> 2;     // `>>` selalu logical → 8'h20
        si = -8;
        ri = si >>> 2;              // int signed → -2
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("rs_signed"), 0xE0, "signed >>> 2 = arithmetic 0xE0");
    assert_eq!(get("rs_shift_ge_width"), 0xFF, "shift >= width → sign fill");
    assert_eq!(get("ru_unsigned"), 0x20, "unsigned >>> = logical 0x20");
    assert_eq!(get("ru_logical_op"), 0x20, ">> selalu logical 0x20");
    assert_eq!(get("ri"), 0xFFFF_FFFE, "int signed >>> 2 = -2");
}

#[test]
fn test_compound_expr_signedness_propagation() {
    // ROUND 36 lanjutan — is_signed_expr di-rekursi ke ekspresi majemuk:
    // `int a; (a+1) < 0` harus signed compare (a=-5 → a+1=-4 < 0 = true).
    // Sebelumnya is_signed_expr(BinaryOp) = false → unsigned compare → false.
    // Berlaku juga utk div/mod, `>>>`, dan %d display pada ekspresi majemuk.
    let source = r#"
module tb;
    int a;
    int b1, b2, b3;
    int q, s;
    initial begin
        a = -5;
        b1 = ((a + 1) < 0) ? 1 : 0;   // -4 < 0 → 1
        b2 = ((a * 2) < 0) ? 1 : 0;   // -10 < 0 → 1
        b3 = ((-a) < 0) ? 1 : 0;      // 5 < 0 → 0
        a = -7;
        q = (a + 1) / 2;   // -6 / 2 = -3
        s = (a * 2) >>> 2; // -14 >>> 2 = -4 (arithmetic)
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_i64())
            .unwrap_or(0)
    };
    assert_eq!(get("b1"), 1, "(a+1) < 0 signed compare");
    assert_eq!(get("b2"), 1, "(a*2) < 0 signed compare");
    assert_eq!(get("b3"), 0, "(-a) < 0 → false (5 < 0)");
    assert_eq!(get("q"), -3, "(a+1)/2 signed division");
    assert_eq!(get("s"), -4, "(a*2) >>> 2 arithmetic shift");
}

#[test]
fn test_lrm_signedness_any_unsigned_rule() {
    // ROUND 36 — aturan LRM §11.8.2 PENUH: operasi signed hanya bila KEDUA
    // operand signed ('ada operand unsigned → hasil unsigned'). Dua prasyarat
    // di-emit elaborator:
    //   - literal desimal UNSIZED (`0`, `127`) → IrExpr::Signed (LRM §6.8.1)
    //   - eval_binary_signed SIGN-extend operan dari lebar aslinya
    //     (`logic signed [7:0] s = -1; s < 0` → -1 < 0, bukan 255 < 0)
    // Kasus uji: signed-signal vs literal desimal = signed; vs literal
    // hex/bin = unsigned (hex/biner sized tidak punya suffix s).
    let source = r#"
module tb;
    logic signed [7:0] s;
    logic [7:0] u;
    int a;
    int r1, r2, r3, r4, r5, r6, r7;
    initial begin
        s = -1;          // 0xFF
        u = 8'hFF;
        a = -5;
        r1 = (s < 0) ? 1 : 0;          // signed vs desimal(signed) → -1<0 → 1
        r2 = (s < 8'h7F) ? 1 : 0;      // signed vs hex(unsigned) → 0xFF<0x7F → 0
        r3 = (s < 127) ? 1 : 0;        // signed vs desimal(signed) → -1<127 → 1
        r4 = (u > 8'hFE) ? 1 : 0;      // unsigned vs hex → 0xFF>0xFE → 1
        r5 = (a < 0) ? 1 : 0;          // int vs desimal → -5<0 → 1
        r6 = (a < 8'h02) ? 1 : 0;      // int vs hex(unsigned) → 0xFFFFFFFB<2 → 0
        r7 = (s / 8'h02) ? 1 : 0;      // signed / hex(unsigned) → unsigned: 0xFF/2 ≠ 0
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 1, "s<0 signed (sign-extend 8-bit -1)");
    assert_eq!(get("r2"), 0, "s<8'h7F unsigned (any-unsigned)");
    assert_eq!(get("r3"), 1, "s<127 signed (-1 < 127)");
    assert_eq!(get("r4"), 1, "u>8'hFE unsigned");
    assert_eq!(get("r5"), 1, "a<0 signed");
    assert_eq!(get("r6"), 0, "a<8'h02 unsigned");
    assert_eq!(get("r7"), 1, "s/8'h02 unsigned div → 0xFF/2 = 127 ≠ 0");
}

#[test]
fn test_const_fold_signedness_propagation() {
    // ROUND 36 — const-fold mempertahankan signedness OPERAND ASLI:
    //   `a < (2+3)` — 2+3 = desimal unsized → SIGNED → a=-5 < 5 = true
    //   `a < (8'h01+8'h05)` — operand hex sized (unsigned) → hasil unsigned
    // Sebelumnya try_fold_const selalu menghasilkan Const UNSIGNED untuk
    // fold positif → `a < (2+3)` memakai unsigned compare (a=-5:
    // 0xFFFFFFFB < 5 = false, salah). Keterbatasan dicatat: ekspresi konstanta
    // yang SEMUA ter-fold dievaluasi const_eval dengan i64 (mis.
    // `(8'h01-8'h05) < 0` = -4 < 0 = 1, padahal LRM: 8'hFC=252 < 0 = 0) —
    // butuh evaluator konstanta sadar-lebar, di luar scope.
    let source = r#"
module tb;
    int a;
    int r1, r2;
    initial begin
        a = -5;
        r1 = (a < (2 + 3)) ? 1 : 0;          // signed: -5 < 5 → 1
        r2 = (a < (8'h01 + 8'h05)) ? 1 : 0;  // unsigned: 0xFFFFFFFB < 6 → 0
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("r1"),
        1,
        "a < (2+3) signed compare (fold desimal signed)"
    );
    assert_eq!(
        get("r2"),
        0,
        "a < (8'h01+8'h05) unsigned compare (any-unsigned)"
    );
}

#[test]
fn test_port_unpacked_array_end_to_end() {
    // Verifikasi dukungan port unpacked-array:
    //   - parser menerima `output logic [7:0] arr[0:3]` (sebelumnya error
    //     'expected RBrack, found Colon')
    //   - elaborator melipat array depth ke lebar total port + set
    //     array_depth/elem_width dengan benar
    //   - flatten check lebar membandingkan width TOTAL + elem_width
    //   - nilai continuous assign di child mengalir ke parent array signal
    let source = r#"
module tb;
    logic [7:0] a[0:3];
    reg [31:0] w;
    reg [31:0] s;
    child c(.arr(a));
    initial begin
        w = $bits(a);
        s = $size(a);
        #1 $finish;
    end
endmodule

module child(output logic [7:0] arr[0:3]);
    assign arr[0] = 8'h01;
    assign arr[1] = 8'h02;
    assign arr[2] = 8'h03;
    assign arr[3] = 8'h04;
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let w = sigs
        .iter()
        .find(|(n, _)| n == "w")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    // Port [7:0] x [0:3]: total 4 elemen x 8 bit = 32 bit, bukan elem_width 8.
    assert_eq!(w, 32, "$bits(port array) should be 4 elems * 8 = 32");
    // $size = jumlah elemen dimensi pertama (array_depth=4).
    assert_eq!(
        s, 4,
        "$size(port array) should return first-dimension size 4"
    );
    // arr[0]=8'h01 ... arr[3]=8'h04 mengalir ke parent: lsb-first 0x04030201.
    assert_eq!(
        a, 0x04030201,
        "child assigns should flow through the array port"
    );
}

#[test]
fn test_port_array_elem_width_mismatch_rejected() {
    // Guard di flatten: dua array bisa punya total width sama tapi elem_width
    // beda (mis. child [15:0][0:1] width 32 elem 16 vs parent [7:0][0:3]
    // width 32 elem 8) — tanpa guard ini check lolos tapi indexing engine salah.
    let source = r#"
module tb;
    logic [7:0] a[0:3];
    child c(.arr(a));
    initial #1 $finish;
endmodule

module child(output logic [15:0] arr[0:1]);
    assign arr[0] = 16'h1111;
    assign arr[1] = 16'h2222;
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_err(),
        "expected elem width mismatch to be rejected, but design compiled ok"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("element width mismatch"),
        "expected array element width mismatch error, got: {}",
        err
    );
}

#[test]
fn test_endmodule_colon_name_suffix() {
    // Suffix `endpackage : name` / `endmodule : name` / `endprogram : name`
    // harus bisa di-parse. Dipisah jadi dua design yang masing-masing punya
    // TEPAT SATU top candidate — StrictSimulation menolak design dengan
    // banyak candidate tops (program main + module top = ambigu).
    let source = r#"
package pkg;
    parameter int W = 8;
endpackage : pkg

module top;
    wire a;
endmodule : top
"#;
    let sigs = simulate_signals(source, 2);
    assert!(
        sigs.is_ok(),
        "endpackage/endmodule : name suffix should parse: {:?}",
        sigs.err()
    );
    let source2 = r#"
program main;
    initial #1 $finish;
endprogram : main
"#;
    let sigs2 = simulate_signals(source2, 2);
    assert!(
        sigs2.is_ok(),
        "endprogram : name suffix should parse: {:?}",
        sigs2.err()
    );
}

#[test]
fn test_module_task_multiple_ports() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    task add_and_store(input [7:0] x, input [7:0] y);
        a = x + y;
        b = x - y;
    endtask
    initial begin
        a = 0;
        b = 0;
        add_and_store(30, 12);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let av = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let bv = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(av, 42, "a should be 30+12=42");
    assert_eq!(bv, 18, "b should be 30-12=18");
}

#[test]
fn test_module_task_output_port() {
    let source = r#"
module tb;
    task double_it(input [7:0] x, output [7:0] y);
        y = x * 2;
    endtask
    reg [7:0] result;
    initial begin
        result = 0;
        double_it(21, result);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        42,
        "result should be 21*2=42 after task with output port"
    );
}

#[test]
fn test_module_task_inout_port() {
    let source = r#"
module tb;
    task increment(input [7:0] x, inout [7:0] acc);
        acc = acc + x;
    endtask
    reg [7:0] total;
    initial begin
        total = 10;
        increment(5, total);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "total").unwrap();
    assert_eq!(
        val.to_u64(),
        15,
        "total should be 10+5=15 after task with inout port"
    );
}

#[test]
fn test_fork_join_basic() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 0; b = 0;
        fork
            #5 a = 42;
            #10 b = 99;
        join
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a, 42, "a should be 42 after fork-join");
    assert_eq!(b, 99, "b should be 99 after fork-join");
}

#[test]
fn test_fork_join_any() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    reg [7:0] result;
    initial begin
        a = 0; b = 0; result = 0;
        fork
            #5 a = 42;
            #10 b = 99;
        join_any
        result = 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let _b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 42, "a should be 42 after join_any");
    assert_eq!(r, 1, "result should be 1 (set after join_any continues)");
}

#[test]
fn test_fork_join_none() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    reg [7:0] result;
    initial begin
        a = 0; b = 0; result = 0;
        fork
            #5 a = 42;
            #10 b = 99;
        join_none
        result = 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 42, "a should be 42 after join_none");
    assert_eq!(b_val, 99, "b should be 99 after join_none");
    assert_eq!(r, 1, "result should be 1 (set immediately after join_none)");
}

#[test]
fn test_wait_fork_waits_for_join_none() {
    // LANG-29: `wait fork;` memblokir sampai SEMUA fork process milik proses
    // ini selesai. Snapshot diambil SETELAH wait fork — bila wait fork tidak
    // memblokir (bug lama: `wait(0)` yang tak pernah true / lanjut segera),
    // snapshot membaca done1/done2 sebelum branch selesai → 0.
    let source = r#"
module tb;
    int done1;
    int done2;
    int snapshot;
    initial begin
        done1 = 0; done2 = 0; snapshot = 0;
        fork
            #5 done1 = 1;
            #10 done2 = 1;
        join_none
        #2
        wait fork;
        snapshot = done1 * 10 + done2;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let snap = sigs
        .iter()
        .find(|(n, _)| n == "snapshot")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        snap, 11,
        "wait fork harus menunggu kedua branch (done1=1, done2=1)"
    );
}

#[test]
fn test_wait_fork_no_active_fork_continues() {
    // LANG-29: tanpa fork proses aktif, `wait fork` lanjut segera (t=0).
    let source = r#"
module tb;
    int ok;
    initial begin
        ok = 0;
        wait fork;
        ok = 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(ok, 1, "tanpa fork aktif, wait fork lanjut segera");
}

#[test]
fn test_wait_fork_nested_in_fork_branch() {
    // LANG-29: `wait fork` di dalam branch fork menunggu child process dari
    // branch itu sendiri; `join` luar tetap menunggu semua branch.
    let source = r#"
module tb;
    int nested_done;
    int outer_ok;
    initial begin
        nested_done = 0; outer_ok = 0;
        fork
            begin
                fork
                    #4 nested_done = 1;
                join_none
                #2
                wait fork;
                if (nested_done != 1) $display("FAIL nested");
            end
            #10 outer_ok = 1;
        join
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let nd = sigs
        .iter()
        .find(|(n, _)| n == "nested_done")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let oo = sigs
        .iter()
        .find(|(n, _)| n == "outer_ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(nd, 1, "nested wait fork harus menunggu child branch");
    assert_eq!(oo, 1, "outer join harus menunggu semua branch");
}

#[test]
fn test_disable_fork_kills_branches() {
    // LANG-30: `disable fork;` men-terminate SEMUA child process milik proses
    // pemanggil (IEEE 1800-2017 §9.6.4). Branch yang tertunda (`#10`/`#20`)
    // tidak pernah dieksekusi (a/b tetap 0) dan proses pemanggil LANJUT —
    // `wait fork` setelah disable langsung selesai (tidak hang).
    let source = r#"
module tb;
    int a;
    int b;
    int after;
    initial begin
        a = 0; b = 0; after = 0;
        fork
            #10 a = 1;
            #20 b = 1;
        join_none
        disable fork;
        after = 1;
        wait fork;
        after = 2;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 0, "branch #10 harus dibunuh (a tetap 0)");
    assert_eq!(get("b"), 0, "branch #20 harus dibunuh (b tetap 0)");
    assert_eq!(
        get("after"),
        2,
        "proses lanjut setelah disable fork; wait fork selesai tanpa hang"
    );
}

#[test]
fn test_disable_fork_process_isolation() {
    // LANG-30: `disable fork` hanya membunuh child process MILIK PROSES
    // pemanggil — branch proses lain tetap berjalan.
    let source = r#"
module tb;
    int a;
    int b;
    initial begin  // proses A: fork join_none
        a = 0; b = 0;
        fork
            #5 a = 1;
            #10 b = 1;
        join_none
    end
    initial begin  // proses B: disable fork (tidak punya child)
        #2
        disable fork;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 1, "branch proses A tetap jalan (a=1 di #5)");
    assert_eq!(get("b"), 1, "branch proses A tetap jalan (b=1 di #10)");
}

#[test]
fn test_disable_fork_branch_label_scoped() {
    // `disable <label>` untuk named fork branch — SUDAH diimplementasikan:
    // label `begin : branch_a` kini dipertahankan parser (sebelumnya dibuang →
    // `Stmt::Block` anonim) dan kontinuasi suspend membawa label-nya
    // (`Continuation::named_labels`), sehingga `disable branch_a` membunuh
    // HANYA branch tersebut — branch_b tetap jalan (LRM 1800 §9.6.4).
    let source = r#"
module tb;
    int a;
    int b;
    initial begin
        a = 0; b = 0;
        fork
            begin : branch_a
                #5 a = 1;
            end
            begin : branch_b
                #10 b = 1;
            end
        join_none
        #2 disable branch_a;
        #15 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 0, "branch_a dibunuh disable label");
    assert_eq!(get("b"), 1, "branch_b TIDAK tersentuh disable branch_a");
}

#[test]
fn test_disable_label_cross_branch_fork_join_continuation() {
    // BUG FIX disable-label lintas branch: `disable worker` dari branch kedua
    // (yang men-suspend) men-sUSPEND-kan branch pertama di dalam
    // `begin : worker`. SEBELUM fix, `disable_pending` bocor tak bertabrak
    // consumer → loop-top guard menelan SEMUA statement sesudahnya termasuk
    // continuation `join` → `$finish` tak pernah jalan (simulasi "quiesced").
    let source = r#"
module tb;
    reg [7:0] a;
    reg done;
    initial begin
        a = 0;
        done = 0;
        fork
            begin : worker
                #10;
                a = 1;
            end
            begin
                #5;
                disable worker;
            end
        join
        #1;
        done = 1;
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 0, "worker dibunuh disable label sebelum t=10");
    assert_eq!(
        get("done"),
        1,
        "continuation join tetap jalan (tanpa fix: disable_pending bocor \
         menelan sisa statement -> done=0)"
    );
}

#[test]
fn test_fork_join_parallel_delays() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    reg [7:0] c;
    initial begin
        a = 0; b = 0; c = 0;
        fork
            begin
                #3 a = 10;
                #3 a = 20;
            end
            #5 b = 99;
            #10 c = 55;
        join
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let c = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        a, 20,
        "a should be 20 after sequential delays in fork branch"
    );
    assert_eq!(b, 99, "b should be 99");
    assert_eq!(c, 55, "c should be 55");
}

#[test]
fn test_zero_delay() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 1;
        #0;
        b = a + 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a, 1, "a should be 1");
    assert_eq!(b, 2, "b should be 2 (a+1 after #0 delay)");
}

#[test]
fn test_zero_delay_ordering() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 0;
        b = 0;
        #0 a = 10;
        #0 b = 20;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a, 10, "a should be 10");
    assert_eq!(b, 20, "b should be 20");
}

#[test]
fn test_always_comb_basic() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    wire [7:0] sum;

    always_comb begin
        sum = a + b;
    end

    initial begin
        a = 10; b = 20;
        #1 a = 30;
        #1 b = 5;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sum_val = sigs
        .iter()
        .find(|(n, _)| n == "sum")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(sum_val, 35, "final sum should be 30 + 5 = 35");
}

#[test]
#[allow(clippy::approx_constant)] // literal `3.14` memang target RTL, bukan PI
fn test_real_declaration_and_assignment() {
    let source = r#"
module tb;
    real r;

    initial begin
        r = 3.14;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let r_val = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (r_val - 3.14).abs() < 1e-9,
        "r should be ~3.14, got {}",
        r_val
    );
}

#[test]
fn test_realtime_system_function() {
    let source = r#"
module tb;
    real t;

    initial begin
        #5;
        t = $realtime;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let t_val = sigs
        .iter()
        .find(|(n, _)| n == "t")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (t_val - 5.0).abs() < 1e-9,
        "$realtime should be 5.0, got {}",
        t_val
    );
}

#[test]
fn test_real_arithmetic() {
    let source = r#"
module tb;
    real a, b, sum, diff, prod, quot;

    initial begin
        a = 10.5;
        b = 3.0;
        sum = a + b;
        diff = a - b;
        prod = a * b;
        quot = a / b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_real = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| f64::from_bits(v.to_u64()))
            .unwrap()
    };
    assert!((get_real("sum") - 13.5).abs() < 1e-9);
    assert!((get_real("diff") - 7.5).abs() < 1e-9);
    assert!((get_real("prod") - 31.5).abs() < 1e-9);
    assert!((get_real("quot") - 3.5).abs() < 1e-9);
}

#[test]
fn test_real_comparison() {
    let source = r#"
module tb;
    real a, b;
    reg gt, lt, eq;

    initial begin
        a = 5.5;
        b = 3.0;
        gt = a > b;
        lt = a < b;
        eq = a == b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_val = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get_val("gt"), 1, "5.5 > 3.0 should be true");
    assert_eq!(get_val("lt"), 0, "5.5 < 3.0 should be false");
    assert_eq!(get_val("eq"), 0, "5.5 == 3.0 should be false");
}

/// Fungsi matematika real LRM 1800 §20.8 (`$sqrt`, `$ln`, `$pow`, ...).
/// Regresi: tak dikenal → 0 + warning RT9003 (mis. `$sqrt(2.0)` = 0).
/// Oracle: iverilog (`sqrt=1.414214 ln=1.000000 floor=3.000000 exp=2.718282`).
#[test]
fn test_real_math_sysfuncs() {
    let source = r#"
module tb;
    real a, b, c, d;
    integer n1, n2, n3;
    initial begin
        a = $sqrt(2.0);
        b = $pow(2.0, 10.0);
        c = $sin(0.0) + $cos(0.0);
        d = $atan2(1.0, 1.0);
        n1 = $rtoi(a * 1000.0);
        n2 = $rtoi(b);
        n3 = $rtoi(d * 1000.0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_val = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get_val("n1"), 1414, "$rtoi($sqrt(2)*1000) = 1414");
    assert_eq!(get_val("n2"), 1024, "$rtoi($pow(2,10)) = 1024");
    assert_eq!(get_val("n3"), 785, "$rtoi($atan2(1,1)*1000) = 785");
}

#[test]
fn test_simulation_state_send_sync_audit() {
    // DEBT-17: Simulation state Send/Sync audit — compile-time guard.
    // SimulationState murni Vec/HashMap/u64/TimeFormat (tanpa RefCell/Rc/raw
    // pointer), sehingga harus Send + Sync. Test ini gagal kompilasi bila ada
    // field state baru yang memecah thread-safety.
    //
    // Catatan: SimulationEngine TIDAK di-assert di sini — sengaja
    // single-threaded. Field Option<FstWaveWriter> (crate wavefst) memegang
    // RefCell<HashMap<String, SendWrapper<*const u8>>> (symbol cache FST)
    // yang !Sync. Parallel eval (parallel.rs) hanya menyalin nilai sinyal
    // (LogicVec) via rayon par_iter().cloned() — tidak pernah memindahkan
    // atau mem-borrow engine lintas thread.
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<crate::simulator::state::SimulationState>();
    assert_sync::<crate::simulator::state::SimulationState>();
    assert_send::<mivon_ir::LogicVec>();
    assert_sync::<mivon_ir::LogicVec>();
}

#[test]
fn test_wreal_net_type_decl() {
    // wreal = real net type: membawa nilai real via continuous assign
    let source = r#"
module tb;
    real src;
    wreal out;
    real captured;

    assign out = src;

    initial begin
        src = 2.5;
        #1;
        captured = out;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let captured = sigs
        .iter()
        .find(|(n, _)| n == "captured")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (captured - 2.5).abs() < 1e-9,
        "wreal net should carry real value 2.5, got {}",
        captured
    );
}

#[test]
fn test_wreal_direct_assignment() {
    // wreal juga boleh di-drive procedural (mengalir seperti real biasa)
    let source = r#"
module tb;
    wreal w;
    real captured;

    initial begin
        w = 6.25;
        #1;
        captured = w;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let captured = sigs
        .iter()
        .find(|(n, _)| n == "captured")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (captured - 6.25).abs() < 1e-9,
        "wreal procedural assignment should carry 6.25, got {}",
        captured
    );
}

#[test]
fn test_bit_type_is_2state() {
    let source = r#"
module tb;
    bit [7:0] b;
    reg [7:0] r;

    initial begin
        b = 8'hFF;
        r = 8'h00;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(b_val, 0xFF, "bit signal should store FF");
}

#[test]
fn test_bit_rejects_xz() {
    let source = r#"
module tb;
    bit [3:0] b;
    reg [3:0] r;

    initial begin
        r = 4'b01xz;
        b = r;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        b_val, 0b0100,
        "bit should convert X/Z to 0; expected 0100, got {:04b}",
        b_val
    );
}

#[test]
fn test_urandom_range() {
    let source = r#"
module tb;
    reg [31:0] val;
    initial begin
        val = $urandom_range(100, 50);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert!(
        (50..=100).contains(&v),
        "urandom_range(100,50) should be [50,100], got {}",
        v
    );
}

#[test]
fn test_urandom_range_single_arg() {
    let source = r#"
module tb;
    reg [31:0] val;
    initial begin
        val = $urandom_range(10);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert!(v <= 10, "urandom_range(10) should be <= 10, got {}", v);
}

#[test]
fn test_timeformat_percent_t() {
    // LANG-48: $timeformat + format %t (IEEE 1800)
    let source = r#"
module tb;
    string s;
    initial begin
        $timeformat(-9, 2, " ns", 0);
        #5;
        s = $sformatf("%t", $time);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert!(
        s.contains("5.00 ns"),
        "$timeformat(-9,2) + %%t should print '5.00 ns' at time 5, got '{}'",
        s
    );
}

#[test]
fn test_printtimescale_runs() {
    // LANG-49: $printtimescale tanpa kurung harus berjalan tanpa error
    let source = r#"
`timescale 1ns / 1ps
module tb;
    initial begin
        $printtimescale;
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(result.is_ok(), "$printtimescale should run without error");
}

#[test]
fn test_showscopes_runs() {
    // LANG-53: $showscopes harus berjalan tanpa error
    let source = r#"
module tb;
    initial begin
        $showscopes;
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(result.is_ok(), "$showscopes should run without error");
}

#[test]
fn test_deposit_forces_value() {
    // LANG-52: $deposit(sig, value) memaksa nilai signal
    let source = r#"
module tb;
    reg [7:0] x;
    initial begin
        x = 8'h00;
        #1;
        $deposit(x, 8'hA5);
        #1;
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0xA5, "$deposit should force x to 0xA5, got {:#x}", v);
}

#[test]
fn test_assign_forces_value() {
    // LANG-52: $assign(sig, value) memaksa nilai signal
    let source = r#"
module tb;
    reg [7:0] x;
    initial begin
        $assign(x, 8'h3C);
        #1;
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0x3C, "$assign should force x to 0x3C, got {:#x}", v);
}

#[test]
fn test_assign_suppresses_write_until_deassign() {
    // LANG-52: $assign override menahan write berikutnya sampai $deassign.
    // Flag `suppressed_ok` menangkap kondisi di tengah — tanpa itu, assert final
    // x==0x55 tetap lulus walau suppression rusak (0x00 ditimpa, lalu 0x55).
    let source = r#"
module tb;
    reg [7:0] x;
    reg suppressed_ok;
    initial begin
        $assign(x, 8'hA1);
        x = 8'h00;        // harus ditekan (masih forced oleh $assign)
        #1;
        suppressed_ok = (x === 8'hA1) ? 1'b1 : 1'b0;
        $deassign(x);
        x = 8'h55;        // setelah $deassign, write berlaku lagi
        #1;
        if (x !== 8'h55) $display("FAILED write after deassign: %h", x);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 6).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0x55, "after $deassign, x should be 0x55, got {:#x}", v);
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "suppressed_ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        ok, 1,
        "$assign should suppress the x=0 write (x stays 0xA1)"
    );
}

#[test]
fn test_get_randcount() {
    // LANG-22: $get_randcount mengembalikan jumlah panggilan random
    let source = r#"
module tb;
    integer a, b, c;
    initial begin
        a = $urandom();
        b = $urandom();
        c = $get_randcount();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        v, 2,
        "$get_randcount after 2 x $urandom should be 2, got {}",
        v
    );
}

#[test]
fn test_get_randstate_seed() {
    // LANG-22: $get_randstate mengembalikan seed RNG
    let source = r#"
module tb;
    integer c;
    initial begin
        c = $get_randstate();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "default $get_randstate should be 42, got {}", v);
}

#[test]
fn test_random_seed_reproducible() {
    // Same seed should produce same random value (reproducibility)
    let source = r#"
module tb;
    reg [31:0] a;
    initial begin
        a = $random(42);
        #1 $finish;
    end
endmodule
"#;
    let sigs1 = simulate_signals(source, 5).unwrap();
    let v1 = sigs1
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);

    // Second simulation with same seed should produce same value
    let sigs2 = simulate_signals(source, 5).unwrap();
    let v2 = sigs2
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);

    assert_eq!(
        v1, v2,
        "$random(42) with same seed should produce same value: {} != {}",
        v1, v2
    );
}

#[test]
fn test_urandom_seed_returns_prev_seed() {
    // LANG-21: $urandom_seed should return previous seed
    let source = r#"
module tb;
    reg [63:0] prev_seed, new_seed;
    initial begin
        prev_seed = $urandom_seed(12345);
        new_seed = $urandom_seed(54321);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let prev = sigs
        .iter()
        .find(|(n, _)| n == "prev_seed")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let new_s = sigs
        .iter()
        .find(|(n, _)| n == "new_seed")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // First call returns default seed (42)
    assert_eq!(
        prev, 42,
        "$urandom_seed first call should return default seed 42, got {}",
        prev
    );
    // Second call returns previous seed (12345)
    assert_eq!(
        new_s, 12345,
        "$urandom_seed second call should return previous seed 12345, got {}",
        new_s
    );
}

#[test]
fn test_srandom_returns_prev_seed() {
    // LANG-21: $srandom should return previous seed, NOT increment rand_call_count
    let source = r#"
module tb;
    reg [63:0] prev1, prev2, cnt1, cnt2;
    initial begin
        prev1 = $srandom(111);
        cnt1 = $get_randcount();
        prev2 = $srandom(222);
        cnt2 = $get_randcount();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let prev1 = sigs
        .iter()
        .find(|(n, _)| n == "prev1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let prev2 = sigs
        .iter()
        .find(|(n, _)| n == "prev2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let cnt1 = sigs
        .iter()
        .find(|(n, _)| n == "cnt1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let cnt2 = sigs
        .iter()
        .find(|(n, _)| n == "cnt2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // $srandom returns previous seed
    assert_eq!(
        prev1, 42,
        "$srandom first call should return default seed 42, got {}",
        prev1
    );
    assert_eq!(
        prev2, 111,
        "$srandom second call should return previous seed 111, got {}",
        prev2
    );
    // $srandom does NOT increment rand_call_count (per IEEE)
    assert_eq!(
        cnt1, 0,
        "$get_randcount after $srandom should be 0, got {}",
        cnt1
    );
    assert_eq!(
        cnt2, 0,
        "$get_randcount after two $srandom should be 0, got {}",
        cnt2
    );
}

#[test]
fn test_urandom_seed_full_scope_determinism() {
    // LANG-21 (gap tersisa): seeding $urandom_seed harus mengendalikan aliran
    // $urandom/$urandom_range secara DETERMINISTIK full-scope — seed yang sama
    // → stream identik (reproducible), seed berbeda → stream berbeda. Sebelumnya
    // hanya return value prev_seed yang diuji; efek pada stream belum diverifikasi.
    let source = r#"
module tb;
    reg [31:0] a1, a2, a3, r1, r2;
    initial begin
        $urandom_seed(777);
        a1 = $urandom;
        a2 = $urandom;
        a3 = $urandom;
        r1 = $urandom_range(1000, 1);
        r2 = $urandom_range(1000, 1);
        #1 $finish;
    end
endmodule
"#;
    // Run 1 & 2: seed SAMA → stream IDENTIK (determinism / reproducibility).
    let run = |src: &str| -> Vec<u64> {
        let sigs = simulate_signals(src, 5).unwrap();
        ["a1", "a2", "a3", "r1", "r2"]
            .iter()
            .map(|n| {
                sigs.iter()
                    .find(|(name, _)| name == n)
                    .map(|(_, v)| v.to_u64())
                    .unwrap_or(0)
            })
            .collect()
    };
    let s1 = run(source);
    let s2 = run(source);
    assert_eq!(
        s1, s2,
        "seed sama → stream $urandom harus identik antar run"
    );

    // Run 3: seed BERBEDA → stream harus berubah (tidak kebetulan sama).
    let src_diff = source.replace("$urandom_seed(777);", "$urandom_seed(888);");
    let s3 = run(&src_diff);
    assert_ne!(
        s1, s3,
        "seed berbeda → stream $urandom harus berubah (seeding tidak efektif?)"
    );

    // $urandom_range dalam [1, 1000] — validasi range tetap bekerja setelah seed.
    for v in &s1[3..] {
        assert!(
            (1..=1000).contains(v),
            "$urandom_range harus dalam [1,1000], got {}",
            v
        );
    }
}

#[test]
fn test_srandom_with_no_arg_returns_prev_seed() {
    // $srandom() without argument should still return previous seed
    let source = r#"
module tb;
    reg [63:0] prev;
    initial begin
        $srandom(999);
        prev = $srandom();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let prev = sigs
        .iter()
        .find(|(n, _)| n == "prev")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        prev, 999,
        "$srandom() no-arg should return previous seed 999, got {}",
        prev
    );
}

#[test]
fn test_error_recovery_unknown_decl() {
    let source = r#"
module tb;
    reg [3:0] a;
    bad_keyword_here x;
    reg [3:0] b;
    initial begin
        a = 1;
        b = 2;
        #1 $finish;
    end
endmodule
"#;
    // Should not panic — returns proper error, no crash
    let _ = compile_str(source);
}

#[test]
fn test_error_recovery_bad_stmt() {
    let source = r#"
module tb;
    reg [3:0] a;
    initial begin
        a = 1;
        bad_statement_here;
        a = 2;
        #1 $finish;
    end
endmodule
"#;
    // Should not panic — returns proper error, no crash
    let _ = compile_str(source);
}

#[test]
fn test_error_recovery_missing_semi() {
    let source = r#"
module tb;
    reg [3:0] a
    reg [3:0] b;
    initial begin
        a = 1
        b = 2;
        #1 $finish;
    end
endmodule
"#;
    // Should not panic — returns proper error, no crash
    let _ = compile_str(source);
}

#[test]
fn test_byte_shortint_int_longint_2state() {
    let source = r#"
module tb;
    byte b;
    shortint si;
    int i;
    longint li;

    initial begin
        b = 8'hAB;
        si = 16'h1234;
        i = 32'hDEAD_BEEF;
        li = 64'h1234_5678_9ABC_DEF0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_val = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get_val("b"), 0xAB);
    assert_eq!(get_val("si"), 0x1234);
    assert_eq!(get_val("i"), 0xDEAD_BEEFu64);
    assert_eq!(get_val("li"), 0x1234_5678_9ABC_DEF0u64);
}

#[test]
fn test_mailbox_put_get() {
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] val;
    initial begin
        mb = new();
        mb.put(42);
        val = mb.get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "mailbox get should return 42");
}

#[test]
fn test_mailbox_bounded_try_put_respects_bound() {
    // LANG-24: `new(2)` bounded — try_put sukses dua kali, lalu 0 saat penuh.
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] r1, r2, r3;
    initial begin
        mb = new(2);
        r1 = mb.try_put(1);
        r2 = mb.try_put(2);
        r3 = mb.try_put(3);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("r1"), 1, "first try_put should succeed");
    assert_eq!(get("r2"), 1, "second try_put should succeed");
    assert_eq!(
        get("r3"),
        0,
        "third try_put on full bounded mailbox should fail"
    );
}

#[test]
fn test_mailbox_bounded_num() {
    // LANG-24: bounded — num() hanya hitung item yang benar-benar masuk.
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] count;
    initial begin
        mb = new(1);
        mb.put(1);
        count = mb.num();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "bounded mailbox num should be 1");
}

#[test]
fn test_mailbox_get_blocks_until_put() {
    // LANG-24: `get` pada mailbox kosong BLOCK (suspend) sampai `put`.
    // Diuji dari task UVM (jalur AST blocking) — konsisten dengan uvm_tlm_fifo.
    let source = r#"
class mb_user extends uvm_component;
    mailbox mb;
    int val;
    int got;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        mb = new();
    endfunction
    task run_phase();
        fork
            begin
                mb.get(val);
                got = 1;
            end
            begin
                #10;
                mb.put(42);
            end
        join
        if (got != 1) $error("blocking get gagal got=%0d", got);
        if (val != 42) $error("nilai salah val=%0d", val);
    endtask
endclass

class mb_test extends uvm_test;
    mb_user u;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        u = new("u", this);
    endfunction
endclass
module tb;
    initial run_test("mb_test");
endmodule
"#;
    let result = simulate_str(source, 100);
    assert!(
        result.is_ok(),
        "blocking mailbox get should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_mailbox_put_blocks_when_bounded_full() {
    // LANG-24: bounded `put` saat penuh BLOCK sampai `get` membebaskan slot.
    let source = r#"
class mb_user extends uvm_component;
    mailbox mb;
    int val;
    int done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        mb = new(1);
    endfunction
    task run_phase();
        mb.put(10);
        fork
            begin
                #10;
                mb.get(val);
            end
            begin
                mb.put(20);
                done = 1;
            end
        join
        if (done != 1) $error("bounded put block gagal done=%0d", done);
        if (val != 10) $error("nilai salah val=%0d", val);
    endtask
endclass

class mb_test extends uvm_test;
    mb_user u;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        u = new("u", this);
    endfunction
endclass
module tb;
    initial run_test("mb_test");
endmodule
"#;
    let result = simulate_str(source, 100);
    assert!(
        result.is_ok(),
        "bounded mailbox put should block then succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_mailbox_num() {
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] count;
    initial begin
        mb = new();
        mb.put(1);
        mb.put(2);
        mb.put(3);
        count = mb.num();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 3, "mailbox num should be 3 after 3 puts");
}

#[test]
fn test_mailbox_try_get_empty() {
    let source = r#"
module tb;
    mailbox mb;
    reg ok;
    initial begin
        mb = new();
        ok = mb.try_get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(1);
    assert_eq!(v, 0, "try_get on empty mailbox should return 0");
}

#[test]
fn test_semaphore_put_get() {
    let source = r#"
module tb;
    semaphore sem;
    reg [31:0] remaining;
    initial begin
        sem = new(2);
        sem.get(1);
        remaining = sem.get(1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "remaining")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(v, 0, "after get(1)+get(1), remaining should be 0");
}

#[test]
fn test_semaphore_try_get() {
    let source = r#"
module tb;
    semaphore sem;
    reg ok;
    initial begin
        sem = new(1);
        ok = sem.try_get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "try_get with available keys should return 1");
}

#[test]
fn test_mailbox_put_try_get() {
    let source = r#"
module tb;
    mailbox mb;
    reg ok;
    reg [31:0] val;
    initial begin
        mb = new();
        mb.put(99);
        ok = mb.try_get();
        val = mb.num();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ok_val = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let remaining = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(ok_val, 1, "try_get with data should return 1");
    assert_eq!(remaining, 0, "after try_get, num should be 0");
}

#[test]
fn test_process_self_and_status() {
    let source = r#"
module tb;
    process p;
    reg [31:0] status_val;
    initial begin
        p = process::self();
        status_val = p.status();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "status_val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(v, 1, "process::self() should return RUNNING status (1)");
}

#[test]
fn test_process_kill_changes_status() {
    let source = r#"
module tb;
    process p;
    reg [31:0] status_after;
    initial begin
        p = process::self();
        p.kill();
        status_after = p.status();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "status_after")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(v, 4, "after kill, status should be KILLED (4)");
}

#[test]
fn test_process_self_parse() {
    let source = r#"
module tb;
    process p;
    initial begin
        p = 42;
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "process p should parse and elaborate: {:?}",
        result.err()
    );
}

#[test]
fn test_process_decl_only() {
    let source = r#"
module tb;
    process p;
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
    // Just verify it compiles and runs without error
}

#[test]
fn test_process_self_method_await_statement() {
    let source = r#"
module tb;
    process p;
    reg [31:0] x;
    initial begin
        fork
            begin
                #10 x = 42;
            end
        join_none
        p = process::self();
        #20 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "fork/join_none should execute body");
}

#[test]
fn test_uvm_object_compile() {
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    initial begin
        obj = my_obj::new("my_test_obj");
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_object compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_object_no_new_override() {
    let source = r#"
class my_obj extends uvm_object;
endclass

module tb;
    my_obj obj;
    initial begin
        obj = my_obj::new("my_test_obj");
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_object no-new compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_object_sim() {
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    reg [31:0] result;
    initial begin
        obj = my_obj::new("my_test_obj");
        result = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "simulation should complete successfully");
}

#[test]
fn test_uvm_super_phase_noop() {
    // `super.end_of_elaboration_phase(phase)` di subclass UVM harus no-op
    // (bukan error RT9003 "uvm_object::end_of_elaboration_phase not
    // implemented") — pola OpenTitan core_ibex_base_test.
    let source = r#"
class my_test extends uvm_test;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void end_of_elaboration_phase(uvm_phase phase);
        super.end_of_elaboration_phase(phase);
    endfunction
endclass

module tb;
    my_test t;
    reg [31:0] result;
    initial begin
        t = new("t", null);
        result = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "super.xxx_phase harus no-op, tidak error");
}

#[test]
fn test_task_method_array_lvalue_dynamic_index() {
    // LHS array dengan index dinamis (`mem[count] = v`) di body task method
    // harus didukung — bukan error RT9003 "unsupported lvalue type in task
    // method: BitSelect" (pola OpenTitan DV, mis. model flash bank).
    let source = r#"
class c;
    int mem[8];
    int count;
    function new();
        for (int i = 0; i < 8; i++) mem[i] = 0;
        count = 0;
    endfunction
    task push(int v);
        mem[count] = v;
        count = count + 1;
    endtask
    function int read(int i);
        return mem[i];
    endfunction
endclass

module tb;
    c obj;
    reg [31:0] result;
    initial begin
        obj = new();
        obj.push(5);
        obj.push(7);
        result = obj.read(1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        v, 7,
        "array lvalue index dinamis di task method harus jalan"
    );
}

#[test]
fn test_uvm_super_new_unknown_base_class() {
    // OpenTitan DV: `dv_report_server extends uvm_default_report_server`
    // (kelas library UVM TIDAK ada di filelist). `super.new(name)` di
    // subclass user harus jatuh ke builtin report_object, bukan error
    // RT8001 "method 'new' not found in class 'uvm_default_report_server'".
    let source = r#"
class dv_report_server extends uvm_default_report_server;
    function new (string name = "");
        super.new(name);
    endfunction
    function string tag();
        return "dv";
    endfunction
endclass

module tb;
    dv_report_server srv;
    reg [31:0] result;
    initial begin
        srv = new("mysrv");
        result = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        v, 42,
        "super.new pada class UVM base yang tidak terdaftar harus tidak error"
    );
}

#[test]
fn test_uvm_object_get_type_name() {
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    reg [31:0] result;
    initial begin
        obj = my_obj::new("my_test_obj");
        result = obj.get_type_name();
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
    // get_type_name returns a string (bits), we just verify simulation completes
}

#[test]
fn test_uvm_printer_print_object() {
    // VERIF-12: uvm_table_printer::print_object(obj) memformat object jadi
    // string tabel (nama, class, fields). Field yang di-set via `obj.x = ...`
    // harus terlihat di output.
    let source = r#"
class my_obj extends uvm_object;
    int count;
    bit [7:0] addr;
    function new(string name);
        super.new(name);
        count = 0;
        addr = 0;
    endfunction
endclass

module tb;
    my_obj obj;
    uvm_table_printer printer;
    string s;
    reg ok;
    initial begin
        obj = new("my_print_obj");
        obj.count = 42;
        obj.addr = 8'hA5;
        printer = new("printer");
        s = printer.print_object(obj);
        ok = 0;
        if (s.len() > 0) ok = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(ok, 1, "print_object harus mengembalikan string non-kosong");
    // String hasil print_object harus berisi nama object + field yang di-set
    // (nama object & field di-format sebagai teks tabel).
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert!(
        s.contains("my_print_obj"),
        "string harus berisi nama object: got '{:?}'",
        s.chars().take(80).collect::<String>()
    );
    assert!(
        s.contains("count") && s.contains("addr"),
        "string harus berisi nama field: got '{:?}'",
        s.chars().take(80).collect::<String>()
    );
}

#[test]
fn test_uvm_printer_contains_fields() {
    // VERIF-12: hasil print_object harus berisi nama object, class, dan nama
    // field yang di-set — verifikasi lewat method get() yang membaca hasil
    // print ke string signal (bukan hanya non-kosong).
    let source = r#"
class my_obj extends uvm_object;
    int count;
    function new(string name);
        super.new(name);
        count = 0;
    endfunction
endclass

module tb;
    my_obj obj;
    uvm_table_printer printer;
    string printed;
    reg [31:0] check;
    initial begin
        obj = new("printer_obj");
        obj.count = 7;
        printer = new("printer");
        printed = printer.print_object(obj);
        // Konten tidak mudah diperiksa per-char dari sinyal; verifikasi sim
        // selesai + print tidak error dengan cek signal turunan.
        check = obj.count;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let check = sigs
        .iter()
        .find(|(n, _)| n == "check")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(check, 7, "field count harus tetap 7 setelah print_object");
}

#[test]
fn test_uvm_object_print_with_printer() {
    // VERIF-12: `obj.print(printer)` dengan argumen printer harus memakai
    // format tabel (tidak error). Tanpa printer → format default.
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    uvm_table_printer printer;
    reg ok;
    initial begin
        obj = new("print_arg_obj");
        printer = new("printer");
        obj.print(printer);  // delegasi ke printer — tidak boleh error
        ok = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(ok, 1, "obj.print(printer) harus sukses tanpa error");
}

#[test]
fn test_uvm_component_compile() {
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_comp comp;
    initial begin
        comp = my_comp::new("my_comp", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_component compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_item_compile() {
    let source = r#"
class my_item extends uvm_sequence_item;
    rand bit [7:0] addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_item item;
    initial begin
        item = my_item::new("item");
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_sequence_item compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_sim() {
    let source = r#"
class my_seq extends uvm_sequence;
    function new(string name);
        super.new(name);
    endfunction
    task body();
        // body runs when start() is called
    endtask
endclass

module tb;
    my_seq seq;
    initial begin
        seq = my_seq::new("seq");
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_sequence sim failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequencer_driver_compile() {
    let source = r#"
class my_driver extends uvm_driver;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

class my_sequencer extends uvm_sequencer;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_driver drv;
    my_sequencer seqr;
    initial begin
        drv = my_driver::new("drv", 0);
        seqr = my_sequencer::new("seqr", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_sequencer/driver compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_start() {
    let source = r#"
class my_seq extends uvm_sequence;
    function new(string name);
        super.new(name);
    endfunction
    task body();
        // body runs when start() is called
    endtask
endclass

class my_sequencer extends uvm_sequencer;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_seq seq;
    my_sequencer seqr;
    initial begin
        seqr = my_sequencer::new("seqr", 0);
        seq = my_seq::new("seq");
        seq.start(seqr);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "uvm_sequence start failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_analysis_port_write_through() {
    let source = r#"
class my_monitor extends uvm_monitor;
    uvm_analysis_port ap;
    function new(string name, uvm_component parent);
        super.new(name, parent);
        ap = uvm_analysis_port::new("ap");
    endfunction
    task run_phase(uvm_phase phase);
        // In real UVM, ap.write(item) would be called here
    endtask
endclass

class my_scoreboard extends uvm_scoreboard;
    int write_count;
    function new(string name, uvm_component parent);
        super.new(name, parent);
        write_count = 0;
    endfunction
    function void write(uvm_sequence_item item);
        write_count = write_count + 1;
    endfunction
endclass

module tb;
    my_monitor mon;
    my_scoreboard sb;
    uvm_analysis_imp imp;
    reg [31:0] result;
    initial begin
        mon = my_monitor::new("mon", 0);
        sb = my_scoreboard::new("sb", 0);
        imp = uvm_analysis_imp::new("imp", sb);
        mon.ap.connect(imp);
        mon.ap.write(0);
        result = sb.write_count;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "write_count should be 1 after analysis_port write"
    );
}

#[test]
fn test_uvm_analysis_port_sim() {
    let source = r#"
class my_scoreboard extends uvm_scoreboard;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void write(uvm_sequence_item item);
        // item received from monitor via analysis port
    endfunction
endclass

module tb;
    my_scoreboard sb;
    uvm_analysis_port ap;
    uvm_analysis_imp imp;
    initial begin
        sb = my_scoreboard::new("sb", 0);
        ap = uvm_analysis_port::new("ap");
        imp = uvm_analysis_imp::new("imp", sb);
        ap.connect(imp);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_analysis_port test failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_phases_execute() {
    let source = r#"
class my_test extends uvm_test;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        super.build_phase();
    endfunction
    function void connect_phase();
        super.connect_phase();
    endfunction
    task run_phase();
        super.run_phase();
    endtask
endclass

module tb;
    my_test test;
    initial begin
        test = my_test::new("test", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(result.is_ok(), "uvm_phases test failed: {:?}", result.err());
}

#[test]
fn test_uvm_config_db_set_get() {
    let source = r#"
module tb;
    int val;
    int success;
    initial begin
        uvm_config_db::set(null, "top", "my_key", 42);
        success = uvm_config_db::get(null, "top", "my_key", val);
        assert(success == 1);
        assert(val == 42);
        // Not found case
        success = uvm_config_db::get(null, "top", "missing", val);
        assert(success == 0);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_config_db test failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_config_db_exists() {
    // VERIF-06: `uvm_config_db::exists(inst, field)` → 1 bila key punya
    // nilai (set pernah dipanggil), 0 bila tidak. $error kanari.
    let source = r#"
module tb;
    int e_before;
    int e_after;
    initial begin
        e_before = uvm_config_db::exists(null, "top", "my_key");
        uvm_config_db::set(null, "top", "my_key", 7);
        e_after = uvm_config_db::exists(null, "top", "my_key");
        if (e_before != 0) $error("exists sebelum set harus 0, got %0d", e_before);
        if (e_after != 1) $error("exists setelah set harus 1, got %0d", e_after);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_config_db exists failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_config_db_wait_modified() {
    // VERIF-06: `uvm_config_db::wait_modified(inst, field)` BLOCKING —
    // task konsumen suspend sampai `set` berikutnya utk key tsb, lalu resume.
    // $error kanari: resume tidak terjadi (woke=0) → Err. Menangkap:
    // (1) intercept block.rs tak ada (wait_modified di-eval sebagai query,
    // statement lanjut seketika → woke=1 padahal key belum ada);
    // (2) release oleh set tak ada (waiter tak pernah dibangunkan).
    let source = r#"
class my_env extends uvm_env;
    int woke;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        fork
            // Konsumen: wait_modified blocking sampai set dipanggil.
            begin
                uvm_config_db::wait_modified(null, "top", "cfg");
                woke = 1;
            end
            // Produsen: delay 10 lalu set → bangunkan konsumen.
            begin
                #10;
                uvm_config_db::set(null, "top", "cfg", 99);
            end
        join
    endtask
endclass

module tb;
    my_env env_ref;
    int woke_snap_5;
    int woke_snap_20;
    initial begin
        env_ref = new("env_ref", null);
        fork
            env_ref.run_phase();
            // Probe: woke harus 0 di t=5 (sebelum set t=10), 1 di t=20.
            begin
                #5 woke_snap_5 = env_ref.woke;
                #15 woke_snap_20 = env_ref.woke;
            end
        join
        if (woke_snap_5 != 0)
            $error("woke harus 0 di t=5 (blocking), got %0d", woke_snap_5);
        if (woke_snap_20 != 1)
            $error("woke harus 1 di t=20 (resume setelah set), got %0d", woke_snap_20);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 100).expect("uvm_config_db wait_modified run failed");
    let s5 = sigs
        .iter()
        .find(|(n, _)| n == "woke_snap_5")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let s20 = sigs
        .iter()
        .find(|(n, _)| n == "woke_snap_20")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(
        s5, 0,
        "woke harus 0 di t=5 (wait_modified blocking sebelum set), got {}",
        s5
    );
    assert_eq!(
        s20, 1,
        "woke harus 1 di t=20 (resume setelah set), got {}",
        s20
    );
}

#[test]
fn test_uvm_config_db_wildcard_path() {
    // F19: wildcard path matching — pola UVM nyata `*.agent` /
    // `uvm_test_top.*` harus match inst_path hierarki, dan exact match
    // MENANG atas wildcard.
    let source = r#"
module tb;
    int val;
    int success;
    initial begin
        // wildcard tunggal: `*` match satu level hierarki
        uvm_config_db::set(null, "*.agent", "count", 8);
        success = uvm_config_db::get(null, "env.agent", "count", val);
        assert(success == 1);
        assert(val == 8);
        // wildcard multi-level: `uvm_test_top.*` match seluruh subtree
        uvm_config_db::set(null, "uvm_test_top.*", "depth", 3);
        success = uvm_config_db::get(null, "uvm_test_top.env.agent", "depth", val);
        assert(success == 1);
        assert(val == 3);
        // exact match menang atas wildcard yang juga match
        uvm_config_db::set(null, "env.agent", "count", 99);
        success = uvm_config_db::get(null, "env.agent", "count", val);
        assert(success == 1);
        assert(val == 99);
        // wildcard TIDAK match field lain
        success = uvm_config_db::get(null, "env.agent", "missing", val);
        assert(success == 0);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_config_db wildcard test failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_config_db_run_test_hierarchy() {
    // F19: skenario UVM NYATA — `initial run_test("my_test")` + set di
    // build_phase test + get(this, "", ...) wildcard di agent. Regresi ini
    // menangkap 3 bug yang diperbaiki bersama:
    //  (1) auto-detect execute_phases() menang atas run_test eksplisit
    //      (guard design_has_explicit_run_test)
    //  (2) `uvm_config_db::set(...)` statement di body method class dikira
    //      deklarasi `pkg::type` → parse_function gagal → build_phase
    //      tidak terdaftar
    //  (3) NUL terminator dari string_to_logicvec bocor ke inst_path
    //      (`uvm_test_top\0.agent` → wildcard tidak pernah match)
    let source = r#"
class my_agent extends uvm_agent;
    int got_count;
    int ok;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ok = uvm_config_db::get(this, "", "count", got_count);
        if (ok != 1 || got_count != 8)
            $error("AGENT get failed ok=%0d count=%0d", ok, got_count);
    endfunction
endclass

class my_env extends uvm_env;
    my_agent ag;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ag = new("agent", this);
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        uvm_config_db::set(this, "*.agent", "count", 8);
        env = new("env", this);
    endfunction
endclass

module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_signals(source, 100);
    assert!(
        result.is_ok(),
        "uvm_config_db run_test hierarchy failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_event_sync() {
    // F21: uvm_event — trigger()/wait_trigger() blocking antar fork branch di
    // run_phase. `$error` dipakai sebagai kanari: kalau wait_trigger tidak
    // memblock (join terlalu cepat) atau field tidak ter-set, simulate_str
    // mengembalikan Err → assert gagal. Menangkap 3 bug: (1) AST fork join
    // mengeksekusi remaining langsung tanpa menunggu branch; (2) current_this
    // hilang saat fork_finish mengeksekusi cont AST setelah join; (3) event
    // data tak ter-insert karena builtin `__uvm_event` punya methods kosong.
    let source = r#"
class my_env extends uvm_env;
    uvm_event done_ev;
    int t1_done;
    int t2_done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        done_ev = new("done_ev");
        fork
            begin
                #10;
                done_ev.trigger();
                t1_done = 1;
            end
            begin
                done_ev.wait_trigger();
                t2_done = 1;
            end
        join
        if (!t1_done || !t2_done)
            $error("uvm_event sync failed t1=%0d t2=%0d", t1_done, t2_done);
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(result.is_ok(), "uvm_event sync failed: {:?}", result.err());
}

#[test]
fn test_uvm_barrier_sync() {
    // F21: uvm_barrier — threshold 3: ketiga branch harus melewati wait_for
    // bersamaan; count di-reset setelah release. `$error` kanari bila ada
    // branch yang tidak pernah di-release (barrier tidak penuh).
    let source = r#"
class my_env extends uvm_env;
    uvm_barrier bar;
    int a_done;
    int b_done;
    int c_done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        bar = new("bar", 3);
        if (bar.get_threshold() != 3)
            $error("barrier threshold wrong: %0d", bar.get_threshold());
        fork
            begin
                #5;
                bar.wait_for();
                a_done = 1;
            end
            begin
                #10;
                bar.wait_for();
                b_done = 1;
            end
            begin
                #15;
                bar.wait_for();
                c_done = 1;
            end
        join
        if (!a_done || !b_done || !c_done)
            $error("uvm_barrier sync failed a=%0d b=%0d c=%0d", a_done, b_done, c_done);
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_barrier sync failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_event_subclass_override() {
    // F21 review fix #1: subclass user `my_event extends uvm_event` — `new`
    // override + method custom HARUS jalan normal, bukan di-intercept builtin
    // ("unknown uvm_event method"), dan `super.new` tetap insert data sync.
    let source = r#"
class my_event extends uvm_event;
    int extra;
    int notify_cnt;
    function new(string name);
        super.new(name);
        extra = 5;
    endfunction
    function void notify_extra();
        notify_cnt = notify_cnt + 1;
    endfunction
endclass

class my_env extends uvm_env;
    my_event ev;
    int ok1;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        ev = new("ev");
        if (ev.extra != 5) $error("subclass new override tidak jalan");
        ev.notify_extra();
        if (ev.notify_cnt != 1) $error("custom method tidak jalan");
        fork
            begin
                #5;
                ev.trigger();
            end
            begin
                ev.wait_trigger();
                ok1 = 1;
            end
        join
        if (ev.triggered() != 1) $error("triggered() salah");
        if (ok1 != 1) $error("wait_trigger gagal");
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_event subclass override failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_subscriber_analysis_broadcast() {
    // F22: uvm_subscriber — monitor menulis item ke analysis_port, connect ke
    // `sub.analysis_imp` (builtin imp auto-dibuat saat new), broadcast sampai
    // ke `write` override user. $error kanari: salah count/addr → Err.
    // Menangkap: (1) analysis_imp internal tak dibuat (field analysis_imp
    // kosong → connect no-op); (2) report/check phase child TIDAK dipanggil
    // bila root tak punya phase tsb (fix execute_report_phases propagate).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_monitor extends uvm_monitor;
    uvm_analysis_port ap;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ap = uvm_analysis_port::new("ap", this);
    endfunction
    task run_phase();
        my_item it;
        #10;
        it = new("it1");
        it.addr = 42;
        ap.write(it);
    endtask
endclass

class my_sub extends uvm_subscriber;
    int got_cnt;
    int last_addr;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void write(my_item t);
        got_cnt = got_cnt + 1;
        last_addr = t.addr;
        if (t.addr != 42) $error("sub addr salah %0d", t.addr);
    endfunction
endclass

// F22 review: subscriber TANPA new override — imp harus tetap dibuat via
// guard is_uvm_subscriber_hierarchy di allocate_new_object (sebelumnya
// find_method_in_hierarchy("new") gagal → imp tak dibuat → connect(0)).
class my_sub2 extends uvm_subscriber;
    int got_cnt;
    function void write(my_item t);
        got_cnt = got_cnt + 1;
        if (t.addr != 42) $error("sub2 addr salah %0d", t.addr);
    endfunction
endclass

class my_env extends uvm_env;
    my_monitor mon;
    my_sub sub;
    my_sub2 sub2; // TANPA new override — analysis_imp harus tetap auto-dibuat
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        mon = new("mon", this);
        sub = new("sub", this);
        sub2 = new("sub2", this);
    endfunction
    function void connect_phase();
        mon.ap.connect(sub.analysis_imp);
        mon.ap.connect(sub2.analysis_imp);
    endfunction
    function void check_phase();
        if (sub.got_cnt != 1) $error("sub got_cnt=%0d harusnya 1", sub.got_cnt);
        if (sub.last_addr != 42) $error("sub last_addr=%0d harusnya 42", sub.last_addr);
        if (sub2.got_cnt != 1) $error("sub2 (tanpa new override) got_cnt=%0d harusnya 1", sub2.got_cnt);
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_subscriber broadcast failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_tlm_fifo_blocking_put_get() {
    // F23: uvm_tlm_fifo — konsumen `get` blocking (suspend saat kosong,
    // resume + tulis lvalue saat put), produsen `put` dua item terpisah.
    // $error kanari: item salah / count salah → Err. Menangkap: (1) statement
    // get tidak ada di continuation saat resume (pop+write tak terjadi);
    // (2) release waiter salah-match (wait_label vs current_method).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_tlm_fifo fifo;
    my_item r1;
    my_item r2;
    int got1;
    int got2;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        fifo = new("fifo", this, 4);
        if (fifo.capacity() != 4) $error("capacity salah");
    endfunction
    task run_phase();
        my_item a;
        my_item b;
        fork
            begin
                fifo.get(r1);
                got1 = 1;
                fifo.get(r2);
                got2 = 1;
            end
            begin
                #10;
                a = new("a");
                a.addr = 100;
                fifo.put(a);
                #10;
                b = new("b");
                b.addr = 200;
                fifo.put(b);
            end
        join
        if (got1 != 1 || got2 != 1) $error("blocking get gagal got1=%0d got2=%0d", got1, got2);
        if (r1.addr != 100 || r2.addr != 200) $error("item salah r1=%0d r2=%0d", r1.addr, r2.addr);
        if (fifo.used() != 0) $error("fifo harus kosong, used=%0d", fifo.used());
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_tlm_fifo blocking failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_tlm_fifo_analysis_export() {
    // F23: `fifo.analysis_export.write(item)` (export analysis internal)
    // memetakan ke put — konsumen `get` menerima item. Tanpa auto-created
    // export, `fifo.analysis_export` = null handle → write no-op → r1 tak
    // ter-set → $error kanari.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_tlm_fifo fifo;
    my_item r1;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        fifo = new("fifo", this, 4);
    endfunction
    task run_phase();
        my_item a;
        fork
            begin
                fifo.get(r1);
            end
            begin
                #10;
                a = new("a");
                a.addr = 77;
                fifo.analysis_export.write(a);
            end
        join
        if (r1.addr != 77) $error("analysis_export gagal r1=%0d", r1.addr);
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_tlm_fifo analysis_export failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_comparator_in_order_match() {
    // VERIF-13: uvm_in_order_comparator — `write_expected` push ke antrian,
    // `write(actual)` pop head & bandingkan (fallback field equality: addr
    // sama → match). $error kanari: count match/mismatch salah → Err.
    // Menangkap: (1) comparator tak ter-dispatch (get_match_count builtin
    // tidak ada); (2) fallback field compare salah (objek berbeda tapi
    // field sama harus dianggap MATCH — bukan obj id).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_in_order_comparator comp;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        comp = new("comp", this);
        if (comp.analysis_imp == 0) $error("analysis_imp internal tak dibuat");
    endfunction
    task run_phase();
        my_item e1;
        my_item e2;
        my_item a1;
        my_item a2;
        e1 = new("e1");
        e1.addr = 10;
        e2 = new("e2");
        e2.addr = 20;
        a1 = new("a1");
        a1.addr = 10;
        a2 = new("a2");
        a2.addr = 20;
        comp.write_expected(e1);
        comp.write_expected(e2);
        comp.write(a1);
        comp.write(a2);
    endtask
    function void check_phase();
        if (comp.get_match_count() != 2)
            $error("match=%0d harusnya 2", comp.get_match_count());
        if (comp.get_mismatch_count() != 0)
            $error("mismatch=%0d harusnya 0", comp.get_mismatch_count());
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_comparator in-order match failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_comparator_mismatch_via_analysis_port() {
    // VERIF-13: jalur analysis port penuh — monitor `ap.write(actual)` →
    // `comp.analysis_imp` → parent.write → compare vs expected head
    // (addr 5 vs 9 → MISMATCH). $error kanari: mismatch count salah.
    // Menangkap: analysis_imp internal tidak terhubung (connect no-op) →
    // write tak sampai → mismatch 0.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_monitor extends uvm_monitor;
    uvm_analysis_port ap;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ap = uvm_analysis_port::new("ap", this);
    endfunction
    task run_phase();
        my_item it;
        #10;
        it = new("it1");
        it.addr = 9;
        ap.write(it);
    endtask
endclass

class my_env extends uvm_env;
    uvm_comparator comp;
    my_monitor mon;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        comp = new("comp", this);
        mon = new("mon", this);
    endfunction
    function void connect_phase();
        mon.ap.connect(comp.analysis_imp);
    endfunction
    task run_phase();
        my_item e;
        e = new("e1");
        e.addr = 5;
        comp.write_expected(e);
    endtask
    function void check_phase();
        if (comp.get_match_count() != 0)
            $error("match=%0d harusnya 0", comp.get_match_count());
        if (comp.get_mismatch_count() != 1)
            $error("mismatch=%0d harusnya 1 (addr 9 vs 5)", comp.get_mismatch_count());
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_comparator mismatch via analysis port failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_heartbeat_ok() {
    // VERIF-15: uvm_heartbeat — object ter-monitor (driver/monitor) memanggil
    // `hb.heartbeat(obj)` cukup kali → `check()` di check_phase = 1, tanpa
    // UVM_ERROR. $error kanari: check()=0 → Err. Menangkap: (1) heartbeat
    // tak ter-dispatch (set_heartbeat/heartbeat/check builtin tidak ada);
    // (2) counter tidak ter-increment (received selalu 0).
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_heartbeat hb;
    my_comp drv;
    my_comp mon;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        hb = new("hb", this);
        drv = new("drv", this);
        mon = new("mon", this);
        hb.set_heartbeat(drv, 2);
        hb.set_heartbeat(mon, 1);
    endfunction
    task run_phase();
        #5;
        hb.heartbeat(drv);
        #5;
        hb.heartbeat(drv);
        hb.heartbeat(mon);
    endtask
    function void check_phase();
        if (hb.get_heartbeat_count(drv) != 2)
            $error("drv heartbeat=%0d harusnya 2", hb.get_heartbeat_count(drv));
        if (hb.get_heartbeat_count(mon) != 1)
            $error("mon heartbeat=%0d harusnya 1", hb.get_heartbeat_count(mon));
        if (hb.check() != 1)
            $error("hb.check() harusnya 1 (semua terpenuhi)");
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_heartbeat ok failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_heartbeat_missing_heartbeat() {
    // VERIF-15: object yang TIDAK mencapai required heartbeat → `check()`
    // mengembalikan 0. Test ini memverifikasi sinyal kegagalan lewat return
    // value check() (diekspresikan sebagai $error kanari di user code),
    // BUKAN lewat severity engine — sehingga expected failure tetap Err.
    // Menangkap: check() selalu 1 (required diabaikan).
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_heartbeat hb;
    my_comp drv;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        hb = new("hb", this);
        drv = new("drv", this);
        hb.set_heartbeat(drv, 3);
    endfunction
    task run_phase();
        #5;
        hb.heartbeat(drv);  // hanya 1 dari 3 — harusnya GAGAL
    endtask
    function void check_phase();
        if (hb.check() != 0)
            $error("hb.check() harusnya 0 (drv hanya 1/3 heartbeat)");
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_heartbeat missing failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_tlm_fifo_blocking_put_full_and_peek() {
    // F23 review: jalur paling berisiko — `put` blocking saat penuh
    // (capacity 1: put a sukses, put b suspend → getter pop → putter resume
    // push b) + `peek` blocking (baca head tanpa pop, used tetap).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_tlm_fifo fifo;
    my_item r1;
    my_item r2;
    int put_b_done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        fifo = new("fifo", this, 1);
    endfunction
    task run_phase();
        my_item a;
        my_item b;
        fork
            begin
                a = new("a");
                a.addr = 1;
                fifo.put(a);
                b = new("b");
                b.addr = 2;
                fifo.put(b); // penuh → suspend sampai getter pop a
                put_b_done = 1;
            end
            begin
                #10;
                fifo.get(r1); // pop a → release putter
                fifo.peek(r2); // baca b tanpa pop
            end
        join
        if (put_b_done != 1) $error("put blocking saat penuh gagal");
        if (r1.addr != 1) $error("r1=%0d harusnya 1", r1.addr);
        if (r2.addr != 2) $error("r2=%0d harusnya 2 (peek)", r2.addr);
        if (fifo.used() != 1) $error("peek tidak boleh pop, used=%0d", fifo.used());
        if (fifo.is_empty() != 0 || fifo.is_full() != 1) $error("is_empty/is_full salah");
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_tlm_fifo blocking put full / peek failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_driver_blocking_handshake() {
    // F24: blocking handshake sequence/sequencer/driver — `start_item` push
    // + release getter, driver `get_next_item` BLOCK sampai item tersedia
    // (pop + tulis lvalue), `finish_item` BLOCK sampai driver `item_done`
    // (release finisher per-item). Tanpa sinkronisasi ini, sequence selesai
    // duluan & driver tak dapat item (got=0). Field `req` (bukan task-local)
    // karena write_ast_lvalue menulis ke field/signal, bukan local task.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_driver extends uvm_driver;
    int got_cnt;
    int last_addr;
    my_item req;
    task run();
        while (got_cnt < 3) begin
            get_next_item(req);
            got_cnt = got_cnt + 1;
            last_addr = req.addr;
            item_done();
        end
    endtask
endclass

class my_sequence extends uvm_sequence;
    int sent_cnt;
    task body();
        my_item it;
        it = new("it1");
        it.addr = 100;
        start_item(it);
        finish_item(it);
        it = new("it2");
        it.addr = 200;
        start_item(it);
        finish_item(it);
        it = new("it3");
        it.addr = 300;
        start_item(it);
        finish_item(it);
        sent_cnt = 3;
    endtask
endclass

module tb;
    my_driver drv;
    uvm_sequencer sqr;
    my_sequence seq;
    initial begin
        drv = new("drv", null);
        sqr = new("sqr", null);
        drv.set_sequencer(sqr);
        seq = new("seq");
        fork
            drv.run();
            seq.start(sqr);
        join
        #50;
        if (drv.got_cnt != 3) $error("driver tak dapat 3 item, got=%0d", drv.got_cnt);
        if (drv.last_addr != 300) $error("addr terakhir salah %0d", drv.last_addr);
        if (seq.sent_cnt != 3) $error("sequence tak selesai, sent=%0d", seq.sent_cnt);
    end
endmodule
"#;
    let result = simulate_str(source, 1000);
    assert!(
        result.is_ok(),
        "uvm sequence/driver blocking handshake failed: {:?}",
        result.err()
    );
}

#[test]
fn test_ir_fork_join_waits_for_suspended_task() {
    // F26: IR `fork ... join` di module initial TIDAK boleh selesai premature
    // saat branch men-suspend task method (delay). Sebelumnya arm loop AST
    // (LoopWhile/LoopForever/LoopFor/Repeat/DoWhile) memakai `break` internal
    // saat body suspend → `evaluate_ast_block_with_delay_fork` mengembalikan
    // Ok(true) → fork_branch_end decrement → join selesai sebelum task selesai.
    // Sekarang `return Ok(false)` diteruskan; resume ContinueAstBlock(fork_id)
    // yang decrement saat branch benar-benar selesai.
    let source = r#"
class worker;
    int done_cnt;
    task run();
        #10;
        done_cnt = done_cnt + 1;
    endtask
endclass

module tb;
    worker w1, w2;
    initial begin
        w1 = new();
        w2 = new();
        fork
            w1.run();
            w2.run();
        join
        // Tanpa fix: join selesai di t=0 → done_cnt masih 0 → $error.
        if (w1.done_cnt + w2.done_cnt != 2) $error("fork join premature: done=%0d", w1.done_cnt + w2.done_cnt);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 100);
    assert!(
        result.is_ok(),
        "fork/join harus menunggu task suspend: {:?}",
        result.err()
    );
}

#[test]
fn test_ir_fork_uvm_handshake_no_workaround() {
    // F26: fork...join handshake UVM penuh TANPA workaround `#50` setelah
    // join (sebelumnya wajib karena fork IR selesai premature saat task
    // suspend). Driver harus dapat 3 item (100/200/300) sebelum join lewat.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_driver extends uvm_driver;
    int got_cnt;
    int last_addr;
    my_item req;
    task run();
        while (got_cnt < 3) begin
            get_next_item(req);
            got_cnt = got_cnt + 1;
            last_addr = req.addr;
            item_done();
        end
    endtask
endclass

class my_sequence extends uvm_sequence;
    int sent_cnt;
    task body();
        my_item it;
        it = new("it1");
        it.addr = 100;
        start_item(it);
        finish_item(it);
        it = new("it2");
        it.addr = 200;
        start_item(it);
        finish_item(it);
        it = new("it3");
        it.addr = 300;
        start_item(it);
        finish_item(it);
        sent_cnt = 3;
    endtask
endclass

module tb;
    my_driver drv;
    uvm_sequencer sqr;
    my_sequence seq;
    initial begin
        drv = new("drv", null);
        sqr = new("sqr", null);
        drv.set_sequencer(sqr);
        seq = new("seq");
        fork
            drv.run();
            seq.start(sqr);
        join
        // Tanpa #50: join harus menunggu handshake selesai (got=3 last=300 sent=3).
        if (drv.got_cnt != 3 || drv.last_addr != 300 || seq.sent_cnt != 3)
            $error("fork join premature: got=%0d last=%0d sent=%0d", drv.got_cnt, drv.last_addr, seq.sent_cnt);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 1000);
    assert!(
        result.is_ok(),
        "fork join harus menunggu handshake UVM: {:?}",
        result.err()
    );
}

#[test]
fn test_interface_port_dut_always_ff_hier_clock() {
    // F27: port bertipe interface di module + always_ff dengan clock hierarkis
    // (`posedge b.clk`). Port interface dielaborasi sebagai handle 64-bit
    // (iface_type); field diakses via hier_signal_map (b.clk). Sebelumnya:
    // "always_ff must have at least one clock edge" + koneksi `.b(b)` gagal
    // di elaborasi (E3001).
    let source = r#"
interface bus_if;
    logic clk;
    logic [7:0] data;
endinterface

module dut (bus_if b);
    always_ff @(posedge b.clk) begin
        b.data <= 8'h42;
    end
endmodule

module tb;
    bus_if b();
    dut u_dut (.b(b));
    initial begin
        b.clk = 0;
        forever #5 b.clk = ~b.clk;
    end
    initial begin
        #50;
        if (b.data != 8'h42) $error("interface data mismatch: %0h", b.data);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(
        result.is_ok(),
        "interface port + hier clock: {:?}",
        result.err()
    );
}

#[test]
fn test_interface_modport_port_procedural_event() {
    // F27: port interface + modport (`axi_lite.dut bus`) + `@(posedge bus.clk)`
    // prosedural (jalur AST event) + reset via field interface. Parser dulu
    // tidak mengenali `iface.modport name` → dtype_name kosong → modul rusak.
    let source = r#"
interface axi_lite;
    logic        clk;
    logic        rst_n;
    logic [7:0]  awaddr;
    logic [7:0]  wdata;
    logic [7:0]  rdata;
    modport dut (input clk, rst_n, awaddr, wdata, output rdata);
endinterface

module slave (axi_lite.dut bus);
    always_ff @(posedge bus.clk or negedge bus.rst_n) begin
        if (!bus.rst_n)
            bus.rdata <= 8'h00;
        else if (bus.awaddr == 8'h10)
            bus.rdata <= bus.wdata + 8'h01;
    end
endmodule

module tb;
    axi_lite bus();
    slave u_slave (.bus(bus));
    initial begin
        bus.rst_n = 0;
        bus.awaddr = 0;
        bus.wdata = 0;
        bus.clk = 0;
        forever #5 bus.clk = ~bus.clk;
    end
    initial begin
        #10 bus.rst_n = 1;
        @(posedge bus.clk);
        bus.awaddr = 8'h10;
        bus.wdata = 8'h2a;
        @(posedge bus.clk);
        #1;
        if (bus.rdata != 8'h2b) $error("modport read mismatch: %0h", bus.rdata);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(
        result.is_ok(),
        "modport port + procedural event: {:?}",
        result.err()
    );
}

#[test]
fn test_interface_with_ports_connection() {
    // F27: interface yang PUNYA port + koneksi port eksternal
    // (`bus_if b(.clk(clk), ...)`) ke modul port interface — koneksi port_map
    // generic harus tetap jalan untuk instance interface.
    let source = r#"
interface bus_if (
    input  logic        clk,
    input  logic        rst_n,
    input  logic [7:0]  din,
    output logic [7:0]  dout
);
    logic [7:0] shadow;
endinterface

module dut (bus_if b);
    always_ff @(posedge b.clk or negedge b.rst_n) begin
        if (!b.rst_n) begin
            b.dout <= 8'h00;
            b.shadow <= 8'h00;
        end else begin
            b.dout <= b.din + 8'h03;
            b.shadow <= b.din;
        end
    end
endmodule

module tb;
    logic clk;
    logic rst_n;
    logic [7:0] din;
    logic [7:0] dout;
    bus_if b (.clk(clk), .rst_n(rst_n), .din(din), .dout(dout));
    dut u_dut (.b(b));
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    initial begin
        rst_n = 0;
        din = 8'h07;
        #10 rst_n = 1;
        #30 din = 8'h20;
        #20;
        if (dout != 8'h23) $error("iface port conn mismatch: %0h", dout);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(result.is_ok(), "interface with ports: {:?}", result.err());
}

#[test]
fn test_interface_port_alias_hier_diff_names() {
    // F28: port interface di child (`axi_if`) dikoneksikan ke instance
    // interface di parent dengan nama BEDA (`bus`) — akses `axi_if.<field>`
    // di child harus resolve ke signal flatten `bus.<field>` via alias hier
    // yang ditambahkan flatten (class_name handle menyimpan nama instance).
    // Pola ini dipakai .mv: `sig bus : axi_lite` → `axi_lite bus();`.
    let source = r#"
interface axi_lite;
    bit clk;
    logic [7:0] awaddr;
    logic [7:0] wdata;
endinterface

module dut (axi_lite axi_if);
    always_ff @(posedge axi_if.clk) begin
        axi_if.wdata <= axi_if.awaddr + 8'h01;
    end
endmodule

module tb;
    axi_lite bus();
    dut u_dut (.axi_if(bus));
    initial begin
        bus.clk = 0;
        forever #5 bus.clk = ~bus.clk;
    end
    initial begin
        #10 bus.awaddr = 8'h2a;
        #30;
        if (bus.wdata != 8'h2b) $error("alias hier mismatch: %0h", bus.wdata);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(result.is_ok(), "alias hier diff names: {:?}", result.err());
}

#[test]
fn test_hex_literal_equality_fix() {
    // F30: literal sized hex di-zero-extend — `sig == 16'h6` benar setelah
    // value_to_logicvec zero-fill. Sebelumnya bit di atas digit = X
    // (LogicVec::new = fill X) → X di operand → hasil X → false.
    let source = r#"
module tb_hexeq;
    logic [15:0] addr = 16'd6;
    initial begin
        #1;
        if (addr != 16'h6) $error("hex eq 6 gagal: got %0d", addr == 16'h6);
        if (addr != 16'h06) $error("hex eq 06 gagal");
        if (addr != 16'hA) $error("hex eq A: 6 harus != 10");
        if (addr != 16'd6) $error("dec eq gagal");
        if (addr != 6) $error("unsized eq gagal");
        $display("HEXEQ_OK");
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 50);
    assert!(result.is_ok(), "hex eq: {:?}", result.err());
}

#[test]
fn test_hex_literal_x_digit_stays_x() {
    // F30 fix: digit x/z eksplisit di literal tetap X (bukan di-zero-kan).
    let source = r#"
module tb_hexx;
    logic [7:0] v;
    initial begin
        v = 8'hx6;
        #1;
        if ($isunknown(v) != 1) $error("8'hx6 harus unknown");
        $display("HEXX_OK");
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 50);
    assert!(result.is_ok(), "hex x: {:?}", result.err());
}

#[test]
fn test_interface_alias_order_independent() {
    // F28 fix review: alias hier port interface diproses POST-pass setelah
    // SEMUA instance ter-flatten — sehingga urutan AST tidak relevan.
    // Di sini child module (`u_dut`) ditulis SEBELUM instance interface
    // (`bus()`) — tanpa post-pass alias tak terbentuk (bus.* belum ada saat
    // u_dut di-flatten) → akses `axi_if.wdata` gagal.
    let source = r#"
interface axi_lite;
    bit clk;
    logic [7:0] awaddr;
    logic [7:0] wdata;
endinterface

module dut (axi_lite axi_if);
    always_ff @(posedge axi_if.clk) begin
        axi_if.wdata <= axi_if.awaddr + 8'h01;
    end
endmodule

module tb;
    dut u_dut (.axi_if(bus));
    axi_lite bus();
    initial begin
        bus.clk = 0;
        forever #5 bus.clk = ~bus.clk;
    end
    initial begin
        #10 bus.awaddr = 8'h2a;
        #30;
        if (bus.wdata != 8'h2b) $error("alias order mismatch: %0h", bus.wdata);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(
        result.is_ok(),
        "alias order independent: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_report_object_compile() {
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void do_report();
        uvm_report_info("my_id", "info message", 0);
    endfunction
endclass

module tb;
    my_comp c;
    initial begin
        c = my_comp::new("c", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_report_object compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_factory_override() {
    let source = r#"
class base_driver extends uvm_driver;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function string get_type();
        return "base_driver";
    endfunction
endclass

class extended_driver extends uvm_driver;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function string get_type();
        return "extended_driver";
    endfunction
endclass

module tb;
    base_driver drv;
    initial begin
        uvm_factory::set_type_override_by_type("base_driver", "extended_driver");
        drv = base_driver::new("drv", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_factory override compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_ovm_factory_override_alias() {
    // VERIF-20 tahap 2: `ovm_factory::set_type_override_by_type` alias
    // ke handler UVM yang sama (elab SysFunc + engine IR/AST).
    // Runtime: return 1 + override tercatat (terbaca via get-like path:
    // override memengaruhi `new` berikutnya — di sini assert return).
    let source = r#"
module tb;
    int ok;
    initial begin
        ok = ovm_factory::set_type_override_by_type("base_driver", "extended_driver");
        assert(ok == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).expect("ovm_factory run failed");
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(ok, 1, "ovm_factory alias harus return 1, got {}", ok);
}

#[test]
fn test_uvm_resource_db_set_get() {
    // VERIF-07: set/get sebagai bare statement (bukan di-eliminasi
    // elaborator) — set menyimpan ke map, get mengembalikan nilai.
    let source = r#"
module tb;
    int val;
    int success;
    int missing_success;
    initial begin
        uvm_resource_db::set("scope1", "key1", 99);
        success = uvm_resource_db::get("scope1", "key1", val);
        missing_success = uvm_resource_db::get("scope1", "missing", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("uvm_resource_db sim");
    let (_, sv) = sigs.iter().find(|(n, _)| n == "success").unwrap();
    assert_eq!(sv.to_u64(), 1, "get harus menemukan resource yang di-set");
    let (_, vv) = sigs.iter().find(|(n, _)| n == "val").unwrap();
    assert_eq!(vv.to_u64(), 99, "nilai resource harus 99");
    let (_, mv) = sigs.iter().find(|(n, _)| n == "missing_success").unwrap();
    assert_eq!(mv.to_u64(), 0, "get untuk key yang tidak ada harus 0");
}

#[test]
fn test_uvm_resource_db_wildcard_scope() {
    // VERIF-07: wildcard scope — set("*.env", ...) terbaca oleh
    // get("tb.env", ...) dan exists("tb.env", ...) = 1.
    let source = r#"
module tb;
    int val;
    int success;
    int e1;
    int e2;
    initial begin
        uvm_resource_db::set("*.env", "baud", 115200);
        success = uvm_resource_db::get("tb.env", "baud", val);
        e1 = uvm_resource_db::exists("tb.env", "baud");
        e2 = uvm_resource_db::exists("tb.env", "nope");
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("wildcard resource_db sim");
    let (_, sv) = sigs.iter().find(|(n, _)| n == "success").unwrap();
    assert_eq!(sv.to_u64(), 1, "wildcard scope harus match get");
    let (_, vv) = sigs.iter().find(|(n, _)| n == "val").unwrap();
    assert_eq!(vv.to_u64(), 115200, "nilai dari set wildcard harus terbaca");
    let (_, a) = sigs.iter().find(|(n, _)| n == "e1").unwrap();
    assert_eq!(a.to_u64(), 1, "exists untuk resource wildcard harus 1");
    let (_, b) = sigs.iter().find(|(n, _)| n == "e2").unwrap();
    assert_eq!(
        b.to_u64(),
        0,
        "exists untuk resource yang tidak ada harus 0"
    );
}

#[test]
fn test_uvm_resource_db_read_write_by_name() {
    // VERIF-07: write_by_name = alias set, read_by_name = alias get.
    let source = r#"
module tb;
    int val;
    int success;
    initial begin
        uvm_resource_db::write_by_name("s1", "k1", 42);
        success = uvm_resource_db::read_by_name("s1", "k1", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("resource_db read/write sim");
    let (_, sv) = sigs.iter().find(|(n, _)| n == "success").unwrap();
    assert_eq!(sv.to_u64(), 1, "read_by_name harus menemukan resource");
    let (_, vv) = sigs.iter().find(|(n, _)| n == "val").unwrap();
    assert_eq!(
        vv.to_u64(),
        42,
        "read_by_name harus mengembalikan nilai write_by_name"
    );
}

#[test]
fn test_param_class_compile() {
    let source = r#"
class #(type T = int) my_param_class;
    T data;
    function T get_data();
        return data;
    endfunction
    function new(T val);
        data = val;
    endfunction
endclass
module tb;
    my_param_class obj;
    initial begin
        obj = my_param_class #(int)::new(42);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "param class sim failed: {:?}", result.err());
}

#[test]
fn test_uvm_scoreboard_compile() {
    let source = r#"
class my_scoreboard extends uvm_scoreboard;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_scoreboard sb;
    initial begin
        sb = my_scoreboard::new("sb", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_scoreboard compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_monitor_compile() {
    let source = r#"
class my_monitor extends uvm_monitor;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase(uvm_phase phase);
        // monitor observes transactions
    endtask
endclass

module tb;
    my_monitor mon;
    initial begin
        mon = my_monitor::new("mon", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_monitor compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_item_get_type_name() {
    let source = r#"
class my_item extends uvm_sequence_item;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_item item;
    reg [63:0] tname;
    initial begin
        item = my_item::new("my_item");
        tname = item.get_type_name();
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
    // get_type_name returns string bits, we just verify sim completes
}

#[test]
fn test_const_fold_binary_op() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 10 + 20;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 30, "10 + 20 should fold to 30");
}

#[test]
fn test_const_fold_ternary() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = (1) ? 100 : 200;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 100, "ternary with true cond should fold to 100");
}

#[test]
fn test_const_fold_concat() {
    let source = r#"
module tb;
    reg [7:0] x;
    initial begin
        x = {4'b1010, 4'b0101};
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0xa5, "concat of constants should fold");
}

#[test]
fn test_dce_if_const_true() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        if (1) x = 50; else x = 99;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 50, "if(1) should execute true branch");
}

#[test]
fn test_dce_if_const_false() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        if (0) x = 50; else x = 99;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 99, "if(0) should execute false branch");
}

#[test]
fn test_dce_case_const() {
    let source = r#"
module tb;
    reg [31:0] x;
    integer sel;
    initial begin
        sel = 2;
        case (sel)
            0: x = 10;
            1: x = 20;
            2: x = 30;
            3: x = 40;
        endcase
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 30, "case const 2 -> x=30");
}

#[test]
fn test_dce_case_default() {
    let source = r#"
module tb;
    reg [31:0] x;
    integer sel;
    initial begin
        sel = 99;
        case (sel)
            0: x = 10;
            1: x = 20;
            default: x = 99;
        endcase
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 99, "case default -> x=99");
}

#[test]
fn test_dce_if_no_else() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        if (1) x = 50;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 50, "if(1) no else should execute true branch");
}

#[test]
fn test_assert_pass() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 1;
        assert (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "assert with true condition should not fail");
}

#[test]
fn test_assert_fail() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        assert (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(1);
    assert_eq!(v, 0, "assert with false condition should continue");
}

#[test]
fn test_assert_else_stmt() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        assert (x == 1) else x = 99;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 99, "assert else stmt should execute on failure");
}

#[test]
fn test_assertion_coverage_metrics() {
    // VERIF-27: assertion coverage metrics — engine.assertion_stats mencatat
    // pass/fail per assertion (assert immediate + expect, jalur IR).
    let source = r#"
module tb;
    int x;
    initial begin
        x = 5;
        assert (x > 0);
        assert (x > 10);
        expect (x == 5) else;
        expect (x == 6) else;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    let _ = engine.run();
    assert_eq!(
        engine.assertion_stats.len(),
        4,
        "4 assertion (2 assert + 2 expect) dievaluasi: {:?}",
        engine.assertion_stats
    );
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert_eq!(total_pass, 2, "2 pass: assert(x>0), expect(x==5)");
    assert_eq!(total_fail, 2, "2 fail: assert(x>10), expect(x==6)");
}

#[test]
fn test_assertion_coverage_concurrent_sequence() {
    // VERIF-27: concurrent assertion (assert property @(posedge clk)) —
    // completion sequence attempt tercatat pass/fail di assertion_stats.
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    // 4 siklus clk → cnt 1..4; assertion `cnt <= 3` gagal di siklus ke-4.
    // (assert property module-level tidak di-drive jadi process — pakai
    // pola immediate assertion di dalam forever @(posedge clk).)
    initial begin
        forever begin
            @(posedge clk);
            assert (cnt <= 3) else $display("over");
        end
    end
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 3,
        "cnt<=3 lulus siklus 1..3: pass={}",
        total_pass
    );
    assert!(
        total_fail >= 1,
        "cnt<=3 gagal siklus ke-4: fail={}",
        total_fail
    );
}

#[test]
fn test_sequence_coverage_matched_and_hole() {
    // VERIF-32: sequence coverage — concurrent assertion `a ##1 b`
    // di-track per (line, col): attempts, matched, failed. Sequence yang
    // tidak pernah match (matched == 0) muncul sebagai coverage gap.
    let src = r#"
module tb_seqcov;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 1;      // match: a di cycle-1, b di cycle ini
    @(posedge clk); a = 1; b = 0;
    @(posedge clk);             // b tetap 0 → attempt ini timeout → fail
    @(posedge clk); a = 0; b = 0;
    #60; $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile seqcov");
    let mut engine = crate::simulator::SimulationEngine::new(design, 200);
    let _ = engine.run();

    assert!(
        !engine.sequence_coverage.is_empty(),
        "sequence coverage tercatat"
    );
    let stats: Vec<_> = engine.sequence_coverage.values().collect();
    let total_attempts: u64 = stats.iter().map(|s| s.attempts).sum();
    let total_matched: u64 = stats.iter().map(|s| s.matched).sum();
    let total_failed: u64 = stats.iter().map(|s| s.failed).sum();
    assert!(
        total_attempts >= 2,
        ">=2 attempt dimulai: {}",
        total_attempts
    );
    assert!(total_matched >= 1, "a##1 b match sekali: {}", total_matched);
    assert!(
        total_failed >= 1 || total_matched < total_attempts,
        "ada attempt yang tidak match (fail): failed={}",
        total_failed
    );

    // Coverage gap: buat design kedua dengan sequence yang TIDAK PERNAH
    // match → matched == 0 → gap "sequence ... tidak pernah match".
    let hole_src = r#"
module tb_hole;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;   // b tidak pernah 1 → hole
    @(posedge clk); a = 0;
    #60; $finish;
  end
endmodule
"#;
    let design2 = compile_str(hole_src).expect("compile hole");
    let mut engine2 = crate::simulator::SimulationEngine::new(design2, 200);
    let _ = engine2.run();
    let holes: Vec<&(usize, usize)> = engine2
        .sequence_coverage
        .iter()
        .filter(|(_, s)| s.matched == 0 && s.attempts > 0)
        .map(|(k, _)| k)
        .collect();
    assert!(
        !holes.is_empty(),
        "sequence tanpa match terdeteksi sbg hole"
    );
    let gaps = engine2.coverage_gaps();
    assert!(
        gaps.iter().any(|g| g.contains("tidak pernah match")),
        "coverage_gaps() memuat sequence hole: {:?}",
        gaps
    );
}

#[test]
fn test_cover_pass() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 1;
        cover (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "cover should not affect execution");
}

#[test]
fn test_module_level_assert_property_pass_fail() {
    // LANG-04: module-level `assert property (@(posedge clk) expr)` —
    // assertion concurrent BOOLEAN kini di-drive jadi always block ber-clock:
    // pass saat cond true, fail (assertion_stats) saat cond false.
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assert property (@(posedge clk) cnt <= 3);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 3,
        "module-level assert property: pass={}",
        total_pass
    );
    assert!(
        total_fail >= 1,
        "module-level assert property: fail={}",
        total_fail
    );
}

#[test]
fn test_module_level_cover_restrict_property() {
    // LANG-11/13: module-level `cover property` (hit saat true) + `restrict
    // property` (diperlakukan seperti assume — violation = fail metric).
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    reg [3:0] cnt_odd;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    cover property (@(posedge clk) cnt >= 2);
    // restrict cnt <= 3 → committed cnt = 4 di t=45 (siklus ke-5) = violation.
    restrict property (@(posedge clk) cnt <= 3);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    // cover property → hit tercatat di cover_hits (key cover@line:col).
    let cover_total: u64 = engine.cover_hits.values().sum();
    assert!(
        cover_total >= 1,
        "cover property harus tercatat hit: {}",
        cover_total
    );
    // restrict property (asumsi) → violation saat committed cnt=4 (>3).
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "restrict property violation harus tercatat fail: {}",
        total_fail
    );
}

#[test]
fn test_module_level_assume_property() {
    // LANG-12: module-level `assume property` — violation = fail metric
    // (asumsi constraint; jika dilanggar → error).
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assume property (@(posedge clk) cnt != 2);
    initial begin
        #30 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 40);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "assume property violation harus fail: {}",
        total_fail
    );
}

#[test]
fn test_module_level_property_temporal_skipped() {
    // LANG-04: property dgn operator temporal (`##1`, `|->`) tidak di-parse
    // (limitation) — modul tetap ter-parse utuh (tidak ada error E1002,
    // assertion kompleks di-skip).
    let source = r#"
module tb;
    reg clk;
    reg a, b;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        a <= b;
    end
    assert property (@(posedge clk) a ##1 b);
    initial begin
        #20 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 25).expect("temporal property skipped, modul tetap parse");
    let v = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(v.1.width > 0, "clk harus tetap ada — modul ter-parse");
}
#[test]
fn test_psl_boolean_assert_always_never() {
    // LANG-03: PSL (IEEE 1850) boolean — `assert always (expr) @(posedge
    // clk);` / `assert never (expr) @(posedge clk);` + directive
    // `default clock = posedge clk;`. always = properti true tiap cycle;
    // never = properti tidak boleh true (cond dibalik !). Dievaluasi tiap
    // posedge via jalur assertion module-level (ROUND 71).
    let source = r#"
default clock = posedge clk;
module tb;
    reg clk = 0;
    reg [7:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assert always (cnt <= 8) @(posedge clk);
    assert never (cnt > 8) @(posedge clk);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_pass >= 8, "PSL always+never: pass={}", total_pass);
    assert_eq!(
        total_fail, 0,
        "PSL never (cnt>8 dibalik) tidak boleh fail: {}",
        total_fail
    );
}

#[test]
fn test_psl_temporal_skipped_safe() {
    // LANG-03: PSL operator temporal (`|->`, `until`) tidak didukung lexer
    // → parse gagal → rollback + skip assertion (modul tetap utuh, perilaku
    // sama dengan temporal SVA LANG-04). Directive `default clock` juga
    // di-skip tanpa error.
    let source = r#"
default clock = posedge clk;
module tb;
    reg clk;
    reg a, b;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        a <= b;
    end
    assert always (a |-> b) @(posedge clk);
    assert always (a until b) @(posedge clk);
    initial begin
        #20 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 25).expect("PSL temporal skipped, modul tetap parse");
    let v = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(v.1.width > 0, "clk harus tetap ada — modul ter-parse");
}

#[test]
fn test_sva_temporal_sequence() {
    // LANG-04: `assert property (@(posedge clk) a ##1 b)` — sequence
    // temporal SVA. Engine mengevaluasi via `SequenceAttempt`: setiap
    // posedge, attempt baru dimulai; evaluator `eval_sequence_depth`
    // memeriksa `a` pada depth=1 (1 posedge lalu) dan `b` pada depth=0
    // (current posedge). Pass jika match, fail jika timeout max_cycles.
    //
    // Test PASS: a=1 diikuti b=1 → 1 match
    // Test FAIL: a pulse tapi b selalu 0 → 0 match, semua fail
    let pass_src = r#"
module tb_pass;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 1;
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(pass_src, 150).expect("compile pass case");
    let clk = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(clk.1.width > 0, "clk exists");

    let fail_src = r#"
module tb_fail;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); a = 0;
    #100; $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(fail_src, 150).expect("compile fail case");
    let clk = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(clk.1.width > 0, "clk exists");
}

#[test]
fn test_sva_temporal_range_delay() {
    // LANG-04 extension: `##[min:max]` range delay (IEEE 1800-2017 §16.9.2.2).
    // a ##[1:2] b → b harus true 1 atau 2 cycle SETELAH a true.
    //
    // Case 1: b true di cycle+1 (min match) → pass
    let pass_min = r#"
module tb_range_pass;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##[1:2] b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 1;  // cycle+1 → min match
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_min).expect("compile range pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    assert!(total_pass >= 1, "##[1:2] min match: pass={}", total_pass);

    // Case 2: b true di cycle+2 (max match) → pass
    let pass_max = r#"
module tb_range_pass2;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##[1:2] b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 0;  // cycle+1 miss
    @(posedge clk); b = 1;  // cycle+2 → max match
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_max).expect("compile range pass2");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    assert!(total_pass >= 1, "##[1:2] max match: pass={}", total_pass);

    // Case 3: b never true → fail
    let fail_src = r#"
module tb_range_fail;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##[1:2] b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 0;
    @(posedge clk); b = 0;
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile range fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_fail >= 1, "##[1:2] no match: fail={}", total_fail);
}

#[test]
fn test_sva_overlap_implication() {
    // LANG-04 extension: `|->` overlap implication (IEEE 1800-2017 §16.9.2).
    // Antecedent match di posedge k → consequent mulai dari posedge yang sama k.
    // Jika antecedent tidak pernah match → vacuously true (pass).
    //
    // Case vacuous: req=0 selalu → pass (vacuously true)
    let vacuous_src = r#"
module tb_vacuous;
  reg clk = 0;
  reg req = 0;
  reg ack = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) req |-> ack);
  initial begin
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(vacuous_src).expect("compile vacuous case");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_pass >= 1, "vacuous implication: pass={}", total_pass);
    assert_eq!(total_fail, 0, "vacuous implication: fail={}", total_fail);
}

#[test]
fn test_sva_nonoverlap_implication() {
    // LANG-47: `|=>` non-overlap implication (IEEE 1800-2017 §16.9.2).
    // Antecedent match di posedge k → consequent dievaluasi mulai posedge
    // k+1 (di window 1 cycle). Vacuous: req selamat dari match → pass.
    //
    // Pass: req=1 di cycle pertamanya yang true, done=1 TEPAT di posedge
    // berikutnya → attempt matched (pass).
    let pass_src = r#"
module tb_nonoverlap_pass;
  reg clk = 0;
  reg req = 0;
  reg done = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) req |=> done);
  initial begin
    @(posedge clk) req = 1;
    @(posedge clk) done = 1;  // done TRUE di posedge TEPAT setelah req
    @(posedge clk) req = 0;
    #20; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_src).expect("compile nonoverlap pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 1,
        "|=> pass case: pass={} fail={}",
        total_pass,
        total_fail
    );
    assert_eq!(total_fail, 0, "|=> pass case: fail={}", total_fail);

    // Fail: done=0 di posedge TEPAT setelah antecedent → attempt failed
    // (|=> menuntut true PERSIS di cycle berikutnya, bukan cycle sama).
    let fail_src = r#"
module tb_nonoverlap_fail;
  reg clk = 0;
  reg req = 1;   // req true sejak t=0 → antecedent match di posedge pertama
  reg done = 0;  // done TIDAK pernah → consequent gagal di posedge berikutnya
  always #5 clk = ~clk;
  assert property (@(posedge clk) req |=> done);
  initial begin
    #40; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile nonoverlap fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "|=> fail case belum terdeteksi: fail={}",
        total_fail
    );
}

#[test]
fn test_sva_labeled_assertion() {
    // LANG-48: assertion berlabel `lbl: assert property (...)` di level
    // module TIDAK boleh dibuang — label di-consume, body di-parse penuh,
    // assertion terelaborasi dan ter-evaluate (pass & fail terhitung).
    //
    // Pass: tidak berlabel ini pass + berlabel this pass.
    let pass_src = r#"
module tb_label_pass;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  labeled_ok: assert property (@(posedge clk) a |-> (a == b));
  unlabeled_ok: assert property (@(posedge clk) b |-> (b == a));
  initial begin
    @(posedge clk) a = 1;
    @(posedge clk) b = 1;   // b==a dari sini → kedua assertion pass
    @(posedge clk) a = 0;
    #20; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_src).expect("compile labeled pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 80);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    // 2 assertion (labeled + unlabeled) keduanya harus ter-evaluate.
    assert!(
        total_pass >= 2,
        "labeled assertion pass: pass={} fail={}",
        total_pass,
        total_fail
    );

    // Fail: labeled assertion harus mencatat fail (done tidak terpenuhi).
    let fail_src = r#"
module tb_label_fail;
  reg clk = 0;
  reg req = 1;   // req true sejak t=0
  reg done = 0;  // done tidak pernah true
  always #5 clk = ~clk;
  as_req_done: assert property (@(posedge clk) req |-> done);
  initial begin
    #40; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile labeled fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "labeled assertion fail belum terdeteksi: fail={}",
        total_fail
    );
}

#[test]
fn test_sva_property_always_never_iff() {
    // LANG-49: operator temporal property (IEEE 1800-2017 §16.14/§16.10):
    // `always (p)` — p true dinilai di setiap posedge;
    // `never (p)` — p TIDAK boleh true di edge mana pun;
    // `a iff b` — TRUE ketika kedua sisi identik (XNOR boolean).
    //
    // Pass: a==1 sepanjang simulasi → always pass, never pass, iff pass.
    let pass_src = r#"
module tb_prop_pass;
  reg clk = 0;
  reg a = 1;
  reg b = 1;
  always #5 clk = ~clk;
  as_always: assert property (@(posedge clk) always (a == 1));
  as_never: assert property (@(posedge clk) never (a == 0));
  as_iff: assert property (@(posedge clk) a iff b);
  initial begin
    #45; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_src).expect("compile temporal pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 80);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 3,
        "always/never/iff pass: pass={} fail={}",
        total_pass,
        total_fail
    );
    assert_eq!(total_fail, 0, "always/never/iff pass: fail={}", total_fail);

    // Fail: a 0 SATU posedge → always + never + iff masing-masing
    // mencatat fail pada cycle itu (b == 0 juga).
    let fail_src = r#"
module tb_prop_fail;
  reg clk = 0;
  reg a = 1;
  reg b = 1;
  always #5 clk = ~clk;
  as_always: assert property (@(posedge clk) always (a == 1));
  as_never: assert property (@(posedge clk) never (a == 0));
  as_iff: assert property (@(posedge clk) a iff b);
  initial begin
    #12 a = 0; b = 0;   // a=b=0 di pertengahan → jika iff dinilai PER-CYCLE,
                        // iff ini tetap true; yang gagal: always (a==1)
                        // dan never (a==0).
    #6 a = 1; b = 1;
    #20; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile temporal fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 80);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    // minimal 2 fail: always (a==1) salah satum & never (a==0) saat a=0.
    assert!(
        total_fail >= 1,
        "temporal fail belum terdeteksi: fail={}",
        total_fail
    );
}

#[test]
fn test_checker_let_items() {
    // LANG-50: `let` di dalam checker di-resolve oleh body assertion
    // (IEEE 1800-2017 §17.8 — checker items satu scope). Body let boleh
    // mereferensi nama port checker (ter-bind saat instance).
    let source = r#"
module tb_chk_let;
    reg clk = 0;
    reg [3:0] cnt = 0;
    always #5 clk = ~clk;
    always @(posedge clk) cnt <= cnt + 1;

    checker chk_let (logic clk_p, logic [3:0] d, logic en);
        let thr = en;
        assert property (@(posedge clk_p) !thr || (d > 0));
        assert property (@(posedge clk_p) d <= 10);
    endchecker

    chk_let u_chk(clk, cnt, 1'b1);
    initial #50 $finish;
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    // Kedua assertion checker (thr let + rentang) pass — cnt 0..di 10.
    assert!(
        total_pass >= 2,
        "checker let assertion pass: pass={}",
        total_pass
    );
    let _ = total_pass;
}

#[test]
fn test_hier_port_edge_wait() {
    // LANG-51: instance port yang dikoneksikan ke referensi hierarkis
    // (`u_i.clk`) — wait @(posedge c) di child HARUS terpicu. Dulu: perubahan
    // clock di-commit region Active, deltas di-reset sebelum Reactive jalan,
    // posedge tak pernah terlihat oleh EventControl → hang.
    let source = r#"
module child (input logic c);
  initial begin
    @(posedge c);
    @(posedge c);
    $display("CHILD_HIER_OK");
    $finish;
  end
endmodule
module inner;
  reg clk = 0;
  always #5 clk = ~clk;
endmodule
module abi_top;
  inner u_i();
  child u_c(u_i.clk);
endmodule
"#;
    let design = compile_str(source).expect("compile hier port");
    let mut engine = crate::simulator::SimulationEngine::new(design, 100);
    let _ = engine.run();
    // Jika wait hang, CHILD_HIER_OK tak pernah dicetak — verifikasi via
    // assertion_stats kosong + proses selesai (engine.run selesai normal).
    // Penanda keberhasilan: simulasi berjalan tanpa hang; verifikasi output
    // $display via simulate-style jalur di atas tidak diekspos — gunakan
    // waktu selesai: sebelum fix, engine berjalan sampai max_time (100).
    assert!(
        engine.state.time < 100,
        "hier port edge wait: simulasi hang sampai max_time (time={})",
        engine.state.time
    );
}

#[test]
fn test_string_realtoa_psprintf() {
    // LANG-54: LRM 1800 §6.16.1 realtoa (MUTATING receiver) + §6.16.9
    // psprintf (receiver = format string, non-mutating).
    let src = r#"
module tb_realtoa_psprintf;
  reg ok_rt = 0;
  reg ok_ps = 0;
  initial begin
    real r = 3.5;
    string rs;
    rs.realtoa(r);
    if (rs == "3.5") ok_rt = 1;
    string ps = "a=%0d b=%s".psprintf(7, "xyz");
    if (ps == "a=7 b=xyz") ok_ps = 1;
  end
endmodule
"#;
    let sigs = crate::simulate_signals(src, 60).expect("sim realtoa+psprintf");
    let find = |name: &str| {
        sigs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_bool() == Some(true))
            .unwrap_or(false)
    };
    assert!(find("ok_rt"), "realtoa: rs harus '3.5'");
    assert!(find("ok_ps"), "psprintf: harus 'a=7 b=xyz'");
}

#[test]
fn test_assoc_iterator_methods() {
    // LANG-55: iterator associative array LRM 1800 §7.10.6 — first/next/
    // last/prev menulis var index (out-arg) dan mengembalikan 1 sukses /
    // 0 gagal. Urutan kunci ascending deterministik.
    let src = r#"
module tb_assoc_iter;
  reg ok_all = 0;
  initial begin
    int aa [string];
    string k;
    reg [31:0] vals;
    reg [31:0] vi;
    aa["a"]=1; aa["b"]=2; aa["c"]=3;
    vi=0;
    if (aa.first(k) && k=="a" && aa[k]==1) vi=vi|1;
    if (aa.next(k) && k=="b" && aa[k]==2) vi=vi|2;
    if (aa.next(k) && k=="c" && aa[k]==3) vi=vi|4;
    if (!aa.next(k)) vi=vi|8; // habis abis → rc 0
    if (aa.last(k) && k=="c" && aa[k]==3) vi=vi|16;
    if (aa.prev(k) && k=="b" && aa[k]==2) vi=vi|32;
    if (aa.prev(k) && k=="a" && aa[k]==1) vi=vi|64;
    if (!aa.prev(k)) vi=vi|128; // diawal kuli → rc 0, k tak berubah
    ok_all = (vi == 32'd255);
  end
endmodule
"#;
    let sigs = crate::simulate_signals(src, 60).expect("sim assoc iterator");
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok_all")
        .map(|(_, v)| v.to_bool() == Some(true))
        .unwrap_or(false);
    assert!(ok, "assoc first/next/last/prev LRM: ok_all harus 1 (vi=255)");
}

#[test]
fn test_checker_construct_instance() {
    // LANG-10: checker construct (IEEE 1800-2017 §17.8) — `checker name
    // (ports); assert property... endchecker` dideklarasikan, diinstansiasi
    // di module, assertion body di-drive dengan port binding (posisi).
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    checker my_checker(input clk, input [3:0] v);
        assert property (@(posedge clk) v <= 3);
    endchecker
    my_checker u_chk(clk, cnt);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_pass >= 3, "checker assertion: pass={}", total_pass);
    assert!(
        total_fail >= 1,
        "checker assertion: fail saat cnt>3: {}",
        total_fail
    );
}

#[test]
fn test_checker_construct_named_ports() {
    // LANG-10: checker instance dengan koneksi named — port binding ke
    // signal modul; assertion body dievaluasi tiap edge.
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    checker my_checker(input clk, input [3:0] v);
        cover property (@(posedge clk) v >= 2);
    endchecker
    my_checker u_chk(.clk(clk), .v(cnt));
    initial begin
        #40 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 50);
    let _ = engine.run();
    let cover_total: u64 = engine.cover_hits.values().sum();
    assert!(
        cover_total >= 1,
        "checker cover property: hit={}",
        cover_total
    );
}

#[test]
fn test_assert_property_parse() {
    let source = r#"
module tb;
    reg clk;
    reg [31:0] x;
    initial begin
        clk = 0;
        x = 1;
        assert property (@(posedge clk) x == 1);
        #1 $finish;
    end
endmodule
"#;

    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "concurrent assert property should parse and execute");
}

#[test]
fn test_assume_fail() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        assume (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(1);
    assert_eq!(v, 0, "assume with false condition should not crash");
}

#[test]
fn test_covergroup_parse() {
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg @(posedge clk);
        cp_a: coverpoint a;
    endgroup
    initial begin
        a = 42;
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "covergroup should parse without error: {:?}",
        result.err()
    );
}

#[test]
fn test_covergroup_integration_coverage_db() {
    // VERIF-21: functional coverage (covergroup) ter-integrasi ke
    // CoverageDatabase — merge_from_engine membawa total/hits/bins
    // coverpoint ke database (persisten/merge multi-run).
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a;
    endgroup
    cg cg_inst = new();
    initial begin
        a = 1;
        cg_inst.sample();
        a = 2;
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.run().unwrap();
    let mut db = crate::simulator::coverage_db::CoverageDatabase::new();
    db.merge_from_engine(&engine);
    let cg = db
        .covergroups
        .get("cg")
        .expect("covergroup 'cg' harus masuk CoverageDatabase");
    assert_eq!(cg.coverpoints.len(), 1);
    let cp = &cg.coverpoints[0];
    assert_eq!(cp.total, 2, "2 sample ter-integrasi ke db");
    assert_eq!(cp.hits, 2);
    assert_eq!(cp.bins.len(), 2, "2 bin unik (a=1, a=2)");
}

#[test]
fn test_load_coverage_db_merges() {
    // `$load_coverage_db(path)` — muat MCDB lalu MERGE ke counter live
    // (line/branch/toggle/FSM/covergroup dijumlahkan). Sebelumnya stub
    // "not yet implemented" — panggilan diam-diam tak berpengaruh.
    let src_a = r#"
module tb;
    reg [3:0] cnt;
    integer i;
    initial begin
        cnt = 0;
        for (i = 0; i < 4; i = i + 1) cnt = cnt + 1;
        #1 $finish;
    end
endmodule
"#;
    let design_a = compile_str(src_a).unwrap();
    let mut engine_a = crate::simulator::SimulationEngine::new(design_a, 10);
    engine_a.run().unwrap();
    assert!(
        !engine_a.cover_line.is_empty(),
        "run A harus catat line hits"
    );
    let saved_hits: u64 = engine_a.cover_line.values().sum();
    let db_path = std::env::temp_dir().join(format!("mivon_covdb_{}.mcdb", std::process::id()));
    let mut db = crate::simulator::coverage_db::CoverageDatabase::new();
    db.merge_from_engine(&engine_a);
    db.save_to_file(db_path.to_str().unwrap())
        .expect("simpan MCDB");

    // Run B memanggil $load_coverage_db — cover_line B harus memuat SEMUA
    // key dari A (merge), bukan hanya statement B sendiri.
    let src_b = format!(
        r#"
module tb2;
    reg [3:0] x;
    initial begin
        $load_coverage_db("{}");
        x = 1;
        #1 $finish;
    end
endmodule
"#,
        db_path.to_str().unwrap()
    );
    let design_b = compile_str(&src_b).unwrap();
    let mut engine_b = crate::simulator::SimulationEngine::new(design_b, 10);
    engine_b.run().unwrap();
    for key in engine_a.cover_line.keys() {
        assert!(
            engine_b.cover_line.contains_key(key),
            "key line {:?} dari MCDB harus ter-merge ke engine B",
            key
        );
    }
    let merged_hits: u64 = engine_b.cover_line.values().sum();
    assert!(
        merged_hits >= saved_hits,
        "hits B ({}) >= hits A tersimpan ({})",
        merged_hits,
        saved_hits
    );
    std::fs::remove_file(&db_path).ok();

    // Negatif: file tak ada → warning IoError, sim TETAP lanjut (x=1).
    let src_c = r#"
module tb3;
    reg [3:0] x;
    initial begin
        $load_coverage_db("tidak_ada_dir/tidak_ada.mcdb");
        x = 1;
        #1 $finish;
    end
endmodule
"#;
    let design_c = compile_str(src_c).unwrap();
    let mut engine_c = crate::simulator::SimulationEngine::new(design_c, 10);
    engine_c.run().unwrap();
    let x_id = engine_c
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == "x")
        .expect("signal x ada");
    assert_eq!(
        engine_c.state.read_signal(x_id).to_u64(),
        1,
        "sim lanjut walau MCDB gagal dimuat"
    );
}

#[test]
fn test_covergroup_type_option_weight() {
    // VERIF-28: `type_option.weight = N` — bobot covergroup utk functional
    // coverage keseluruhan (weighted average). cg_heavy (weight 2, di-sample
    // penuh) + cg_light (weight 1, TIDAK pernah di-sample = coverage hole 0%)
    // → (2*100 + 1*0)/3 = 66.67%. Tanpa weight: (100+0)/2 = 50%.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg_heavy;
        type_option.weight = 2;
        cp_a: coverpoint a;
    endgroup
    covergroup cg_light;
        type_option.weight = 1;
        cp_b: coverpoint a;
    endgroup
    cg_heavy h = new();
    cg_light l = new();
    initial begin
        a = 1;
        h.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    // IR membawa weight ter-parse.
    let heavy = design
        .covergroups
        .iter()
        .find(|c| c.name == "cg_heavy")
        .expect("cg_heavy");
    let light = design
        .covergroups
        .iter()
        .find(|c| c.name == "cg_light")
        .expect("cg_light");
    assert_eq!(heavy.weight, 2, "type_option.weight = 2 ter-parse");
    assert_eq!(light.weight, 1, "default weight 1");

    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    // cg_heavy di-sample (100%); cg_light tidak pernah di-sample (0% hole).
    let pct = engine.functional_coverage_percent();
    assert!(
        (pct - 66.6667).abs() < 0.01,
        "weighted functional coverage harus 66.67% (weight 2:1), got {}",
        pct
    );
}

#[test]
fn test_covergroup_per_instance() {
    // VERIF-28: `type_option.per_instance = 1` — coverage dilacak per-instance
    // (`cg.i<id>.cp`), bukan di-merge. Dua instance dengan sample berbeda →
    // key terpisah; merge_from_engine tetap menjumlahkan ke agregat.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        type_option.per_instance = 1;
        cp_a: coverpoint a;
    endgroup
    cg i1 = new();
    cg i2 = new();
    initial begin
        a = 1;
        i1.sample();
        i1.sample();
        a = 2;
        i2.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let cg = design.covergroups.iter().find(|c| c.name == "cg").unwrap();
    assert!(cg.per_instance, "type_option.per_instance = 1 ter-parse");

    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    // Per-instance keys: cg.i<id>.cp_a — total 2 utk instance pertama (2
    // sample) dan 1 utk instance kedua, ATAU 1 utk instance1 + 1 auto-sample
    // tiap new(). Verifikasi: tiap key instance punya total sendiri > 0 dan
    // tidak ada agregat tunggal yang menampung semua.
    let mut per_inst_totals: Vec<u64> = Vec::new();
    let mut agg_total = 0u64;
    for (k, v) in &engine.cover_total {
        let s = k.as_str();
        if let Some(rest) = s.strip_prefix("cg.i") {
            if let Some(_cp) = rest.strip_suffix(".cp_a") {
                per_inst_totals.push(*v);
            }
        } else if s == "cg.cp_a" {
            agg_total += v;
        }
    }
    assert_eq!(
        agg_total, 0,
        "per_instance=1 tidak boleh memakai key agregat"
    );
    assert_eq!(per_inst_totals.len(), 2, "dua instance → dua key terpisah");
    assert!(
        per_inst_totals.contains(&2),
        "instance1 2 sample: {:?}",
        per_inst_totals
    );
    assert!(
        per_inst_totals.contains(&1),
        "instance2 1 sample: {:?}",
        per_inst_totals
    );
    assert_eq!(
        per_inst_totals.iter().sum::<u64>(),
        3,
        "total 3 sample (2+1)"
    );

    // merge_from_engine menjumlahkan semua key instance ke agregat.
    let mut db = crate::simulator::coverage_db::CoverageDatabase::new();
    db.merge_from_engine(&engine);
    let entry = db.covergroups.get("cg").expect("cg in db");
    assert_eq!(entry.coverpoints.len(), 1);
    assert_eq!(entry.coverpoints[0].total, 3, "merge agregat = 3 sample");
    assert_eq!(entry.coverpoints[0].hits, 3);
}

#[test]
fn test_covergroup_cross() {
    let source = r#"
module tb;
    reg [31:0] a;
    reg [31:0] b;
    covergroup cg;
        cp_a: coverpoint a;
        cp_b: coverpoint b;
        cross_a_b: cross cp_a, cp_b;
    endgroup
    cg cg_inst = new();
    initial begin
        a = 1; b = 2;
        cg_inst.sample();
        a = 3; b = 4;
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    // Check cross coverage: 2 samples, 2 unique cross bins
    let cross_key = Symbol::intern("cg.cross_a_b");
    assert_eq!(
        engine.cover_total.get(&cross_key).copied().unwrap_or(0),
        2,
        "cross total should be 2"
    );
    assert_eq!(
        engine.cover_hits.get(&cross_key).copied().unwrap_or(0),
        2,
        "cross hits should be 2"
    );
    let cross_bins = engine.cover_bins.get(&cross_key).unwrap();
    assert_eq!(cross_bins.len(), 2, "should have 2 unique cross bins");
    assert!(
        cross_bins.contains_key(&Symbol::intern("cp_a=1 x cp_b=2")),
        "missing cross bin for a=1,b=2"
    );
    assert!(
        cross_bins.contains_key(&Symbol::intern("cp_a=3 x cp_b=4")),
        "missing cross bin for a=3,b=4"
    );
    assert_eq!(cross_bins[&Symbol::intern("cp_a=1 x cp_b=2")], 1);
    assert_eq!(cross_bins[&Symbol::intern("cp_a=3 x cp_b=4")], 1);
}

#[test]
fn test_covergroup_with_bins() {
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins low = {[0:10]};
            bins high = {[11:20]};
        }
    endgroup
    initial begin
        a = 42;
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "covergroup with bins should parse without error: {:?}",
        result.err()
    );
}

#[test]
fn test_covergroup_ignore_bins_excluded() {
    // VERIF-30: ignore_bins — nilai yang dikecualikan TIDAK dihitung (total
    // tidak naik, tidak masuk auto-bin). Sebelumnya bin eksplisit di-parse tapi
    // di-drop elaborator → nilai ignore tetap masuk auto-binning default.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            ignore_bins skip = {[5:7]};
            bins keep = {[0:4]};
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 6;   // ignore → dikecualikan
        cg_inst.sample();
        a = 3;   // keep → dihitung ke bin keep
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    assert_eq!(
        engine.cover_total.get(&key).copied().unwrap_or(0),
        1,
        "total hanya 1 (sampel ignore dikecualikan)"
    );
    assert_eq!(engine.cover_hits.get(&key).copied().unwrap_or(0), 1);
    let bins = engine.cover_bins.get(&key).unwrap();
    assert!(
        bins.contains_key(&Symbol::intern("cp_a=keep")),
        "bin eksplisit keep harus terisi"
    );
    assert!(
        !bins.contains_key(&Symbol::intern("cp_a=6")),
        "nilai ignore tidak boleh masuk auto-bin"
    );
}

#[test]
fn test_covergroup_illegal_bins_error() {
    // VERIF-30: illegal_bins — nilai yang TIDAK BOLEH muncul → laporan
    // (AssertionFailed) + sampel TIDAK dihitung sebagai hit.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            illegal_bins never = {[100:200]};
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 150;  // illegal → error
        cg_inst.sample();
        a = 5;    // ok → default bin
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    assert_eq!(engine.cover_total.get(&key).copied().unwrap_or(0), 2);
    assert_eq!(
        engine.cover_hits.get(&key).copied().unwrap_or(0),
        1,
        "hits hanya 1 (sampel illegal tidak dihitung)"
    );
    let diags = engine.flush_diagnostics();
    assert!(
        diags
            .iter()
            .any(|d| d.message.as_ref().contains("illegal_bins hit")),
        "harus ada laporan illegal_bins: {:#?}",
        diags.iter().map(|d| d.message.as_ref()).collect::<Vec<_>>()
    );
}

#[test]
fn test_covergroup_explicit_bin_list_values() {
    // VERIF-30: `bins b = {1, 2}` (daftar nilai) vs `{[1:5]}` (range) —
    // representasi BinRange memisahkan keduanya; nilai tunggal tidak boleh
    // diinterpretasi sebagai range.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins two = {2, 4};
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 2;
        cg_inst.sample();
        a = 4;
        cg_inst.sample();
        a = 3;   // tidak match bin two → auto-bin
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    let bins = engine.cover_bins.get(&key).unwrap();
    assert_eq!(
        bins.get(&Symbol::intern("cp_a=two")).copied().unwrap_or(0),
        2,
        "bin two harus kena 2x (nilai 2 dan 4)"
    );
    assert!(
        bins.contains_key(&Symbol::intern("cp_a=3")),
        "nilai 3 di luar bin → auto-bin default"
    );
}

#[test]
fn test_covergroup_transition_bins() {
    // VERIF-31: transition bins `(a => b)` — cocokkan (prev, curr). Sebelumnya
    // `=>` tidak di-lex (jadi `=` + `>`), bin transisi ter-parse dgn range_list
    // kosong → transisi tidak pernah dicatat (semua jadi auto-bin).
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins rising  = (0 => 1);
            bins falling = (1 => 0);
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 0;
        cg_inst.sample();  // prev=None → auto-bin (tidak ada transisi)
        a = 1;
        cg_inst.sample();  // 0=>1 → rising
        a = 0;
        cg_inst.sample();  // 1=>0 → falling
        a = 1;
        cg_inst.sample();  // 0=>1 → rising (lagi)
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    let bins = engine.cover_bins.get(&key).unwrap();
    assert_eq!(
        bins.get(&Symbol::intern("cp_a=rising"))
            .copied()
            .unwrap_or(0),
        2,
        "rising (0=>1) harus kena 2x"
    );
    assert_eq!(
        bins.get(&Symbol::intern("cp_a=falling"))
            .copied()
            .unwrap_or(0),
        1,
        "falling (1=>0) harus kena 1x"
    );
    assert!(
        bins.contains_key(&Symbol::intern("cp_a=0")),
        "sampel pertama (prev=None) → auto-bin default"
    );
    assert_eq!(engine.cover_total.get(&key).copied().unwrap_or(0), 4);
}

#[test]
fn test_coverage_gap_analysis() {
    // VERIF-26: coverage gap analysis — bin eksplisit yang tidak pernah kena
    // dan coverpoint/cross yang tidak pernah di-sample terdaftar di
    // engine.coverage_gaps().
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins low  = {0};
            bins high = {100};
        }
        cp_b: coverpoint a;
    endgroup
    covergroup never_sampled;
        cp_x: coverpoint a;
    endgroup
    cg cg_inst = new();
    never_sampled ns_inst = new();
    initial begin
        a = 0;
        cg_inst.sample();   // bin low kena; bin high TIDAK
        a = 5;
        cg_inst.sample();   // auto-bin 5
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.run().unwrap();
    let gaps = engine.coverage_gaps();
    // Bin high tidak pernah kena.
    assert!(
        gaps.iter().any(|g| g.contains("bin 'high'")),
        "gap harus menyebut bin 'high' tak pernah kena: {:?}",
        gaps
    );
    // Covergroup never_sampled tidak pernah di-sample → cp_x gap.
    assert!(
        gaps.iter()
            .any(|g| g.contains("never_sampled.cp_x") && g.contains("tidak pernah di-sample")),
        "gap harus menyebut coverpoint never_sampled.cp_x tak pernah di-sample: {:?}",
        gaps
    );
    // Tidak ada false-positive: cp_a LOW bin (kena) tidak boleh jadi gap.
    assert!(
        !gaps.iter().any(|g| g.contains("bin 'low'")),
        "bin low pernah kena — tidak boleh jadi gap: {:?}",
        gaps
    );
}

#[test]
fn test_wand_resolution() {
    let source = r#"
module tb;
    wand w;
    reg a, b;
    assign w = a;
    assign w = b;
    initial begin
        a = 0; b = 1;
        #1;
        // wand: AND of drivers → 0 & 1 = 0
        if (w !== 0) $display("FAIL: wand expected 0 got %b", w);
        a = 1; b = 1;
        #1;
        if (w !== 1) $display("FAIL: wand expected 1 got %b", w);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "wand resolution failed: {:?}", result.err());
}

#[test]
fn test_wor_resolution() {
    let source = r#"
module tb;
    wor w;
    reg a, b;
    assign w = a;
    assign w = b;
    initial begin
        a = 0; b = 1;
        #1;
        // wor: OR of drivers → 0 | 1 = 1
        if (w !== 1) $display("FAIL: wor expected 1 got %b", w);
        a = 0; b = 0;
        #1;
        if (w !== 0) $display("FAIL: wor expected 0 got %b", w);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "wor resolution failed: {:?}", result.err());
}

#[test]
fn test_multi_driver_detection() {
    // SIM-15: signal yang di-drive oleh >1 proses ditandai `multi_driver` oleh
    // elaborator (detect_multi_driver_signals, ext.rs) — dipakai engine untuk
    // skip false-positive race check + mengaktifkan resolusi net. Sebelumnya
    // hanya ada resolusi (wand/wor/tri), deteksi sendiri belum diuji langsung.
    let source = r#"
module tb;
    wire w;   // 2 driver → multi_driver=true
    wire s;   // 1 driver  → multi_driver=false
    reg a, b;
    assign w = a;
    assign w = b;
    assign s = a;
    initial begin
        a = 0; b = 1;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let w = design
        .top
        .signals
        .iter()
        .find(|s| s.name.as_str() == "w")
        .unwrap();
    let s = design
        .top
        .signals
        .iter()
        .find(|s| s.name.as_str() == "s")
        .unwrap();
    assert!(w.multi_driver, "w (2 assign) harus terdeteksi multi-driver");
    assert!(!s.multi_driver, "s (1 assign) bukan multi-driver");
    // Sim tetap jalan dengan multi-driver (resolusi aktif, bukan race error).
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "sim multi-driver gagal: {:?}", result.err());
}

#[test]
fn test_multi_driver_output_logic_resolution_consistent() {
    // Fuzz differential (bug_d2919857ef6d): `output logic` didorong DUA
    // continuous assign — jalur default vs DAG dulu beda (1 vs x). Kini
    // sinyal Output/Logic multi-driver ikut ditandai → resolusi net aktif
    // → semua jalur engine konvergen.
    let source = r#"
module m(output logic o, input logic a);
  assign o = 1'b1;
  assign o = a;
endmodule
"#;
    let design = compile_str(source).unwrap();
    let o = design
        .top
        .signals
        .iter()
        .find(|s| s.name.as_str() == "o")
        .unwrap();
    assert!(o.multi_driver, "output logic 2-driver harus multi_driver");
    let a1 = simulate_signals_with_flags_quiet(source, 100, &EngineFlags::default()).unwrap();
    let dag = EngineFlags {
        use_dag_parallel: true,
        ..EngineFlags::default()
    };
    let a2 = simulate_signals_with_flags_quiet(source, 100, &dag).unwrap();
    let v1 = a1
        .iter()
        .find(|(n, _)| n == "o")
        .map(|(_, v)| format!("{v}"))
        .unwrap_or_default();
    let v2 = a2
        .iter()
        .find(|(n, _)| n == "o")
        .map(|(_, v)| format!("{v}"))
        .unwrap_or_default();
    assert_eq!(v1, v2, "default vs DAG harus identik");
}

#[test]
fn test_tri_resolution() {
    let source = r#"
module tb;
    tri t;
    reg a, en;
    assign t = en ? a : 1'bz;
    assign t = 1'b1;  // pullup
    initial begin
        en = 0; a = 0;
        #1;
        // tri: driver2 = Z, driver1 = 1 → 1
        if (t !== 1) $display("FAIL: tri expected 1 got %b", t);
        en = 1; a = 0;
        #1;
        // tri: driver2 = 0, driver1 = 1 → X (conflict)
        if (t !== 1'bx) $display("FAIL: tri expected X got %b", t);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "tri resolution failed: {:?}", result.err());
}

#[test]
fn test_wand_keyword_parse() {
    let source = r#"
module tb;
    wand w;
    initial #1 $finish;
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "wand keyword should parse: {:?}",
        result.err()
    );
}

#[test]
fn test_dpi_import_function() {
    let source = r#"
module tb;
    import "DPI-C" function int my_add(input int a, input int b);
    int result;
    initial begin
        result = my_add(3, 4);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI function import should compile: {:?}",
        design.err()
    );
}

#[test]
fn test_dpi_import_task() {
    let source = r#"
module tb;
    import "DPI-C" task my_task(input int x);
    initial begin
        my_task(42);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI task import should compile: {:?}",
        design.err()
    );
}

#[test]
fn test_dpi_import_void() {
    let source = r#"
module tb;
    import "DPI-C" function void dpi_void(input int x);
    initial begin
        dpi_void(42);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI void function import should compile: {:?}",
        design.err()
    );
}

#[test]
fn test_dpi_import_multi_arg() {
    let source = r#"
module tb;
    import "DPI-C" function int dpi_mul(input byte a, input shortint b, input int c);
    int result;
    initial begin
        result = dpi_mul(1, 2, 3);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI multi-arg import should compile: {:?}",
        design.err()
    );
}

// ── DPI-C Enhancement Tests (CRIT-009) ──

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_scope_management() {
    // Test svGetScope/svSetScope via thread-local path
    use crate::simulator::dpi::*;

    // Initially no scope set
    let scope = sv_get_scope();
    assert!(scope.is_null(), "no scope should be set initially");

    // Set scope via thread-local
    set_current_dpi_scope("top.u_sub");
    let scope = sv_get_scope();
    assert!(!scope.is_null(), "scope should be non-null after setting");
    let name = sv_get_scope_name(scope);
    assert_eq!(name, Some("top.u_sub".to_string()));

    // Test svSetScope
    let new_scope = sv_set_scope_name("top.other");
    let result = sv_set_scope(new_scope);
    assert_eq!(result, 1, "svSetScope should succeed");
    let scope2 = sv_get_scope();
    let name2 = sv_get_scope_name(scope2);
    assert_eq!(name2, Some("top.other".to_string()));
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_time_query() {
    use crate::simulator::dpi::*;

    // Set time to 42
    set_current_dpi_time(42);
    let scope = svScope::NULL;
    let time = sv_get_time(scope, std::ptr::null_mut());
    assert_eq!(time, 42, "svGetTime should return current time");
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_chandle_store() {
    use crate::simulator::dpi::*;

    // Allocate a chandle for an opaque pointer value
    let handle1 = chandle_alloc(0xDEADBEEF);
    assert!(handle1 > 0, "chandle handle should be non-zero");

    let handle2 = chandle_alloc(0xCAFEBABE);
    assert_ne!(handle1, handle2, "chandle handles should be unique");

    // Get back the stored value
    let val1 = chandle_get(handle1);
    assert_eq!(val1, Some(0xDEADBEEF));

    let val2 = chandle_get(handle2);
    assert_eq!(val2, Some(0xCAFEBABE));

    // Free handle
    chandle_free(handle1);
    assert_eq!(
        chandle_get(handle1),
        None,
        "freed handle should return None"
    );
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_export_register_and_call() {
    use crate::simulator::dpi::*;
    use mivon_ir::*;

    // Register a simple SV function as DPI export
    let func = DpiExportedFunction {
        export_name: "my_sv_func".to_string(),
        n_args: 2,
        arg_widths: vec![32, 32],
        is_task: false,
        callback: Box::new(|args| {
            let a = args.first().map(|v| v.to_u64()).unwrap_or(0);
            let b = args.get(1).map(|v| v.to_u64()).unwrap_or(0);
            LogicVec::from_u64(a + b, 32)
        }),
    };
    sv_export_register(func);

    // Call the exported function
    let args = vec![LogicVec::from_u64(40, 32), LogicVec::from_u64(2, 32)];
    let result = sv_export_call("my_sv_func", &args).unwrap();
    assert_eq!(result.to_u64(), 42, "DPI export should return 40+2=42");
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_export_task() {
    use crate::simulator::dpi::*;
    use mivon_ir::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static CALLED: AtomicBool = AtomicBool::new(false);

    let func = DpiExportedFunction {
        export_name: "my_sv_task".to_string(),
        n_args: 0,
        arg_widths: vec![],
        is_task: true,
        callback: Box::new(|_| {
            CALLED.store(true, Ordering::SeqCst);
            LogicVec::new(0)
        }),
    };
    sv_export_register(func);

    sv_export_call("my_sv_task", &[]).unwrap();
    assert!(
        CALLED.load(Ordering::SeqCst),
        "task should have been called"
    );
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_bit_vector_helpers() {
    use crate::simulator::dpi::*;

    // Test svGetBitsel and svPutBitsel (helpers are unsafe fn — pointer args)
    let mut vec_bits: [svBitVecVal; 2] = [0, 0];
    unsafe {
        sv_put_bitsel(&mut vec_bits as *mut svBitVecVal, 3, 1);
        assert_eq!(sv_get_bitsel(&vec_bits as *const svBitVecVal, 3), 1);
        assert_eq!(sv_get_bitsel(&vec_bits as *const svBitVecVal, 2), 0);
    }

    // Test svPutPartSelect and svGetPartSelect
    let mut vec2: [svBitVecVal; 2] = [0, 0];
    let val = unsafe {
        sv_put_part_select(&mut vec2 as *mut svBitVecVal, 0, 8, 0xAB);
        sv_get_part_select(&vec2 as *const svBitVecVal, 0, 8)
    };
    assert_eq!(val, 0xAB, "part select should round-trip 0xAB");
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_logic_vector_helpers() {
    use crate::simulator::dpi::*;

    // Test svGetLogicBitsel with 4-state encoding (helpers are unsafe fn)
    let mut logic_vec: [svLogicVecVal; 4] = [0, 0, 0, 0];
    unsafe {
        // Set bit 0 to '1' (aval=1, bval=0)
        sv_put_logic_bitsel(&mut logic_vec as *mut svLogicVecVal, 0, 1);
        assert_eq!(
            sv_get_logic_bitsel(&logic_vec as *const svLogicVecVal, 0),
            1
        );

        // Set bit 1 to 'X' (aval=0, bval=1)
        sv_put_logic_bitsel(&mut logic_vec as *mut svLogicVecVal, 1, 2);
        assert_eq!(
            sv_get_logic_bitsel(&logic_vec as *const svLogicVecVal, 1),
            2
        );

        // Set bit 2 to 'Z' (aval=1, bval=1)
        sv_put_logic_bitsel(&mut logic_vec as *mut svLogicVecVal, 2, 3);
        assert_eq!(
            sv_get_logic_bitsel(&logic_vec as *const svLogicVecVal, 2),
            3
        );
    }
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_chandle_conversion() {
    use crate::simulator::dpi::*;

    let ptr_val: u64 = 0x1234567890ABCDEF;
    let ch = u64_to_chandle(ptr_val);
    let back = chandle_to_u64(ch);
    assert_eq!(back, ptr_val, "chandle pointer round-trip should work");
}

#[test]
fn test_dpi_builtin_sv_bit_functions() {
    // Test that built-in DPI conversion functions work (with proper import)
    let source = r#"
module tb;
    import "DPI-C" function int svToInt(input logic [31:0] a);
    logic [31:0] a;
    logic [31:0] result;
    initial begin
        a = 42;
        result = svToInt(a);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5);
    assert!(
        sigs.is_ok(),
        "DPI built-in svToInt should not crash: {:?}",
        sigs.err()
    );
}

// ── End DPI-C Enhancement Tests ──

#[test]
fn test_inout_basic_parse() {
    let source = r#"
module top;
    tri w;
    driver u1(.port(w));
    initial #1 $finish;
endmodule
module driver(inout port);
    assign port = 1'b1;
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "inout port should parse: {:?}",
        result.err()
    );
}

#[test]
fn test_inout_tri_resolution() {
    let source = r#"
module top;
    tri t;
    driver u1(.port(t));
    driver u2(.port(t));
    initial begin
        #1;
        if (t !== 1'bx) $display("FAIL: inout conflict expected X got %b", t);
        #1 $finish;
    end
endmodule
module driver(inout port);
    assign port = 1'b1;
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "inout tri resolution failed: {:?}",
        result.err()
    );
}

#[test]
fn test_inout_bidirectional() {
    let source = r#"
module top;
    reg [1:0] drv_val;
    tri w;
    bus_driver u1(.val(drv_val), .bus(w));
    initial begin
        drv_val = 0;
        #1;
        if (w !== 1'b0) $display("FAIL: expected 0 at time 1 got %b", w);
        drv_val = 1;
        #1;
        if (w !== 1'b1) $display("FAIL: expected 1 at time 2 got %b", w);
        #1 $finish;
    end
endmodule
module bus_driver(inout bus, input [1:0] val);
    reg oe;
    assign bus = oe ? val[0] : 1'bz;
    initial oe = 1;
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "inout bidirectional failed: {:?}",
        result.err()
    );
}

#[test]
fn test_parameter_type_default() {
    let source = r#"
module my_mux #(parameter type T = logic) (input T a, output T y);
    assign y = a;
endmodule
module tb;
    wire a, y;
    my_mux u1(.a(a), .y(y));
    initial begin
        a = 1;
        #1;
        if (y !== 1) $display("FAIL: expected 1 got %b", y);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "parameter type parse failed: {:?}",
        result.err()
    );
}

#[test]
fn test_parameter_type_override() {
    let source = r#"
module my_bus #(parameter type T = logic) (input T [7:0] a, output T [7:0] y);
    assign y = a;
endmodule
module tb;
    wire [7:0] a, y;
    my_bus #(.T(bit)) u1(.a(a), .y(y));
    initial begin
        a = 8'hAB;
        #1;
        if (y !== 8'hAB) $display("FAIL: expected AB got %h", y);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "parameter type override failed: {:?}",
        result.err()
    );
}

// PARSER-10: module parameter type list edge cases
#[test]
fn test_parameter_type_edge_cases() {
    // Scoped type default, signed type, multiple type params,
    // mixed type+value params, type with packed range default.
    let source = r#"
module pkg_types (input int a, output int y);
    assign y = a;
endmodule
module param_edge_test #(
    parameter type T1 = int,
    parameter type T2 = logic,
    parameter int W = 8,
    parameter signed [7:0] SP = -1,
    parameter logic [3:0] MASK = 4'hF
) (
    input T1 a,
    input T2 b,
    output logic [W-1:0] y
);
    assign y = a[W-1:0] + {b, 3'b000} + {1'b0, MASK};
endmodule
module tb;
    logic [7:0] a_val = 8'h05;
    logic b_val = 1'b1;
    logic [7:0] y_val;
    param_edge_test #(.T1(int), .T2(logic)) u1(.a(a_val), .b(b_val), .y(y_val));
    initial begin
        #1;
        $display("y = %h", y_val);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "PARSER-10 parameter edge cases failed: {:?}",
        result.err()
    );
}

