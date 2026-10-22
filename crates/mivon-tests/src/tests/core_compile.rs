//! Test dasar kompilasi & tipe data: modul sederhana, struct/typedef dan
//! offset member, pipeline compile (file tracking), string, serta PLI/VPI.

use super::*;

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
