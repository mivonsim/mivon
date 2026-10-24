//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan parser.rs (SRP Refactoring).
//! Tanggung jawab: Parsing UDP (User-Defined Primitive) declarations.
//!
//! Fungsi:
//!   - parse_udp_symbol()       — parsing symbol dalam UDP table
//!   - parse_udp_table()        — parsing tabel UDP
//!   - parse_udp_declaration()  — parsing primitive ... endprimitive
//!
//! ──────────────────────────────────────────────────────────────────────────────

use super::Parser;
use crate::lexer::*;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

impl Parser {
    pub(crate) fn parse_udp_symbol(&mut self) -> Result<UdpSymbol, SimError> {
        let tok = self.peek().clone();
        match &tok {
            Token::Number { value, base, .. }
                if *base == Some(2)
                    || *base == Some(10)
                    || *base == Some(16)
                    || *base == Some(8) =>
            {
                // Could be sized like 1'b0, but in table it's just '0', '1', 'x'
                let trimmed = if let Some(b) = base {
                    let prefix = format!("'{}", *b as char);
                    if let Some(idx) = value.as_str().find(&prefix) {
                        value.as_str()[idx + prefix.len()..].to_string()
                    } else {
                        value.as_str().to_string()
                    }
                } else {
                    value.as_str().to_string()
                };
                self.advance();
                match trimmed.as_str() {
                    "0" => Ok(UdpSymbol::Zero),
                    "1" => Ok(UdpSymbol::One),
                    "x" | "X" => Ok(UdpSymbol::X),
                    "?" => Ok(UdpSymbol::DontCare),
                    "-" => Ok(UdpSymbol::NoChange),
                    _ if trimmed.starts_with('(') => {
                        let end = trimmed.find(')').unwrap_or(trimmed.len() - 1);
                        let edge = trimmed[1..end].to_string();
                        Ok(UdpSymbol::Edge(Symbol::intern(&edge)))
                    }
                    _ => Err(self.err_with_code(
                        DiagCode::InvalidSyntax,
                        format!("invalid UDP table symbol '{}'", trimmed),
                    )),
                }
            }
            Token::Number { value, .. } if value == "0" || value == "1" => {
                self.advance();
                match value.as_str() {
                    "0" => Ok(UdpSymbol::Zero),
                    "1" => Ok(UdpSymbol::One),
                    _ => Ok(UdpSymbol::X),
                }
            }
            Token::FillLit(_) => {
                self.advance();
                Ok(UdpSymbol::X)
            }
            Token::Minus => {
                self.advance();
                Ok(UdpSymbol::NoChange)
            }
            Token::Question => {
                self.advance();
                Ok(UdpSymbol::DontCare)
            }
            Token::LParen => {
                // Edge transition: (01), (0x), etc.
                self.advance();
                let mut edge_str = String::new();
                // Read content until )
                while self.peek() != &Token::Eof && self.peek() != &Token::RParen {
                    match self.peek() {
                        Token::Number { value, .. } => {
                            edge_str.push_str(value.as_str());
                            self.advance();
                        }
                        Token::Question => {
                            edge_str.push('?');
                            self.advance();
                        }
                        Token::FillLit(_) => {
                            edge_str.push('x');
                            self.advance();
                        }
                        Token::Ident(s) => {
                            edge_str.push_str(s.as_str());
                            self.advance();
                        }
                        _ => break,
                    }
                }
                self.expect(Token::RParen)?;
                Ok(UdpSymbol::Edge(Symbol::intern(&edge_str)))
            }
            Token::Ident(s) if s == "x" || s == "X" => {
                self.advance();
                Ok(UdpSymbol::X)
            }
            Token::Ident(s) if s == "r" || s == "R" => {
                self.advance();
                Ok(UdpSymbol::Edge(Symbol::intern("01")))
            }
            Token::Ident(s) if s == "f" || s == "F" => {
                self.advance();
                Ok(UdpSymbol::Edge(Symbol::intern("10")))
            }
            Token::Ident(s) if s == "p" || s == "P" => {
                self.advance();
                Ok(UdpSymbol::Edge(Symbol::intern("p")))
            }
            Token::Ident(s) if s == "n" || s == "N" => {
                self.advance();
                Ok(UdpSymbol::Edge(Symbol::intern("n")))
            }
            Token::Ident(s) if s == "*" || s == "Star" => {
                self.advance();
                Ok(UdpSymbol::Edge(Symbol::intern("??")))
            }
            _ => Err(self.err_with_code(
                DiagCode::UnexpectedToken,
                format!("unexpected token in UDP table: {}", tok),
            )),
        }
    }

    pub(crate) fn parse_udp_table(
        &mut self,
        is_sequential: bool,
    ) -> Result<Vec<UdpTableEntry>, SimError> {
        self.expect(Token::Table)?;
        self.skip_semi();

        let mut entries = Vec::new();
        loop {
            if self.peek() == &Token::EndTable {
                self.advance();
                break;
            }
            if self.peek() == &Token::Eof {
                return Err(
                    self.err_with_code(DiagCode::ExpectedToken, "unexpected EOF in UDP table")
                );
            }
            // Parse one line
            let mut inputs = Vec::new();
            loop {
                if self.peek() == &Token::Colon {
                    self.advance();
                    break;
                }
                let sym = self.parse_udp_symbol()?;
                inputs.push(sym);
            }
            if is_sequential {
                // Sequential UDP: inputs : current_state : output ;
                let current_state = self.parse_udp_symbol()?;
                inputs.push(current_state);
                self.expect(Token::Colon)?;
            }
            let output = self.parse_udp_symbol()?;
            self.skip_semi();
            entries.push(UdpTableEntry { inputs, output });
        }
        Ok(entries)
    }

    pub(crate) fn parse_udp_declaration(&mut self) -> Result<UdpDef, SimError> {
        self.expect(Token::Primitive)?;
        // Posisi nama UDP (1-based) untuk diagnostic elaborator.
        let uline = self.peek_line();
        let ucol = self.peek_col();
        let name = self.expect_ident()?;

        // Parse port list: (output [reg] port1, input port2, input port3, ...)
        self.expect(Token::LParen)?;
        let mut ports = Vec::new();
        let mut is_sequential = false;

        loop {
            if self.peek() == &Token::RParen {
                self.advance();
                break;
            }
            let direction = if self.peek() == &Token::Output {
                self.advance();
                // Check for 'output reg name' (sequential UDP)
                if self.peek() == &Token::Reg {
                    self.advance();
                    let name = self.expect_ident()?;
                    ports.push(UdpPort {
                        direction: PortDirection::Output,
                        name,
                        is_reg: true,
                    });
                    is_sequential = true;
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                    continue;
                }
                PortDirection::Output
            } else if self.peek() == &Token::Input {
                self.advance();
                // Check for 'input reg name'
                if self.peek() == &Token::Reg {
                    self.advance();
                }
                let name = self.expect_ident()?;
                ports.push(UdpPort {
                    direction: PortDirection::Input,
                    name,
                    is_reg: false,
                });
                if self.peek() == &Token::Comma {
                    self.advance();
                }
                continue;
            } else if self.peek() == &Token::Inout {
                self.advance();
                let name = self.expect_ident()?;
                ports.push(UdpPort {
                    direction: PortDirection::Inout,
                    name,
                    is_reg: false,
                });
                if self.peek() == &Token::Comma {
                    self.advance();
                }
                continue;
            } else if self.peek() == &Token::Reg {
                // bare reg without direction (non-standard)
                self.advance();
                is_sequential = true;
                let name = self.expect_ident()?;
                ports.push(UdpPort {
                    direction: PortDirection::Output,
                    name,
                    is_reg: true,
                });
                if self.peek() == &Token::Comma {
                    self.advance();
                }
                continue;
            } else {
                return Err(self.err_with_code(
                    DiagCode::ExpectedToken,
                    "expected direction (input/output) in UDP port list",
                ));
            };

            let name = self.expect_ident()?;
            ports.push(UdpPort {
                direction,
                name,
                is_reg: false,
            });

            if self.peek() == &Token::Comma {
                self.advance();
            }
        }
        self.skip_semi();

        // Check for optional initial statement (sequential UDP)
        let mut initial_output = None;
        if self.peek() == &Token::Initial {
            self.advance();
            // Expect output port name
            if matches!(self.peek(), Token::Ident(_)) || self.peek() == &Token::Output {
                self.advance();
            }
            // expect =
            self.expect(Token::BlockingAssign)?;
            let sym = self.parse_udp_symbol()?;
            initial_output = Some(sym);
            self.skip_semi();
        }

        let table = self.parse_udp_table(is_sequential)?;
        self.expect(Token::EndPrimitive)?;
        self.skip_semi();

        Ok(UdpDef {
            name,
            line: uline,
            col: ucol,
            ports,
            table,
            is_sequential,
            initial_output,
        })
    }
}
