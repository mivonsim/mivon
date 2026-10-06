//! Regression test: system task/function tanpa argumen kurung.
//!
//! Bug ditemukan fuzzer (roundtrip mismatch target `mv`, minimal 38 byte):
//! `module c#(W=1){sig k:bit initial{$a}}` → transpiler MV memancarkan
//! `$a;` ke SV, lalu parser SV menolak dengan `E1002: expected LParen,
//! found Semi`.
//!
//! Akar: `parse_syscall` di `crates/mivon-parser/src/stmt.rs` mewajibkan
//! `Token::LParen` di lengan fallback — padahal BNF `system_task_call`
//! (IEEE 1800-2017 §20 preamble, plus §20.1 untuk display task)
//! menyatakan daftar argumen bersifat opsional dalam kurung, dan kurungnya
//! sendiri boleh dihilangkan total. `$display;` legal.
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

/// Nol-argumen yang PASTI sah menurut IEEE 1800-2017: display task dengan
/// daftar argumen opsional (§20.1), plus §20.2 file I/O, §20.11 dumpvars,
/// §20.12 severity/exit. Semua diuji golden: `iverilog -g2012` menerima.
const NOL_ARG_OK: &[&str] = &[
    "$display;",
    "$write;",
    "$fflush;",
    "$monitor;",
    "$strobe;",
    "$dumpvars;",
    "$dumpall;",
    "$dumpon;",
    "$dumpoff;",
    "$exit;",
];

/// Nol-argumen yang IEEE 1800-2017 TIDAK mewajibkan (system *function*
/// dibuang sebagai statement, atau task dengan argumen wajib seperti
/// `$fdisplay` yang butuh `fd`) — TAPI `iverilog -g2012` menerimanya.
///
/// Test ini mengunci komitmen mivon, bukan klaim LRM: parser tidak boleh
/// menolak bentuk yang reference tool terima (False-rejection = bug mivon),
/// tapi mivon juga tidak mengklaim bentuk-bentuk ini sah SV. Nilai fungsi
/// yang dibuang diabaikan lengan runtime tanpa panic.
const NOL_ARG_PERMISSIVE: &[&str] = &[
    "$random;",
    "$urandom;",
    "$urandom_range;",
    "$stime;",
    "$time;",
    "$realtime;",
    "$fdisplay;",
    "$fmonitor;",
    "$fstrobe;",
    "$ferror;",
    "$feof;",
    "$fgetc;",
    "$ungetc;",
    "$rewind;",
    "$fseek;",
    "$ftell;",
];

/// REGRESI: tiap system task nol-argumen yang PASTI sah IEEE harus parse
/// (dulu E1002 "expected LParen, found Semi").
#[test]
fn syscall_nol_args_without_parens_parses() {
    for stmt in NOL_ARG_OK {
        if let Err(e) = parse_sys_stmt(stmt) {
            panic!("`{stmt}` harus sah (BNF system_task_call; iverilog -g2012 terima) tapi: {e}");
        }
    }
}

/// REGRESI: bentuk nol-argumen yang `iverilog -g2012` juga terima harus
/// tidak ditolak mivon (false-rejection = bug mivon). Termasuk system
/// *function* yang nilainya dibuang, dan file-I/O task yangargumennya
/// sebenarnya wajib — mivon sengaja permisif di sini.
#[test]
fn syscall_nol_args_permissive_forms_also_parse() {
    for stmt in NOL_ARG_PERMISSIVE {
        if let Err(e) = parse_sys_stmt(stmt) {
            panic!("`{stmt}` diterima iverilog -g2012 — mivon tak boleh menolak: {e}");
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

/// REGRESI: statement nol-argumen harus bertahan sampai elaborator, bukan
/// cuma lolos parse — lengan runtime menerima `SysCall` dengan 0 argumen
/// tanpa panic.
#[test]
fn syscall_nol_args_survives_elaboration() {
    let src = "module t; initial begin $display; $fflush; $monitor; $dumpvars; end endmodule\n";
    compile_str_quiet(src).unwrap_or_else(|e| panic!("nol-arg syscall harus compile: {e}"));
}

/// REGRESI: `$time;`/`$realtime;` sebagai statement tak boleh memunculkan
/// warning RT9003 "unknown system call". Parser sudah menerima bentuk ini
/// sejak F81, tapi lengan runtime (`evaluate_lang_syscall`) tak punya
/// `"time"`/`"realtime"` — jadi bentuk yang SAH itu menghasilkan warning
/// palsu. Source legal tak boleh diwarnai.
#[test]
fn syscall_time_as_statement_runs_without_unknown_call_warning() {
    let src = "module t; time x; initial begin #1; x = $time; $time; $realtime; end endmodule\n";
    let sigs = simulate_signals(src, 10).expect("sim harus sukses");
    // `$time` sebagai ekspresi tetap berfungsi (nilai waktu simulasi).
    let t = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .expect("signal x harus ada");
    assert!(t >= 1, "x = $time harus >= 1 (waktu sim), dapat {t}");
}

/// REGRESI balik di jalur elaborasi: `$display "hi";` (argumen tanpa kurung)
/// TIDAK SAH — parser sudah menolak (lihat
/// `syscall_args_without_parens_still_rejected`), tapi di sini kita kunci
/// juga bahwa-compile tidak bisa dilewatkan oleh jalur lain (mis. recovery).
#[test]
fn syscall_args_without_parens_rejected_at_compile() {
    let src = "module t; initial begin $display \"hi\"; end endmodule\n";
    assert!(
        compile_str_quiet(src).is_err(),
        "`$display \"hi\";` harus ditolak di jalur compile juga"
    );
}