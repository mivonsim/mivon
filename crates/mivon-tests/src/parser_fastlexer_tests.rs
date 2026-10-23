//! PARSER-08 re-verifikasi: FastLexer byte-level (mivon-compiler) sudah
//! terintegrasi dengan Parser baru di jalur `CompileSession` (jalur yang
//! dipakai semua tool CLI), bukan hanya tokenizer terpisah.
//!
//! Bukti yang diuji:
//! - `fast_lexer_files` > 0 hanya pada mode fast (counter di CompileSession),
//! - tidak ada diagnostik error hasil parse,
//! - struktur AST (module, parameter, port, package typedef) sama dengan
//!   jalur legacy (Preprocessor + Lexer + Parser).

use mivon_ast::{Design, PortDirection};
use mivon_compiler::frontend::compile_session::{CompileSession, SessionConfig};

/// Parse jalur legacy: preprocessor + Lexer + Parser (tanpa CompileSession).
fn legacy_design(src: &str) -> Design {
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let pre = pp.preprocess(src, None).expect("preprocess");
    let mut lexer = mivon_parser::lexer::Lexer::new(&pre);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "legacy.sv")
        .with_source_lines(&pre)
        .with_file_line_map(lexer.file_line_map.clone());
    parser.parse_design().expect("legacy parse")
}

/// Hasil compile session: design, jumlah file lewat FastLexer, error parse.
struct SessionOut {
    design: Design,
    fast_files: usize,
    error_count: usize,
}

fn session_design(files: &[(&str, &str)], tag: &str, fast: bool) -> SessionOut {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let uniq = format!(
        "{}_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
        tag
    );
    let dir = std::env::temp_dir().join(format!("mivon_parser08_{uniq}"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut sources = Vec::new();
    for (name, src) in files {
        let p = dir.join(name);
        std::fs::write(&p, src).unwrap();
        sources.push(p);
    }
    let cfg = SessionConfig {
        sources,
        use_fast_lexer: fast,
        ..Default::default()
    };
    let mut session = CompileSession::new(cfg);
    let (design, _index) = session.compile().expect("session compile");
    let fast_files = session
        .fast_lexer_files
        .load(std::sync::atomic::Ordering::Relaxed);
    let error_count = session
        .parse_errors
        .iter()
        .filter(|d| d.level == mivon_core::diagnostics::DiagLevel::Error)
        .count();
    let _ = std::fs::remove_dir_all(&dir);
    SessionOut {
        design,
        fast_files,
        error_count,
    }
}

/// Sidik jari struktur: module (parameter, port, jumlah decl/item) + package.
fn signature(d: &Design) -> Vec<String> {
    let mut out: Vec<String> = d
        .modules
        .iter()
        .map(|m| {
            let params: Vec<String> = m
                .params
                .iter()
                .map(|p| format!("{}={:?}", p.name, p.default))
                .collect();
            // Lebar di-resolve dari parameter module: membuktikan `[W-1:0]`
            // menghasilkan 8 bit di kedua jalur (posisi source tidak ikut
            // dibandingkan karena base offset file berbeda).
            let mut param_vals = std::collections::HashMap::new();
            for p in &m.params {
                if let Some(expr) = &p.default {
                    if let Ok(v) = mivon_ast::const_eval_with_params(expr, &param_vals) {
                        param_vals.insert(p.name, v);
                    }
                }
            }
            let ports: Vec<String> = m
                .ports
                .iter()
                .map(|p| {
                    let dir = match p.direction {
                        PortDirection::Input => "input",
                        PortDirection::Output => "output",
                        PortDirection::Inout => "inout",
                        PortDirection::Ref => "ref",
                    };
                    let width = p.resolved_width(&param_vals).ok();
                    format!(
                        "{}:{}:{:?}:{:?}:{:?}",
                        p.name, dir, width, p.range, p.dtype_name
                    )
                })
                .collect();
            format!(
                "module {} params=[{}] ports=[{}] decls={} items={}",
                m.name,
                params.join(","),
                ports.join(","),
                m.decls.len(),
                m.items.len()
            )
        })
        .collect();
    out.extend(d.packages.iter().map(|p| {
        format!("package {} items={}", p.name, p.items.len())
    }));
    out.sort();
    out
}

const SRC: &str = r#"
package pkg;
  typedef enum logic [1:0] { A, B } state_t;
endpackage

module alu #(parameter W = 8) (
    input  logic         clk,
    input  logic [W-1:0] a,
    input  logic [W-1:0] b,
    output logic [W-1:0] y
);
  assign y = a + b;
endmodule

module top (input logic clk, output logic [7:0] q);
  logic [7:0] y;
  alu u_alu (.clk(clk), .a(8'd1), .b(8'd2), .y(y));
  assign q = y;
endmodule
"#;

#[test]
fn fast_lexer_session_matches_legacy() {
    let legacy = signature(&legacy_design(SRC));
    let out = session_design(&[("design.sv", SRC)], "single", true);
    assert_eq!(out.error_count, 0, "parse harus tanpa error");
    assert!(out.fast_files > 0, "cabang FastLexer harus dieksekusi");
    assert_eq!(signature(&out.design), legacy, "AST jalur FastLexer = legacy");
}

#[test]
fn slow_mode_uses_legacy_lexer() {
    let out = session_design(&[("design.sv", SRC)], "slow", false);
    assert_eq!(out.error_count, 0);
    assert_eq!(out.fast_files, 0, "mode slow tidak boleh memakai FastLexer");
}

#[test]
fn both_lexer_modes_agree() {
    let fast = session_design(&[("design.sv", SRC)], "agree", true);
    let slow = session_design(&[("design.sv", SRC)], "agree", false);
    assert!(fast.fast_files > 0);
    assert_eq!(signature(&fast.design), signature(&slow.design));
}

#[test]
fn fast_lexer_handles_multiple_files() {
    let a = "module m_a(input logic x, output logic y);\n  assign y = x;\nendmodule\n";
    let b = "module m_b(input logic x, output logic y);\n  assign y = ~x;\nendmodule\n";
    let out = session_design(&[("a.sv", a), ("b.sv", b)], "multi", true);
    assert_eq!(out.error_count, 0);
    assert_eq!(out.fast_files, 2, "kedua file lewat FastLexer");
    let sig = signature(&out.design);
    assert_eq!(sig.len(), 2, "dua module dari dua file");
    assert!(sig.iter().any(|s| s.contains("module m_a")));
    assert!(sig.iter().any(|s| s.contains("module m_b")));
}
