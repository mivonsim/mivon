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
}
