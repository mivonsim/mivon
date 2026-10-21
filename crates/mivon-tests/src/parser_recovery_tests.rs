//! PERF-19: parser recovery — anti-flood regression (audit: "recovery mode
//! retries same tokens → cascading errors slow parsing").
//!
//! Verifikasi empirik: recovery parser sudah TIDAK meretry token sama.
//! Tiap kali `parse_module_item`/`parse_stmt` gagal, recovery melakukan
//! skip-to-sync-token (`skip_until_semi_or_end` / `skip_to_stmt_boundary`)
//! + guard kemajuan (`if pos == before { advance() }` — lihat komentar
//! flood 71 ribu E1002 di stmt.rs). Bukti: 11 input malformed → 0 duplikat
//! (line,col), observasi maks 2 error per file (bound kasar +1 guard;
//! audit PERF-19, ROUND 86).
//!
//! Sifat yang dijaga test:
//! 1. Tidak ada error DUPLIKAT di posisi (line,col) sama — flood historis
//!    terjadi saat recovery berhenti tanpa maju → error identik ter-push
//!    tanpa batas.
//! 2. Jumlah error ter-bounds proporsional (≤ jumlah stmt rusak), termasuk
//!    input 500× stmt rusak dalam satu blok.
//! 3. Posisi line/col error akurat (aturan repo: 100% akurat).

use mivon_core::diagnostics::Diagnostic;
use mivon_parser::lexer::{Lexer, Token};
use mivon_parser::Parser;
use std::collections::HashSet;

fn parse_errors(src: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(src);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let file_line_map = lexer.file_line_map.clone();
    let mut parser = Parser::new(tokens, "<test>")
        .with_source_lines(src)
        .with_file_line_map(file_line_map);
    let _ = parser.parse_design();
    parser
        .errors
        .into_iter()
        .filter(|d| d.is_error())
        .collect()
}

/// Posisi diag: parser error mengisi source_snippet (bukan spans);
/// fallback spans (start=line, end=col).
fn diag_pos(d: &Diagnostic) -> (usize, usize) {
    if let Some(s) = d.spans.first() {
        if s.start > 0 {
            return (s.start as usize, s.end as usize);
        }
    }
    d.source_snippet
        .as_ref()
        .map(|s| (s.line, s.col))
        .unwrap_or((0, 0))
}

fn assert_no_duplicate_positions(name: &str, errs: &[Diagnostic]) {
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for e in errs {
        let pos = diag_pos(e);
        assert!(
            seen.insert(pos),
            "[{name}] error DUPLIKAT di L{}:C{} (flood cascade): {:?} {}",
            pos.0,
            pos.1,
            e.code,
            e.message
        );
    }
}

/// Kasus malformed tunggal — error terbatas, tak ada duplikat posisi.
/// Bound = observasi ROUND 86 (recovery yang sehat); melanggar = flood.
/// `min_errors`: sebagian kasus (mis. missing `;`) memang dipulihkan
/// senyap tanpa diagnosa — min = observasi, bukan asumsi "selalu ada error".
#[test]
fn recovery_no_cascade_single_malformed() {
    // (nama, src, min_errors, max_errors) — max = observasi + guard flood.
    let cases: &[(&str, &str, usize, usize)] = &[
        (
            "missing_semi_stmt",
            r#"
module top;
  initial begin
    a = 1
    b = 2
    c = 3
  end
  initial #1 $finish;
endmodule
"#,
            0,
            1,
        ),
        (
            "garbage_in_module_body",
            r#"
module top;
  logic a;
  %%% garbage @@@ here
  logic b;
  assign a = 1;
  initial #1 $finish;
endmodule
"#,
            0,
            1,
        ),
        (
            "unclosed_begin",
            r#"
module top;
  initial begin
    a = 1;
    b = 2;
  initial #1 $finish;
endmodule
"#,
            1,
            2,
        ),
        (
            "bad_always_block",
            r#"
module top;
  logic a, b;
  always_comb begin
    ??? bad token here
    a = b;
  end
  initial #1 $finish;
endmodule
"#,
            1,
            2,
        ),
        (
            "unclosed_case_arm_missing_end",
            r#"
module top;
  logic [1:0] s;
  initial begin
    case (s)
      2'b00: begin
        s = 2'b01;
      2'b01: begin
        s = 2'b10;
    endcase
  end
  initial #1 $finish;
endmodule
"#,
            1,
            2,
        ),
        (
            "garbage_top_level_between_modules",
            r#"
module a;
  logic x;
endmodule
%%% garbage %%%
module b;
  logic y;
endmodule
"#,
            0,
            1,
        ),
        (
            "unclosed_class_body",
            r#"
class bad;
  function void f();
    x = 1;
endclass
module top;
  logic q;
  initial #1 $finish;
endmodule
"#,
            0,
            1,
        ),
        (
            "nested_garbage_fork",
            r#"
module top;
  initial begin
    fork
      @@@ bad
      a = 1;
      begin
        ??? worse
        b = 2;
      end
    join
  end
  initial #1 $finish;
endmodule
"#,
            2,
            3,
        ),
        (
            "missing_endmodule_trailing_items",
            r#"
module top;
  logic a;
  initial begin
    a = 1;
  end
module another;
  logic b;
  assign b = 1;
endmodule
"#,
            0,
            1,
        ),
    ];
    for &(name, src, min_errors, max_errors) in cases {
        let errs = parse_errors(src);
        assert_no_duplicate_positions(name, &errs);
        assert!(
            errs.len() >= min_errors,
            "[{name}] recovery terlalu senyap: {} error (observasi minimal {min_errors})",
            errs.len()
        );
        assert!(
            errs.len() <= max_errors,
            "[{name}] error flood: {} error (bound {max_errors}): {:?}",
            errs.len(),
            errs.iter()
                .map(|e| (diag_pos(e), e.message.to_string()))
                .collect::<Vec<_>>()
        );
    }
}

/// Flood guard: 500 stmt rusak dalam satu blok — recovery harus maju per
/// stmt (maks 1 error/stmt), bukan mengulang token sama (flood historis
/// 71 ribu duplikat E1002 dari DV macro-case base_seq::body).
#[test]
fn recovery_bounded_errors_large_garbage_block() {
    let mut body = String::from("module top;\n  initial begin\n");
    for i in 0..500 {
        body.push_str(&format!("    @@@ bad stmt {i}\n"));
    }
    body.push_str("  end\n  initial #1 $finish;\nendmodule\n");
    let errs = parse_errors(&body);
    assert_no_duplicate_positions("large_garbage", &errs);
    assert!(
        errs.len() <= 500,
        "flood: {} error untuk 500 stmt rusak (bound: 1/stmt)",
        errs.len()
    );
}

/// Akurasi posisi (aturan repo: line/col 100% akurat) — `???` di L5:C5.
#[test]
fn recovery_error_position_accurate() {
    let src = r#"
module top;
  logic a, b;
  always_comb begin
    ??? bad token here
    a = b;
  end
  initial #1 $finish;
endmodule
"#;
    let errs = parse_errors(src);
    assert!(!errs.is_empty(), "harusnya ada minimal 1 error");
    let pos = diag_pos(&errs[0]);
    assert_eq!(
        pos,
        (5, 5),
        "posisi error harus tepat di token rusak (L5:C5), dapat L{}:C{}",
        pos.0,
        pos.1
    );
}

#[test]
fn recovery_valid_source_no_errors() {
    let src = r#"
module top;
  logic [7:0] a, b;
  always_comb begin
    a = b + 1;
  end
  initial begin
    b = 8'h10;
    #1 $finish;
  end
endmodule
"#;
    let errs = parse_errors(src);
    assert!(
        errs.is_empty(),
        "source valid tak boleh menghasilkan error: {:?}",
        errs.iter().map(|e| e.message.to_string()).collect::<Vec<_>>()
    );
}
