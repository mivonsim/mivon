//! Parsing sequence/property SVA (LANG-06): ##N / ##[min:max], operasi
//! temporal (|->, |=>, and/or/intersect, repetition), named property/sequence,
//! dan substitusi argumen sequence.

use super::*;

impl Parser {
    /// LANG-06 (SVA temporal): parse delay sequence setelah `##` — bentuk
    /// `##N` (Delay konstan) atau `##[min:max]` (DelayRange). Caller sudah
    /// meng-consume `##`.
    pub(super) fn parse_sequence_delay(&mut self) -> Result<super::types::Sequence, SimError> {
        if self.peek() == &Token::LBrack {
            // ##[min:max]
            self.advance();
            let min = self.parse_sequence_number()?;
            self.expect(Token::Colon)?;
            let max = self.parse_sequence_number()?;
            self.expect(Token::RBrack)?;
            Ok(super::types::Sequence::DelayRange(min, max))
        } else {
            let n = self.parse_sequence_number()?;
            Ok(super::types::Sequence::Delay(n))
        }
    }

    /// Parse bilangan bulat untuk delay sequence (`##3` → 3).
    pub(super) fn parse_sequence_number(&mut self) -> Result<u64, SimError> {
        match self.peek().clone() {
            Token::Number { value, base, .. } => {
                let s = value.as_str().to_string();
                self.advance();
                match base {
                    None | Some(10) => s
                        .parse::<u64>()
                        .map_err(|_| self.err("invalid sequence delay number")),
                    Some(b) => u64::from_str_radix(&s, b as u32)
                        .map_err(|_| self.err("invalid sequence delay number")),
                }
            }
            _ => Err(self.err("expected number after '##'")),
        }
    }

    /// Parse consequent sequence setelah `|->`. Bisa `##N expr`,
    /// `##[min:max] expr`, atau `expr` (tanpa delay = same-cycle).
    pub(super) fn parse_sequence_after_consequent(
        &mut self,
    ) -> Result<super::types::Sequence, SimError> {
        if self.peek() == &Token::HashHash {
            self.advance(); // consume ##
            let delay = self.parse_sequence_delay()?;
            let expr = self.parse_expr(0)?;
            Ok(super::types::Sequence::Concat(
                Box::new(delay),
                Box::new(super::types::Sequence::Expr(expr)),
            ))
        } else {
            let expr = self.parse_expr(0)?;
            Ok(super::types::Sequence::Expr(expr))
        }
    }

    /// LANG-42: parse body sequence/property SVA (IEEE 1800-2017 §16.9):
    /// `[##N] expr (##N expr)*`, `expr |-> consequent`, atau `expr`.
    pub(crate) fn parse_sequence_expr(&mut self) -> Result<super::types::Sequence, SimError> {
        if self.peek() == &Token::HashHash {
            self.advance();
            let delay = self.parse_sequence_delay()?;
            let first = self.parse_expr(0)?;
            let mut seq = super::types::Sequence::Concat(
                Box::new(delay),
                Box::new(super::types::Sequence::Expr(first)),
            );
            while self.peek() == &Token::HashHash {
                self.advance();
                let delay = self.parse_sequence_delay()?;
                let next = self.parse_expr(0)?;
                seq = super::types::Sequence::Concat(
                    Box::new(seq),
                    Box::new(super::types::Sequence::Concat(
                        Box::new(delay),
                        Box::new(super::types::Sequence::Expr(next)),
                    )),
                );
            }
            return Ok(seq);
        }
        let expr = self.parse_expr(0)?;
        if self.peek() == &Token::PipeArrow {
            self.advance();
            let cons = self.parse_sequence_after_consequent()?;
            return Ok(super::types::Sequence::Implication(
                Box::new(super::types::Sequence::Expr(expr)),
                Box::new(cons),
            ));
        }
        if self.peek() == &Token::PipeFatArrow {
            // `expr |=> consequent` — non-overlap implication (SVA §16.9.2).
            // Encoding sama dengan jalur assert inline: Delay(1) sebagai leader
            // concat di dalam umbrella Implication. Semantik: consequent
            // dievaluasi dari posedge tepat SETELAH antecedent match.
            self.advance();
            let cons = self.parse_sequence_after_consequent()?;
            return Ok(super::types::Sequence::Implication(
                Box::new(super::types::Sequence::Expr(expr)),
                Box::new(super::types::Sequence::Concat(
                    Box::new(super::types::Sequence::Delay(1)),
                    Box::new(cons),
                )),
            ));
        }
        let mut seq = super::types::Sequence::Expr(expr);
        while self.peek() == &Token::HashHash {
            self.advance();
            let delay = self.parse_sequence_delay()?;
            let next = self.parse_expr(0)?;
            seq = super::types::Sequence::Concat(
                Box::new(seq),
                Box::new(super::types::Sequence::Concat(
                    Box::new(delay),
                    Box::new(super::types::Sequence::Expr(next)),
                )),
            );
        }
        Ok(seq)
    }

    /// LANG-42: deklarasi SVA bernama (IEEE 1800-2017 §16.12):
    /// `sequence name [(formals)] [@(clk)] body endsequence` atau bentuk
    /// ringkas `sequence name = body;` (idem untuk `property`).
    pub(crate) fn parse_named_sva_decl(&mut self, is_property: bool) -> Result<(), SimError> {
        self.advance(); // consume 'sequence'/'property'
        let name = self.expect_ident()?;
        let mut formals: Vec<Symbol> = Vec::new();
        if self.peek() == &Token::LParen {
            self.advance();
            while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                match self.peek() {
                    Token::Comma
                    | Token::Input
                    | Token::Output
                    | Token::Inout
                    | Token::Logic
                    | Token::Bit
                    | Token::Reg
                    | Token::Int
                    | Token::Integer
                    | Token::Byte
                    | Token::Shortint
                    | Token::Longint
                    | Token::Wire
                    | Token::LBrack
                    | Token::RBrack
                    | Token::Colon
                    | Token::Signed
                    | Token::Unsigned
                    | Token::Number { .. } => {
                        self.advance();
                    }
                    Token::Ident(s) => {
                        formals.push(*s);
                        self.advance();
                    }
                    _ => {
                        self.advance();
                    }
                }
            }
            self.expect(Token::RParen)?;
        }
        // Bentuk ringkas: `name = expr;` — tidak ada endsequence/endproperty.
        if self.peek() == &Token::BlockingAssign {
            self.advance();
            let body = self.parse_sequence_expr()?;
            self.skip_semi();
            self.register_named_sva(is_property, name, formals, body, None);
            return Ok(());
        }
        self.skip_semi();
        // Optional clocking event di dalam deklarasi: `property p; @(posedge clk) a; endproperty`.
        let decl_clock = if self.peek() == &Token::At {
            self.advance();
            self.expect(Token::LParen)?;
            let ce = if self.peek() == &Token::PosEdge {
                self.advance();
                let sig = self.expect_ident()?;
                Some(ClockEvent::Posedge(sig))
            } else if self.peek() == &Token::NegEdge {
                self.advance();
                let sig = self.expect_ident()?;
                Some(ClockEvent::Negedge(sig))
            } else {
                let sig = self.expect_ident()?;
                Some(ClockEvent::Edge(sig))
            };
            self.expect(Token::RParen)?;
            ce
        } else {
            None
        };
        let body = self.parse_sequence_expr()?;
        self.skip_semi();
        match self.peek() {
            Token::EndSequence => {
                self.advance();
            }
            Token::Ident(n) if n.as_str() == "endproperty" => {
                self.advance();
            }
            _ => {
                return Err(self.err("expected endsequence/endproperty"));
            }
        }
        if self.peek() == &Token::Colon {
            self.advance();
            if matches!(self.peek(), Token::Ident(_)) {
                self.advance();
            }
        }
        self.register_named_sva(is_property, name, formals, body, decl_clock);
        Ok(())
    }

    pub(super) fn register_named_sva(
        &mut self,
        is_property: bool,
        name: Symbol,
        formals: Vec<Symbol>,
        body: super::types::Sequence,
        clock: Option<ClockEvent>,
    ) {
        if is_property {
            self.named_properties.insert(name, (formals, body, clock));
        } else {
            self.named_sequences.insert(name, (formals, body, clock));
        }
    }

    /// LANG-42: substitusi parameter formal sequence/property dengan argumen
    /// aktual (`substitute_let_args` untuk node Expr).
    pub(crate) fn subst_sequence(
        seq: &super::types::Sequence,
        map: &std::collections::HashMap<Symbol, &Expr>,
    ) -> super::types::Sequence {
        use super::types::Sequence;
        match seq {
            Sequence::Expr(e) => {
                Sequence::Expr(mivon_ast::inline_util::substitute_let_args(e.clone(), map))
            }
            Sequence::Delay(n) => Sequence::Delay(*n),
            Sequence::DelayRange(a, b) => Sequence::DelayRange(*a, *b),
            Sequence::Concat(a, b) => Sequence::Concat(
                Box::new(Self::subst_sequence(a, map)),
                Box::new(Self::subst_sequence(b, map)),
            ),
            Sequence::Or(a, b) => Sequence::Or(
                Box::new(Self::subst_sequence(a, map)),
                Box::new(Self::subst_sequence(b, map)),
            ),
            Sequence::And(a, b) => Sequence::And(
                Box::new(Self::subst_sequence(a, map)),
                Box::new(Self::subst_sequence(b, map)),
            ),
            Sequence::Repeat(a, n) => Sequence::Repeat(Box::new(Self::subst_sequence(a, map)), *n),
            Sequence::Implication(a, b) => Sequence::Implication(
                Box::new(Self::subst_sequence(a, map)),
                Box::new(Self::subst_sequence(b, map)),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi pemisahan modul: sequence dengan delay range (`##[1:3]`) harus
    /// tetap ter-parse — jalur ini yang dipindah ke `stmt/sva.rs`.
    #[test]
    fn parses_sequence_with_delay_range() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(posedge clk) a ##[1:3] b);\nendmodule\n";
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lexer.next_token();
            if tok == Token::Eof {
                break;
            }
            tokens.push((tok, line, col));
        }
        let mut p = Parser::new(tokens, "t.sv");
        let design = p.parse_design().expect("SVA delay range harus parse");
        assert_eq!(design.modules.len(), 1);
    }
    #[test]
    fn parses_assert_property_named_seq_and_implication() {
        let src = "module m(input logic clk, a, b, c);
  sequence s1; a ##1 b; endsequence
  assert property (@(posedge clk) s1 |-> c);
endmodule
";
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lexer.next_token();
            if tok == Token::Eof {
                break;
            }
            tokens.push((tok, line, col));
        }
        let mut p = Parser::new(tokens, "t.sv");
        let design = p
            .parse_design()
            .expect("assert property dengan named sequence + implication harus parse");
        assert_eq!(design.modules.len(), 1);
        // Regression lock yang NYATA: pastikan assertion tidak di-skip diam-diam.
        let mut ante_shape = None;
        for item in &design.modules[0].items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq {
                    sequence: crate::types::Sequence::Implication(ante, _),
                    ..
                } = &**s
                {
                    ante_shape = Some((**ante).clone());
                }
            }
        }
        let ante = ante_shape.expect(
            "assert property harus menghasilkan PropertyAssert ber-Sequence::Implication, bukan di-skip",
        );
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(ids.iter().any(|n| n == "a"), "a harus ada: {:?}", ids);
        assert!(ids.iter().any(|n| n == "b"), "b harus ada: {:?}", ids);
        assert!(!ids.iter().any(|n| n == "s1"), "s1 harus hilang: {:?}", ids);
        // LANG-42b: antecedent s1 harus tersubstitusi menjadi body deklarasi
        // (a ##1 b), bukan Ident mentah yang memicu E2001 di elaborator.
        assert!(
            !matches!(
                ante,
                crate::types::Sequence::Expr(crate::Expr::Ident { .. })
            ),
            "antecedent harus tersubstitusi, dapat {:?}",
            ante
        );
    }
    /// Kumpulkan nama Ident di dalam sequence (rekursif) untuk assertion test.
    fn seq_idents(seq: &crate::types::Sequence, out: &mut Vec<String>) {
        use crate::types::Sequence;
        match seq {
            Sequence::Expr(crate::Expr::Ident { name, .. }) => out.push(name.as_str().to_string()),
            Sequence::Concat(a, c)
            | Sequence::Or(a, c)
            | Sequence::And(a, c)
            | Sequence::Implication(a, c) => {
                seq_idents(a, out);
                seq_idents(c, out);
            }
            Sequence::Repeat(s, _) => seq_idents(s, out),
            _ => {}
        }
    }

    /// LANG-42b: named sequence ber-argumen di antecedent implication harus
    /// disubstitusi (formal x/y -> aktual a/b), bukan diteruskan mentah.
    #[test]
    fn parses_named_sequence_with_args_in_antecedent() {
        let src = "module m(input logic clk, a, b, c);\n  sequence s1(x, y); x ##1 y; endsequence\n  assert property (@(posedge clk) s1(a, b) |-> c);\nendmodule\n";
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lexer.next_token();
            if tok == Token::Eof {
                break;
            }
            tokens.push((tok, line, col));
        }
        let mut p = Parser::new(tokens, "t.sv");
        let design = p
            .parse_design()
            .expect("named sequence ber-argumen di antecedent harus parse");
        let mut ante_shape = None;
        for item in &design.modules[0].items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq {
                    sequence: crate::types::Sequence::Implication(ante, _),
                    ..
                } = &**s
                {
                    ante_shape = Some((**ante).clone());
                }
            }
        }
        let ante = ante_shape.expect("harus ada PropertyAssert ber-Implication");
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(
            !ids.iter().any(|n| n == "s1"),
            "nama deklarasi harus hilang: {:?}",
            ids
        );
        assert!(
            ids.iter().any(|n| n == "a"),
            "argumen aktual a harus masuk: {:?}",
            ids
        );
        assert!(
            ids.iter().any(|n| n == "b"),
            "argumen aktual b harus masuk: {:?}",
            ids
        );
    }

    /// Parse source test dan kembalikan Design (panic bila parse gagal).
    fn parse_src(src: &str) -> crate::types::Design {
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lexer.next_token();
            if tok == Token::Eof {
                break;
            }
            tokens.push((tok, line, col));
        }
        let mut p = Parser::new(tokens, "t.sv");
        p.parse_design().expect("parse gagal")
    }

    /// Antecedent implication dari assertion pertama module pertama.
    fn first_antecedent(design: &crate::types::Design) -> crate::types::Sequence {
        for item in &design.modules[0].items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq {
                    sequence: crate::types::Sequence::Implication(ante, _),
                    ..
                } = &**s
                {
                    return (**ante).clone();
                }
            }
        }
        panic!("tidak ada PropertyAssert ber-Implication");
    }

    /// LANG-42b: sequence module A tidak boleh bocor ke module B. Bila bocor,
    /// identifier sinyal ack di module B di-expand menjadi body module A dan
    /// elaborator melaporkan E2001 palsu.
    #[test]
    fn named_sequence_does_not_leak_across_modules() {
        let src = "module A(input logic clk, x);\n  sequence ack; x ##1 x; endsequence\nendmodule\nmodule B(input logic clk, a, ack);\n  assert property (@(posedge clk) a |-> ack);\nendmodule\n";
        let design = parse_src(src);
        let module_b = design
            .modules
            .iter()
            .find(|m| m.name.as_str() == "B")
            .expect("module B harus ada");
        let mut ante = None;
        let mut cons = None;
        for item in &module_b.items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq {
                    sequence: crate::types::Sequence::Implication(a, c),
                    ..
                } = &**s
                {
                    ante = Some((**a).clone());
                    cons = Some((**c).clone());
                }
            }
        }
        let mut ante_ids = Vec::new();
        seq_idents(
            &ante.expect("assertion di module B harus ter-parse"),
            &mut ante_ids,
        );
        assert_eq!(
            ante_ids,
            vec!["a".to_string()],
            "antecedent harus tetap sinyal a"
        );
        let mut cons_ids = Vec::new();
        seq_idents(&cons.expect("consequent harus ada"), &mut cons_ids);
        assert_eq!(
            cons_ids,
            vec!["ack".to_string()],
            "ack di module B adalah sinyal, bukan sequence module A"
        );
    }

    /// LANG-42b: alias bertingkat (s1 -> s2 -> expr) harus tersubstitusi penuh.
    #[test]
    fn transitive_named_sequence_alias_is_resolved() {
        let src = "module m(input logic clk, a, b, c);\n  sequence s2; b; endsequence\n  sequence s1; a ##1 s2; endsequence\n  assert property (@(posedge clk) s1 |-> c);\nendmodule\n";
        let design = parse_src(src);
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(!ids.iter().any(|n| n == "s1"), "s1 harus hilang: {:?}", ids);
        assert!(!ids.iter().any(|n| n == "s2"), "s2 harus hilang: {:?}", ids);
        assert!(ids.iter().any(|n| n == "a"), "a harus ada: {:?}", ids);
        assert!(ids.iter().any(|n| n == "b"), "b harus ada: {:?}", ids);
    }

    /// Guard siklus: sequence yang mereferensikan dirinya sendiri tidak boleh
    /// membuat parser hang.
    #[test]
    fn self_referential_sequence_terminates() {
        let src = "module m(input logic clk, a);\n  sequence s1; s1 ##1 a; endsequence\n  assert property (@(posedge clk) s1 |-> a);\nendmodule\n";
        let design = parse_src(src);
        assert_eq!(design.modules.len(), 1);
        let _ = first_antecedent(&design);
    }

    /// LANG-42b: sequence di dalam program juga tidak boleh bocor ke module.
    #[test]
    fn named_sequence_does_not_leak_from_program() {
        let src = "program P(input logic clk, zz);\n  sequence pl; zz ##1 zz; endsequence\nendprogram\nmodule B(input logic clk, a, pl);\n  assert property (@(posedge clk) a |-> pl);\nendmodule\n";
        let design = parse_src(src);
        let module_b = design
            .modules
            .iter()
            .find(|m| m.name.as_str() == "B")
            .expect("module B harus ada");
        let mut cons_ids = Vec::new();
        for item in &module_b.items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq {
                    sequence: crate::types::Sequence::Implication(_, c),
                    ..
                } = &**s
                {
                    seq_idents(c, &mut cons_ids);
                }
            }
        }
        assert_eq!(
            cons_ids,
            vec!["pl".to_string()],
            "pl di module B adalah sinyal, bukan sequence program P"
        );
    }

    /// LANG-42b: alias lewat property (p1 -> s2 -> expr) harus tersubstitusi.
    #[test]
    fn property_alias_to_named_sequence_is_resolved() {
        let src = "module m(input logic clk, a, b);\n  sequence s2; a ##1 b; endsequence\n  property p1; s2; endproperty\n  assert property (@(posedge clk) p1);\nendmodule\n";
        let design = parse_src(src);
        let mut found = false;
        for item in &design.modules[0].items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq { sequence, .. } = &**s {
                    let mut ids = Vec::new();
                    seq_idents(sequence, &mut ids);
                    assert!(!ids.iter().any(|n| n == "s2"), "s2 harus hilang: {:?}", ids);
                    assert!(ids.iter().any(|n| n == "a"), "a harus ada: {:?}", ids);
                    found = true;
                }
            }
        }
        assert!(found, "assertion p1 harus jadi PropertySeq, bukan di-skip");
    }

    /// LANG-42b: checker di dalam module punya scope SVA sendiri.
    #[test]
    fn named_sequence_does_not_leak_from_checker() {
        let src = "module m(input logic clk, a, cs);\n  checker chk(input x);\n    sequence cs; x ##1 x; endsequence\n  endchecker\n  assert property (@(posedge clk) a |-> cs);\nendmodule\n";
        let design = parse_src(src);
        let mut cons_ids = Vec::new();
        for item in &design.modules[0].items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq {
                    sequence: crate::types::Sequence::Implication(_, c),
                    ..
                } = &**s
                {
                    seq_idents(c, &mut cons_ids);
                }
            }
        }
        assert_eq!(
            cons_ids,
            vec!["cs".to_string()],
            "cs di module adalah sinyal, bukan sequence checker"
        );
    }

    /// Budget ekspansi: alias bercabang 2^n harus berhenti cepat, bukan OOM.
    #[test]
    fn exponential_alias_expansion_is_bounded() {
        let mut src = String::from("module m(input logic clk, a, b);\n");
        src.push_str("  sequence s0; a; endsequence\n");
        for i in 1..=24 {
            src.push_str(&format!(
                "  sequence s{}; s{} ##1 s{}; endsequence\n",
                i,
                i - 1,
                i - 1
            ));
        }
        src.push_str("  assert property (@(posedge clk) s24 |-> b);\nendmodule\n");
        let start = std::time::Instant::now();
        let design = parse_src(&src);
        let elapsed = start.elapsed();
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(
            !ids.is_empty() && ids.len() < 20_000,
            "hasil ekspansi harus terisi dan dibatasi budget, dapat {}",
            ids.len()
        );
        assert!(
            elapsed.as_secs() < 10,
            "ekspansi harus berhenti cepat, butuh {:?}",
            elapsed
        );
    }

    /// Clock dari deklarasi alias harus terbawa bila assertion tanpa clock.
    #[test]
    fn alias_declaration_clock_is_propagated() {
        let src = "module m(input logic clk, a, b);\n  sequence s2; @(posedge clk) a ##1 b; endsequence\n  property p1; s2; endproperty\n  assert property (p1);\nendmodule\n";
        let design = parse_src(src);
        let mut clock_ok = false;
        for item in &design.modules[0].items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq { clock_event, .. } = &**s {
                    clock_ok = matches!(clock_event, Some(crate::types::ClockEvent::Posedge(_)));
                }
            }
        }
        assert!(clock_ok, "clock deklarasi alias harus dipakai assertion");
    }

    /// Rantai alias sangat dalam harus berhenti di batas kedalaman, bukan
    /// menabrak stack (SIGABRT).
    #[test]
    fn deep_alias_chain_does_not_overflow_stack() {
        // 5.000 level: jauh di atas ambang stack overflow (~1.000 pada stack
        // 2 MB) sehingga test ini gagal (SIGABRT) bila depth cap dihapus.
        let mut src = String::from("module m(input logic clk, a, b);\n");
        src.push_str("  sequence c0; a; endsequence\n");
        for i in 1..=5000 {
            src.push_str(&format!("  sequence c{}; c{}; endsequence\n", i, i - 1));
        }
        src.push_str("  assert property (@(posedge clk) c5000 |-> b);\nendmodule\n");
        let start = std::time::Instant::now();
        let design = parse_src(&src);
        let elapsed = start.elapsed();
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        // Cap 256 -> salah satu nama alias tersisa unresolved (bukan hang/crash).
        assert!(
            ids.iter().any(|n| n.starts_with('c')),
            "harus ada alias tersisa yang unresolved: {:?}",
            ids
        );
        assert!(
            elapsed.as_secs() < 10,
            "rantai dalam harus berhenti cepat, butuh {:?}",
            elapsed
        );
    }
}
