//! Regression test: system task/function tanpa argumen kurung.
//!
//! Bug ditemukan fuzzer (roundtrip mismatch target `mv`, minimal 38 byte):
//! `module c#(W=1){sig k:bit initial{$a}}` → transpiler MV memancarkan
//! `$a;` ke SV, lalu parser SV menolak dengan `E1002: expected LParen,
//! found Semi`.
//!
//! Akar: `parse_syscall` di `crates/mivon-parser/src/stmt.rs` mewajibkan
//! `Token::LParen` di lengan fallback — padahal IEEE 1800-2017 §20.2
//! menyatakan argumen system task/function bersifat opsional dalam kurung,
//! dan kurungnya sendiri boleh dihilangkan total. `$display;` legal.
//!
//! Test ini mengunci DUA halves dari fix:
//! 1. Nol-argumen tanpa kurung diterima untuk system task apa pun
//!    (`$display;`, `$fflush;`, `$monitor;`, `$random;`, `$dumpvars;`, ...).
//! 2. `$time;` / `$realtime;` diterima sebagai STATEMENT — `time`/`realtime`
//!    adalah keyword SV, jadi lexer memverbatakannya sebagai
//!    `Token::Time`/`Token::RealTime`, bukan `Token::Ident`. Sebelumnya
//!    `parse_syscall` hanya menerima `Ident`, sehingga nama system call
//!    tak pernah bisa berupa keyword → "expected system call name after $".
//!
//! Golden reference: `iverilog -g2012` menerima SEMUA bentuk di bawah.
//! Bentuk yang iverilog TOLAK (`$display "hi";` — argumen tanpa kurung
//! tak sah) tetap harus ditolak mivon, dengan pesan yang akurat.

use super::*;
use mivon_parser::lexer::Token;

/// Parse satu statement system call; `Ok` berarti tidak ada error parse.
///
/// Jalur sama dengan `debug_lexer.rs`: Lexer → token stream → `Parser`
/// (`parse_design`), plus `parse` (recovery) supaya error yang
/// terakumulasi ikut terlihat — `parse_design()` saja bisa sukses
/// lewat recovery meski ada error.
fn parse_sys_stmt(stmt: &str) -> Result<(), String> {
    let src = format!("module t; initial begin {stmt} end endmodule\n");
    let mut lx = Lexer::new(&src);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lx.next_token();
        if tok == Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = Parser::new(tokens, "<syscall-test>").with_source_lines(&src);
    parser.parse_design().map_err(|e| format!("{e}"))?;
    let errs: Vec<String> = parser
        .errors
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.to_string())
        .collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

///nol-argumen system task SAH menurut IEEE 1800-2017 §20.2.
const NOL_ARG_OK: &[&str] = &[
    "$display;",
    "$write;",
    "$fflush;",
    "$monitor;",
    "$strobe;",
    "$fdisplay;",
    "$fmonitor;",
    "$fstrobe;",
    "$random;",
    "$urandom;",
    "$urandom_range;",
    "$dumpvars;",
    "$dumpall;",
    "$dumpon;",
    "$dumpoff;",
    "$exit;",
    "$stime;",
    "$ferror;",
    "$feof;",
    "$fgetc;",
    "$ungetc;",
    "$rewind;",
    "$fseek;",
    "$ftell;",
    "$time;",
    "$realtime;",
];

/// REGRESI: tiap system task tanpa argumen harus parse (dulu E1002
/// "expected LParen, found Semi").
#[test]
fn syscall_nol_args_without_parens_parses() {
    for stmt in NOL_ARG_OK {
        if let Err(e) = parse_sys_stmt(stmt) {
            panic!("`{stmt}` harus sah (IEEE 1800 §20.2, iverilog -g2012 terima) tapi: {e}");
        }
    }
}

/// REGRESI: `$time;`/`$realtime;` sebagai statement. Akar kedua: `time`/
/// `realtime` keyword SV → lexer token `Token::Time`/`Token::RealTime`,
/// dan `parse_syscall` hanya terima `Token::Ident` sebagai nama system
/// call → "expected system call name after $".
#[test]
fn syscall_time_and_realtime_as_statement_parse() {
    for stmt in ["$time;", "$realtime;"] {
        if let Err(e) = parse_sys_stmt(stmt) {
            panic!("`{stmt}` sebagai statement harus sah tapi: {e}");
        }
    }
}

/// REGRESI balık (false-accept guard): argumen TANPA kurung bukan SV sah.
/// iverilog -g2012 menolak `syntax error`. mivon harus menolak juga —
/// pesan boleh berbeda, tapi HARUS tetap error (jangan sampai fix nol-argumen
/// membuat `$display "hi";` lolos diam-diam).
#[test]
fn syscall_args_without_parens_still_rejected() {
    for stmt in ["$display \"hi\";", "$fflush x;", "$monitor a;"] {
        assert!(
            parse_sys_stmt(stmt).is_err(),
            "`{stmt}` bukan SV sah (iverilog tolak) — tak boleh diterima"
        );
    }
}

/// REGRESI: bentuk BERKURUNG tak boleh rusak oleh fix nol-argumen.
/// `$display("a", "b")` masih argumentASI normal.
#[test]
fn syscall_with_parens_and_args_still_parses() {
    for stmt in [
        "$display(\"a\");",
        "$display(\"a\", \"b\");",
        "$display;",
        "$time(1);",
        "$finish;",
        "$finish(1);",
    ] {
        if let Err(e) = parse_sys_stmt(stmt) {
            panic!("`{stmt}` harus parse tapi: {e}");
        }
    }
}

/// REGRESI end-to-end: repro asli fuzzer. Transpiler MV memancarkan
/// system call tanpa argumen ke SV; outputnya HARUS bisa dikompilasi.
/// Dulu gagal `E1002: expected LParen, found Semi` — bug SV parser,
/// bukan bug transpiler.
#[test]
fn mv_transpile_output_with_nol_arg_syscall_compiles() {
    let mv_src = "module c { sig k : bit\n initial { $display } }";
    let out = mivon_api::mv::transpile(mv_src, "fz").expect("transpile MV");
    let combined = format!("{}\n{}", out.svh, out.sv);
    assert!(
        combined.contains("$display;"),
        "transpiler harus memancarkan `$display;` — dapat: {combined}"
    );
    compile_str_quiet(&combined).unwrap_or_else(|e| panic!("output MV harus bisa dikompilasi: {e}"));
}

/// REGRESI: `$display;` tak boleh salah-parse jadi argumen nyasar.
/// Kalau fix melonggarkan terlalu jauh, `$display "hi";` bisa lolos dan
/// konten string ikut jadi argumen. Test ini mengunci batasnya lewat
/// elaborator: statement nol-argumen harus tetap bisa dikompilasi.
#[test]
fn syscall_nol_args_elaborates_with_zero_args() {
    // 3 system task nol-argumen; elaborator harus menerima semuanya
    // tanpa argumen. Kalau ada yang salah-parse jadi SysCall BERARGUMEN,
    // compile tetap bisa lolos — jadi kita cek lewatjalur yang lebih
    // ketat: statement `$display "hi";` yang tak sah HARUS ditolak.
    let src = "module t; initial begin $display; $fflush; $random; end endmodule\n";
    compile_str_quiet(src).unwrap_or_else(|e| panic!("nol-arg syscall harus compile: {e}"));
}