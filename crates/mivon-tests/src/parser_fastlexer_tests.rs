//! PARSER-08 re-verifikasi: FastLexer byte-level (mivon-compiler) sudah
//! terintegrasi dengan Parser baru di jalur `CompileSession` (jalur yang
//! dipakai semua tool CLI), bukan hanya tokenizer terpisah.
//!
//! Test membandingkan hasil AST jalur session dengan jalur legacy
//! (Preprocessor + Lexer + Parser) dan memastikan kedua mode lexer
//! (`use_fast_lexer` true/false) menghasilkan struktur yang sama.

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

fn session_design(files: &[(&str, &str)], tag: &str, fast: bool) -> Design {
    let dir = std::env::temp_dir().join(format!("mivon_parser08_{tag}_{fast}"));
    let _ = std::fs::remove_dir_all(&dir);
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
    design
}

/// Sidik jari struktur: nama module + port (nama, arah, range).
fn signature(d: &Design) -> Vec<(String, Vec<(String, String, Option<(usize, usize)>)>)> {
    let mut out: Vec<_> = d
        .modules
        .iter()
        .map(|m| {
            let ports = m
                .ports
                .iter()
                .map(|p| {
                    let dir = match p.direction {
                        PortDirection::Input => "input",
                        PortDirection::Output => "output",
                        PortDirection::Inout => "inout",
                        PortDirection::Ref => "ref",
                    };
                    (
                        p.name.to_string(),
                        dir.to_string(),
                        p.range.as_ref().map(|r| (r.msb, r.lsb)),
                    )
                })
                .collect();
            (m.name.to_string(), ports)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
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
    let session = signature(&session_design(&[("design.sv", SRC)], "single", true));
    assert_eq!(session, legacy, "jalur FastLexer harus sama dengan legacy");
    assert_eq!(session.len(), 2, "alu + top");
}

#[test]
fn both_lexer_modes_agree() {
    let fast = signature(&session_design(&[("design.sv", SRC)], "mode", true));
    let slow = signature(&session_design(&[("design.sv", SRC)], "mode", false));
    assert_eq!(fast, slow, "use_fast_lexer true/false harus identik");
}

#[test]
fn fast_lexer_handles_multiple_files() {
    let a = "module m_a(input logic x, output logic y);\n  assign y = x;\nendmodule\n";
    let b = "module m_b(input logic x, output logic y);\n  assign y = ~x;\nendmodule\n";
    let design = session_design(&[("a.sv", a), ("b.sv", b)], "multi", true);
    let sig = signature(&design);
    assert_eq!(sig.len(), 2, "dua module dari dua file");
    assert!(sig.iter().any(|(n, _)| n == "m_a"));
    assert!(sig.iter().any(|(n, _)| n == "m_b"));
    // Port tetap lengkap (bukan hanya module pertama).
    for (_, ports) in &sig {
        assert_eq!(ports.len(), 2, "setiap module punya 2 port");
    }
}
