//! Parsing sequence/property SVA (LANG-06): ##N / ##[min:max], operasi
//! temporal (|->, |=>, and/or/intersect, repetition), named property/sequence,
//! dan substitusi argumen sequence.

use super::*;

impl Parser {
    /// LANG-42b: bungkus sequence yang sudah terbentuk dengan implication bila
    /// diikuti |-> atau |=>. Sebelumnya bentuk `a ##1 b |-> c` membuat
    /// expect(RParen) gagal lalu seluruh assert di-rollback TANPA diagnostik.
    pub(super) fn wrap_implication(
        &mut self,
        seq: super::types::Sequence,
    ) -> Result<super::types::Sequence, SimError> {
        if self.peek() == &Token::PipeArrow {
            self.advance();
            let cons = self.parse_sequence_after_consequent()?;
            Ok(super::types::Sequence::Implication(
                Box::new(seq),
                Box::new(cons),
            ))
        } else if self.peek() == &Token::PipeFatArrow {
            self.advance();
            let cons = self.parse_sequence_after_consequent()?;
            Ok(super::types::Sequence::Implication(
                Box::new(seq),
                Box::new(super::types::Sequence::Concat(
                    Box::new(super::types::Sequence::Delay(1)),
                    Box::new(cons),
                )),
            ))
        } else {
            Ok(seq)
        }
    }

    /// Apakah token '(' di posisi sekarang membungkus SEQUENCE (bukan ekspresi
    /// biasa)? Benar bila di dalam kurung berimbang ada operator temporal
    /// (##, |->, |=>). Tanpa ini '(a |-> b)' — bentuk yang dihasilkan semua
    /// makro assertion OpenTitan — gagal di parse_expr lalu assertion dibuang.
    pub(super) fn paren_starts_sequence(&self) -> bool {
        if self.peek() != &Token::LParen {
            return false;
        }
        let mut depth = 0usize;
        let mut i = self.pos.get();
        while i < self.tokens.len() {
            match &self.tokens[i].0 {
                Token::LParen => depth += 1,
                Token::RParen => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                    if depth == 0 {
                        return false;
                    }
                }
                Token::HashHash | Token::PipeArrow | Token::PipeFatArrow if depth >= 1 => {
                    return true
                }
                Token::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

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

    /// Parse consequent sequence setelah |->: [##N] expr (##N expr)*.
    pub(super) fn parse_sequence_after_consequent(
        &mut self,
    ) -> Result<super::types::Sequence, SimError> {
        let mut seq = if self.peek() == &Token::HashHash {
            self.advance(); // consume ##
            let delay = self.parse_sequence_delay()?;
            let expr = self.parse_expr(0)?;
            super::types::Sequence::Concat(
                Box::new(delay),
                Box::new(super::types::Sequence::Expr(expr)),
            )
        } else if self.paren_starts_sequence() {
            self.advance(); // '('
            let inner = self.parse_sequence_expr()?;
            self.expect(Token::RParen)?;
            inner
        } else {
            super::types::Sequence::Expr(self.parse_expr(0)?)
        };
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

    /// LANG-42: parse body sequence/property SVA (IEEE 1800-2017 §16.9):
    /// [##N] expr (##N expr)* lalu implication opsional (|-> / |=>).
    pub(crate) fn parse_sequence_expr(&mut self) -> Result<super::types::Sequence, SimError> {
        let mut seq = if self.peek() == &Token::HashHash {
            self.advance();
            let delay = self.parse_sequence_delay()?;
            let first = self.parse_expr(0)?;
            super::types::Sequence::Concat(
                Box::new(delay),
                Box::new(super::types::Sequence::Expr(first)),
            )
        } else if self.paren_starts_sequence() {
            self.advance(); // '('
            let inner = self.parse_sequence_expr()?;
            self.expect(Token::RParen)?;
            inner
        } else {
            super::types::Sequence::Expr(self.parse_expr(0)?)
        };
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
        // LANG-42b: implication setelah sequence lengkap (a ##1 b |-> c).
        self.wrap_implication(seq)
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
                if self.peek() == &Token::Not {
                    self.advance();
                    let sig = self.expect_ident()?;
                    Some(ClockEvent::Negedge(sig))
                } else {
                    let sig = self.expect_ident()?;
                    Some(ClockEvent::Posedge(sig))
                }
            } else if self.peek() == &Token::NegEdge {
                self.advance();
                if self.peek() == &Token::Not {
                    self.advance();
                    let sig = self.expect_ident()?;
                    Some(ClockEvent::Posedge(sig))
                } else {
                    let sig = self.expect_ident()?;
                    Some(ClockEvent::Negedge(sig))
                }
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

    /// LANG-42b: assertion a ##1 b |-> c harus jadi Implication ber-antecedent
    /// Concat, bukan di-rollback diam-diam (0 item, 0 diagnostik).
    #[test]
    fn parses_delay_sequence_with_implication() {
        let src = "module m(input logic clk, a, b, c);\n  assert property (@(posedge clk) a ##1 b |-> c);\nendmodule\n";
        let design = parse_src(src);
        let ante = first_antecedent(&design);
        assert!(
            matches!(ante, crate::types::Sequence::Concat(_, _)),
            "antecedent harus Concat, dapat {:?}",
            ante
        );
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(ids.iter().any(|n| n == "a") && ids.iter().any(|n| n == "b"));
    }
    /// Bentuk sequence dimulai delay: ##1 a |-> b.
    /// Bentuk sequence dimulai delay: .
    #[test]
    fn parses_leading_delay_with_implication() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(posedge clk) ##1 a |-> b);\nendmodule\n";
        let design = parse_src(src);
        let ante = first_antecedent(&design);
        assert!(
            matches!(ante, crate::types::Sequence::Concat(_, _)),
            "antecedent harus Concat, dapat {:?}",
            ante
        );
    }
    /// Alias + concat + implication: s1 ##1 s2 |-> c harus resolve keduanya.
    /// Alias + concat + implication:  harus resolve keduanya.
    #[test]
    fn parses_alias_concat_with_implication() {
        let src = "module m(input logic clk, a, b, c);\n  sequence s1; a; endsequence\n  sequence s2; b; endsequence\n  assert property (@(posedge clk) s1 ##1 s2 |-> c);\nendmodule\n";
        let design = parse_src(src);
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(!ids.iter().any(|n| n == "s1"), "s1 harus hilang: {:?}", ids);
        assert!(!ids.iter().any(|n| n == "s2"), "s2 harus hilang: {:?}", ids);
        assert!(ids.iter().any(|n| n == "a") && ids.iter().any(|n| n == "b"));
    }

    /// Consequent boleh berupa rantai delay: a ##1 b |-> c ##1 d.
    #[test]
    fn parses_consequent_with_delay_chain() {
        let src = "module m(input logic clk, a, b, c, d);\n  assert property (@(posedge clk) a ##1 b |-> c ##1 d);\nendmodule\n";
        let design = parse_src(src);
        let mut cons = None;
        for item in &design.modules[0].items {
            if let crate::types::ModuleItem::PropertyAssert(s) = item {
                if let crate::Stmt::PropertySeq {
                    sequence: crate::types::Sequence::Implication(_, c),
                    ..
                } = &**s
                {
                    cons = Some((**c).clone());
                }
            }
        }
        let cons = cons.expect("assertion harus ter-parse");
        let mut ids = Vec::new();
        seq_idents(&cons, &mut ids);
        assert!(ids.iter().any(|n| n == "c") && ids.iter().any(|n| n == "d"));
    }

    /// Bentuk ringkas property p1 = a ##1 b |-> c; tidak boleh terpotong.
    #[test]
    fn parses_shorthand_property_with_implication() {
        let src = "module m(input logic clk, a, b, c);\n  property p1 = a ##1 b |-> c;\n  assert property (@(posedge clk) p1);\nendmodule\n";
        let design = parse_src(src);
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(ids.iter().any(|n| n == "a") && ids.iter().any(|n| n == "b"));
    }

    /// Named property ber-antecedent delay: property p1; a ##1 b |-> c; endproperty.
    #[test]
    fn parses_named_property_with_delay_antecedent() {
        let src = "module m(input logic clk, a, b, c);\n  property p1; a ##1 b |-> c; endproperty\n  assert property (@(posedge clk) p1);\nendmodule\n";
        let design = parse_src(src);
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(ids.iter().any(|n| n == "a") && ids.iter().any(|n| n == "b"));
    }

    /// Parse dan kembalikan Design + diagnostik parser (warning ikut).

    fn first_clock_event(design: &crate::types::Design) -> Option<crate::types::ClockEvent> {
        for item in &design.modules[0].items {
            match item {
                crate::types::ModuleItem::PropertyAssert(s) => {
                    if let crate::Stmt::PropertySeq { clock_event, .. } = &**s {
                        return clock_event.clone();
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn parse_with_diags(
        src: &str,
    ) -> (
        crate::types::Design,
        Vec<mivon_core::diagnostics::diagnostic::Diagnostic>,
    ) {
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
        let design = p.parse_design().expect("parse gagal");
        let diags = std::mem::take(&mut p.errors);
        (design, diags)
    }

    /// Assertion dengan konstruksi tak didukung (repetition `[*2]`) harus
    /// DILAPORKAN (E1007) dan di-skip — bukan hilang diam-diam.
    #[test]
    fn unsupported_assertion_is_reported_not_silently_dropped() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(posedge clk) a[*2] |-> b);\n  logic x;\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert_eq!(design.modules.len(), 1);
        // Assertion tidak boleh muncul sebagai PropertyAssert setengah jadi.
        assert!(
            !design.modules[0]
                .items
                .iter()
                .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_))),
            "assertion tak didukung seharusnya di-skip, bukan di-elaborasi"
        );
        // ... tapi user harus tahu.
        let warn = diags
            .iter()
            .find(|d| d.code == DiagCode::UnsupportedAssertion)
            .unwrap_or_else(|| panic!("warning E1007 hilang; diags = {:?}", diags));
        assert_eq!(warn.code.as_str(), "E1007");
        assert_eq!(
            warn.level,
            mivon_core::diagnostics::diagnostic::DiagLevel::Warning
        );
        // Posisi harus menunjuk keyword `assert` (baris 2), bukan 0:0.
        assert_eq!(warn.spans.first().map(|s| s.start), Some(2));
        // Item sesudah assertion harus tetap ter-parse (skip tidak menelan sisa
        // modul). `logic x;` masuk ke `decls`, bukan `items`.
        assert!(
            design.modules[0]
                .decls
                .iter()
                .any(|d| d.names.iter().any(|n| n.name.as_str() == "x")),
            "deklarasi x setelah assertion hilang: {:?}",
            design.modules[0].decls
        );
    }

    /// Assertion yang DIDUKUNG tidak boleh memunculkan E1007 (tidak ada
    /// false-positive warning).
    #[test]
    fn supported_assertion_emits_no_unsupported_warning() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(posedge clk) a ##1 b |-> b);\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert_eq!(design.modules.len(), 1);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "assertion didukung tidak boleh di-warn E1007: {:?}",
            diags
        );
        assert!(
            design.modules[0]
                .items
                .iter()
                .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_))),
            "assertion didukung harus jadi PropertyAssert"
        );
    }

    /// Property yang dibungkus kurung (a |-> b) — bentuk yang dihasilkan
    /// makro assertion (prim_assert ASSERT(name, clk, rst, prop)) — harus
    /// diparse sebagai sequence, bukan dibuang dengan E1007.
    #[test]
    fn parses_parenthesized_implication() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(posedge clk) (a |-> b));\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "bentuk ber-kurung tidak boleh di-warn E1007: {:?}",
            diags
        );
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(
            ids.iter().any(|n| n == "a"),
            "antecedent a hilang: {:?}",
            ids
        );
    }

    /// Bentuk lengkap makro OpenTitan: disable iff (...) + properti ber-kurung.
    #[test]
    fn parses_disable_iff_with_parenthesized_property() {
        let src = "module m(input logic clk, rst, a, b);\n  assert property (@(posedge clk) disable iff ((rst) !== 1'b0) (a |-> b));\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "disable iff + kurung tidak boleh di-warn E1007: {:?}",
            diags
        );
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(
            ids.iter().any(|n| n == "a"),
            "antecedent a hilang: {:?}",
            ids
        );
    }

    /// Antecedent ber-kurung yang berisi concat: ((a ##1 b) |-> c).
    #[test]
    fn parses_parenthesized_concat_antecedent() {
        let src = "module m(input logic clk, a, b, c);\n  assert property (@(posedge clk) ((a ##1 b) |-> c));\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "antecedent ber-kurung tidak boleh di-warn E1007: {:?}",
            diags
        );
        let ante = first_antecedent(&design);
        let mut ids = Vec::new();
        seq_idents(&ante, &mut ids);
        assert!(
            ids.iter().any(|n| n == "a") && ids.iter().any(|n| n == "b"),
            "antecedent harus a dan b: {:?}",
            ids
        );
    }

    /// Kurung yang TIDAK berisi operator temporal tetap ekspresi biasa.
    #[test]
    fn parenthesized_boolean_property_stays_expression() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(posedge clk) (a && b));\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "ekspresi ber-kurung tidak boleh di-warn E1007: {:?}",
            diags
        );
        assert!(
            design.modules[0]
                .items
                .iter()
                .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_))),
            "assertion boolean ber-kurung harus tetap ter-parse"
        );
    }

    /// Assertion BERLABEL yang tak didukung harus tetap di-warn E1007.
    #[test]
    fn labeled_unsupported_assertion_is_reported() {
        let src = "module m(input logic clk, a, b);\n  chk: assert property (@(posedge clk) a[*2] |-> b);\n  logic x;\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        let warn = diags
            .iter()
            .find(|d| d.code == DiagCode::UnsupportedAssertion)
            .unwrap_or_else(|| panic!("E1007 hilang utk assertion berlabel; diags = {:?}", diags));
        // Span harus menunjuk keyword assert (baris 2), bukan label.
        assert_eq!(warn.spans.first().map(|s| s.start), Some(2));
        assert!(
            design.modules[0]
                .decls
                .iter()
                .any(|d| d.names.iter().any(|n| n.name.as_str() == "x")),
            "deklarasi x setelah assertion berlabel hilang"
        );
    }

    /// Assertion berlabel di dalam GENERATE block dulu ditelan
    /// skip_until_semi_or_end tanpa diagnostik sama sekali.
    #[test]
    fn labeled_unsupported_assertion_in_generate_block_is_reported() {
        let src = "module m(input logic clk, a, b);\n  if (1) begin : g\n    chk: assert property (@(posedge clk) a[*2] |-> b);\n  end\n  logic x;\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        let n = diags
            .iter()
            .filter(|d| d.code == DiagCode::UnsupportedAssertion)
            .count();
        assert_eq!(
            n, 1,
            "harus tepat 1 E1007 (bukan dobel); diags = {:?}",
            diags
        );
        assert_eq!(design.modules.len(), 1);
        // skip_assert_item tidak boleh menelan 'end' generate / item sesudahnya.
        assert!(
            design.modules[0]
                .decls
                .iter()
                .any(|d| d.names.iter().any(|n| n.name.as_str() == "x")),
            "deklarasi x setelah generate block hilang: {:?}",
            design.modules[0].decls
        );
    }

    /// label: property ... adalah DEKLARASI, bukan assertion — tidak boleh
    /// di-warn E1007 dan deklarasinya harus tetap terdaftar.
    #[test]
    fn labeled_property_declaration_is_not_warned() {
        let src = "module m(input logic clk, a, b);\n  p1: property p1; a |-> b; endproperty\n  assert property (@(posedge clk) p1);\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "deklarasi property berlabel tidak boleh di-warn E1007: {:?}",
            diags
        );
        assert!(
            design.modules[0]
                .items
                .iter()
                .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_))),
            "assert property p1 harus ter-parse"
        );
    }

    /// 'label: restrict property' juga assertion — dulu senyap total karena
    /// gate jalur berlabel tidak memuat Token::Restrict.
    #[test]
    fn labeled_restrict_property_is_reported() {
        let src = "module m(input logic clk, a);\n  chk: restrict property (@(posedge clk) a[*2]);\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .any(|d| d.code == DiagCode::UnsupportedAssertion),
            "E1007 hilang utk 'label: restrict property'; diags = {:?}",
            diags
        );
        assert_eq!(design.modules.len(), 1);
    }

    /// E1007 tidak boleh membawa fix-it ';' (dulu dipasang karena pesan
    /// warning menyalin alasan parse asli yang memuat "expected ';'").
    #[test]
    fn unsupported_assertion_has_no_semicolon_fixit() {
        // Jalur snippet hanya aktif bila source_lines diisi (jalur produksi).
        let src = "module m;\nendmodule\n";
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lexer.next_token();
            if tok == Token::Eof {
                break;
            }
            tokens.push((tok, line, col));
        }
        let mut p = Parser::new(tokens, "t.sv").with_source_lines(src);
        p.push_warning_code_at(
            DiagCode::UnsupportedAssertion,
            "assertion skipped, not simulated: expected ';'",
            2,
            3,
        );
        let warn = p.errors.last().expect("warning harus tercatat");
        assert_eq!(warn.code, DiagCode::UnsupportedAssertion);
        assert!(
            warn.fix_its.is_empty(),
            "E1007 tidak boleh punya fix-it: {:?}",
            warn.fix_its
        );
    }

    /// Clock event @(posedge !clk) (negedge pada sinyal terinversi) harus
    /// diparse sebagai Negedge, bukan memicu E1007.
    #[test]
    fn parses_negative_edge_clock_with_bang() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(posedge !clk) a |-> b);\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "posedge !clk tidak boleh di-warn E1007: {:?}",
            diags
        );
        assert!(
            design.modules[0]
                .items
                .iter()
                .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_))),
            "assertion harus tetap ter-parse"
        );
        let clk = first_clock_event(&design).expect("clock harus ada");
        assert!(
            matches!(clk, crate::types::ClockEvent::Negedge(s) if s.as_str() == "clk"),
            "posedge !clk harus Negedge(clk), dapat {:?}",
            clk
        );
    }

    /// Bentuk @(negedge !clk) juga harus diterima (jadi Posedge).
    #[test]
    fn parses_negedge_bang_clock() {
        let src = "module m(input logic clk, a, b);\n  assert property (@(negedge !clk) a |-> b);\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "negedge !clk tidak boleh di-warn E1007: {:?}",
            diags
        );
        assert!(
            design.modules[0]
                .items
                .iter()
                .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_))),
            "assertion harus tetap ter-parse"
        );
        let clk = first_clock_event(&design).expect("clock harus ada");
        assert!(
            matches!(clk, crate::types::ClockEvent::Posedge(s) if s.as_str() == "clk"),
            "negedge !clk harus Posedge(clk), dapat {:?}",
            clk
        );
    }

    /// 'assert always (x) @(posedge !clk)' (PSL, site ke-3 di stmt.rs) juga
    /// harus menerima clock terinversi -> Negedge(clk), tanpa E1007.
    #[test]
    fn parses_psl_always_with_negated_clock() {
        let src = "module m(input logic clk, a);
  assert always (a) @(posedge !clk);
endmodule
";
        let (design, diags) = parse_with_diags(src);
        assert!(
            diags
                .iter()
                .all(|d| d.code != DiagCode::UnsupportedAssertion),
            "assert always @(posedge !clk) tidak boleh di-warn E1007: {:?}",
            diags
        );
        // Site ke-3 (PSL assert always) kini menerima clock terinversi -> salah
        // satu bentuk assertion VALID tidak lagi dibuang (di-wrap sebagai
        // PropertyAssert berisi Stmt::Assert).
        assert!(
            design.modules[0]
                .items
                .iter()
                .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_))),
            "assert always harus menghasilkan item, bukan di-skip"
        );
    }

    /// PARSER-09 MAJOR 3(a): posisi E1007 (rollback assertion) harus memetakan
    /// ke baris FILE ASLI lewat `line directive (file_line_map), bukan baris
    /// kumulatif source gabungan hasil ekspansi makro lintas file.
    #[test]
    fn unsupported_assertion_position_uses_file_line_map() {
        // Source gabungan dengan `line 500 "real.sv": baris kumulatif 1 =
        // file-line 499, baris 2 = 500. Map menyatakan directive di posisi
        // kumulatif 1 bernilai 500.
        let src = "`line 500 \"real.sv\"\nassert property (@(posedge clk) a |-> b);\n";
        let map = vec![(1usize, 500usize, "real.sv".to_string())];
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lexer.next_token();
            if tok == Token::Eof {
                break;
            }
            tokens.push((tok, line, col));
        }
        let mut p = Parser::new(tokens, "real.sv")
            .with_source_lines(src)
            .with_file_line_map(map);
        // Simulasikan rollback assertion di baris kumulatif 2 (token `assert`).
        p.push_warning_code_at(
            DiagCode::UnsupportedAssertion,
            "assertion skipped, not simulated: unsupported construct",
            2,
            1,
        );
        let warn = p.errors.last().expect("warning harus tercatat");
        let span = warn.spans.first().expect("span harus ada");
        assert_eq!(span.file.as_str(), "real.sv", "file harus real.sv");
        assert_eq!(
            span.start as usize, 500,
            "baris harus 500 (file-relative), bukan baris kumulatif"
        );
        if let Some(snip) = &warn.source_snippet {
            assert_eq!(snip.file, "real.sv");
            assert_eq!(snip.line, 500);
        }
    }

    /// ROUND 179: 'expect property' di level module TIDAK boleh hilang
    /// senyap (dulu ditelan skip_until_semi_or_end tanpa diag). Invariant:
    /// entah ter-parse jadi PropertyAssert, entah memicu E1007 — dan parsing
    /// item setelahnya harus tetap utuh.
    #[test]
    fn module_level_expect_not_silently_dropped() {
        let src = "module m(input logic clk, a, b);\n  expect property (@(posedge clk) a |-> b);\n  logic x;\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        let parsed = design.modules[0]
            .items
            .iter()
            .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_)));
        let warned = diags
            .iter()
            .any(|d| d.code == DiagCode::UnsupportedAssertion);
        assert!(
            parsed || warned,
            "expect level module hilang senyap: items={:?} diags={:?}",
            design.modules[0].items.len(),
            diags.len()
        );
        assert!(
            design.modules[0]
                .decls
                .iter()
                .any(|d| d.names.iter().any(|n| n.name.as_str() == "x")),
            "deklarasi setelah expect harus tetap ter-parse"
        );
    }

    /// Varian berlabel: label: expect property(...) juga tidak boleh senyap
    /// (gate arm berlabel kini memuat Token::Expect).
    #[test]
    fn labeled_expect_not_silently_dropped() {
        let src = "module m(input logic clk, a, b);\n  chk: expect property (@(posedge clk) a |-> b);\n  logic x;\nendmodule\n";
        let (design, diags) = parse_with_diags(src);
        let parsed = design.modules[0]
            .items
            .iter()
            .any(|i| matches!(i, crate::types::ModuleItem::PropertyAssert(_)));
        let warned = diags
            .iter()
            .any(|d| d.code == DiagCode::UnsupportedAssertion);
        assert!(
            parsed || warned,
            "label: expect hilang senyap: items={:?} diags={:?}",
            design.modules[0].items.len(),
            diags.len()
        );
        assert!(
            design.modules[0]
                .decls
                .iter()
                .any(|d| d.names.iter().any(|n| n.name.as_str() == "x")),
            "deklarasi setelah label: expect harus tetap ter-parse"
        );
    }
}
