//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan parser.rs (SRP Refactoring).
//! Tanggung jawab: Parsing ekspresi (expression parsing).
//!
//! Fungsi:
//!   - is_type_token()       — cek apakah token saat ini adalah tipe data
//!   - parse_dist_item()     — parsing dist item untuk constraint randomization
//!   - parse_expr()          — parsing expression (Pratt parser)
//!   - parse_primary_expr()  — parsing primary expression (literal, ident, dll.)
//!
//! ──────────────────────────────────────────────────────────────────────────────

use super::Parser;
use crate::lexer::*;
use crate::util::dec_str_to_bits;
use mivon_ast::types::const_eval_simple;
use mivon_ast::*;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

impl Parser {
    pub(crate) fn is_type_token(&self) -> bool {
        // `int'(expr)` / `logic'(expr)` / `real'(expr)` adalah CAST ekspresi,
        // bukan type parameter — jika token tipe diikuti `'` (Quote), parse
        // sebagai ekspresi agar instance `#(.p(int'(x)))` tidak salah dikira
        // type param (sebelumnya: "expected RParen, found '" di rv_dm dll.).
        if matches!(self.peek_ahead(1), Token::Quote) {
            return false;
        }
        matches!(
            self.peek(),
            Token::Bit
                | Token::Logic
                | Token::Int
                | Token::Integer
                | Token::Byte
                | Token::Shortint
                | Token::Longint
                | Token::Time
                | Token::Reg
                | Token::Real
                | Token::RealTime
                | Token::String
                | Token::Struct
                | Token::Union
                | Token::Enum
        )
    }

    pub(crate) fn parse_dist_item(&mut self) -> Result<DistItem, SimError> {
        // dist item: expr := weight  or  expr :/ weight  or  [lo:hi] := weight  or  [lo:hi] :/ weight
        if self.peek() == &Token::LBrack
            && self.peek_ahead(1) != &Token::RBrack
            && self.peek_ahead(1) != &Token::Colon
        {
            // Range item: [lo:hi] := weight or [lo:hi] :/ weight
            self.advance(); // [
            let lo = self.parse_expr(0)?;
            self.expect(Token::Colon)?;
            let hi = self.parse_expr(0)?;
            self.expect(Token::RBrack)?;
            // `:=` di-lex sebagai AssignOp (lexer.rs: `AssignOp, // :=`);
            // `Equiv` adalah `===` (case equality) — BUKAN dist weight.
            if self.peek() == &Token::AssignOp {
                self.advance();
                let val = self.parse_expr(0)?;
                let weight = const_eval_simple(&val).unwrap_or(0) as u64;
                Ok(DistItem::Range(
                    Box::new(lo),
                    Box::new(hi),
                    DistWeight::Item(weight),
                ))
            } else if matches!(self.peek(), Token::Colon) && self.peek_ahead(1) == &Token::Slash {
                // :/
                self.advance(); // :
                self.advance(); // /
                let val = self.parse_expr(0)?;
                let weight = const_eval_simple(&val).unwrap_or(0) as u64;
                Ok(DistItem::Range(
                    Box::new(lo),
                    Box::new(hi),
                    DistWeight::Range(weight),
                ))
            } else {
                Err(self.err("expected := or :/ after dist range"))
            }
        } else {
            // Single value: expr := weight or expr :/ weight
            let expr = self.parse_expr(0)?;
            // `:=` = AssignOp (bukan Equiv/`===`)
            if self.peek() == &Token::AssignOp {
                self.advance();
                let val = self.parse_expr(0)?;
                let weight = const_eval_simple(&val).unwrap_or(0) as u64;
                Ok(DistItem::Value(Box::new(expr), DistWeight::Item(weight)))
            } else if matches!(self.peek(), Token::Colon) && self.peek_ahead(1) == &Token::Slash {
                // :/
                self.advance(); // :
                self.advance(); // /
                let val = self.parse_expr(0)?;
                let weight = const_eval_simple(&val).unwrap_or(0) as u64;
                Ok(DistItem::Value(Box::new(expr), DistWeight::Range(weight)))
            } else {
                Err(self.err("expected := or :/ after dist item"))
            }
        }
    }

    /// Parse daftar argumen panggilan fungsi `f(a, b, ...)`, mendukung named
    /// argument `.name(expr)` (LRM 1800 §10.6.1) — pola umum OpenTitan DV:
    /// `str_remove_sections(.s(text), .start_delim("/*"), .end_delim("*/"))`.
    /// Nama argumen dibuang; urutan posisi dipertahankan (OpenTitan menulis
    /// named arg sesuai urutan deklarasi). Tanpa dukungan ini, `read_vmem` di
    /// `dv_utils_pkg.sv` gagal parse (`expected expression, found Dot`) →
    /// seluruh package hilang → E3001 'Host'/'Device' tidak ter-resolve.
    fn parse_call_args(&mut self) -> Result<Vec<Expr>, SimError> {
        let mut args = Vec::new();
        if self.peek() == &Token::RParen || self.peek() == &Token::Eof {
            return Ok(args);
        }
        loop {
            if self.peek() == &Token::Comma {
                // Empty positional arg: `f(a, , , msg)` — umum pada macro DV
                // OpenTitan setelah ekspansi (arg default di-skip). Push
                // placeholder agar jumlah arg tetap; evaluasi tidak memakai.
                args.push(Expr::Value(Value::Decimal(1)));
                self.advance();
                if self.peek() == &Token::RParen {
                    break;
                }
                continue;
            }
            if self.peek() == &Token::Dot {
                // Named arg: `.name(expr)` — skip nama, ambil ekspresinya.
                self.advance();
                self.expect_ident()?;
                self.expect(Token::LParen)?;
                let e = self.parse_expr(0)?;
                self.expect(Token::RParen)?;
                args.push(e);
            } else {
                // Some DV task calls pass a parenthesized ternary as an
                // argument (`f(x, (cond ? a : b))`).  The normal expression
                // parser can recover the value, but a malformed surrounding
                // macro expansion must not consume the call's closing paren.
                // Skip only this balanced argument when it visibly contains
                // a ternary and retain a harmless placeholder.
                if self.peek() == &Token::LParen {
                    let mut depth = 0i32;
                    let mut has_question = false;
                    let mut i = self.pos.get();
                    while i < self.tokens.len() {
                        match self.tokens[i].0 {
                            Token::LParen => depth += 1,
                            Token::RParen => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            Token::Question => has_question = true,
                            Token::Eof => break,
                            _ => {}
                        }
                        i += 1;
                    }
                    if has_question {
                        let _ = self.skip_balanced_paren();
                        args.push(Expr::Value(Value::Decimal(1)));
                        if self.peek() == &Token::Comma {
                            self.advance();
                            continue;
                        }
                        break;
                    }
                }
                let arg = match self.parse_expr(0) {
                    Ok(e) => e,
                    Err(_) => {
                        // Arg gagal parse (concat `{...}`, cast `T'(...)`,
                        // `$system(...)`, struct dll.) — recovery: skip ke
                        // koma/`)` agar pemanggil lanjut. Arg panggilan
                        // verification/dll tidak dipakai evaluator; placeholder
                        // menjaga jumlah arg.
                        let placeholder = Expr::Value(Value::Decimal(1));
                        loop {
                            match self.peek() {
                                Token::Comma | Token::RParen | Token::Eof => break,
                                _ => {
                                    self.advance();
                                }
                            }
                        }
                        placeholder
                    }
                };
                args.push(arg);
            }
            if self.peek() == &Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(args)
    }

    pub(crate) fn parse_expr(&mut self, min_prec: usize) -> Result<Expr, SimError> {
        let mut lhs = self.parse_primary_expr()?;

        loop {
            let op_info = match self.peek() {
                Token::Plus => Some((9, BinaryOp::Add)),
                Token::Minus => Some((9, BinaryOp::Sub)),
                Token::Star => Some((10, BinaryOp::Mul)),
                Token::Slash => Some((10, BinaryOp::Div)),
                Token::Percent => Some((10, BinaryOp::Mod)),
                Token::StarStar => Some((11, BinaryOp::Power)),
                Token::Eq => Some((6, BinaryOp::Eq)),
                Token::Neq => Some((6, BinaryOp::Neq)),
                Token::Equiv => Some((6, BinaryOp::CaseEq)),
                Token::NotEquiv => Some((6, BinaryOp::CaseNeq)),
                Token::CaseEq => Some((6, BinaryOp::EqWild)),
                Token::CaseNeq => Some((6, BinaryOp::NeqWild)),
                Token::WildcardEq => Some((6, BinaryOp::EqWild)),
                Token::WildcardNeq => Some((6, BinaryOp::NeqWild)),
                Token::Lt => Some((7, BinaryOp::Lt)),
                Token::Le | Token::NonBlockingAssign => Some((7, BinaryOp::Le)),
                Token::Gt => Some((7, BinaryOp::Gt)),
                Token::Ge => Some((7, BinaryOp::Ge)),
                Token::Shl => Some((8, BinaryOp::Shl)),
                Token::Shr => Some((8, BinaryOp::Shr)),
                Token::Sshl => Some((8, BinaryOp::Sshl)),
                Token::Sshr => Some((8, BinaryOp::Sshr)),
                Token::Amp => Some((5, BinaryOp::BitAnd)),
                Token::Pipe => Some((3, BinaryOp::BitOr)),
                Token::Caret => Some((4, BinaryOp::BitXor)),
                Token::CaretTilde => Some((4, BinaryOp::BitXnor)),
                Token::AmpAmp => Some((2, BinaryOp::LogicalAnd)),
                Token::PipePipe => Some((1, BinaryOp::LogicalOr)),
                // Logical implication `a -> b` (= `~a | b`) — umum di kode
                // formal verification / constraint. Dikenali dari token Arrow
                // di posisi ekspresi (statement-level `->event;` di-handle
                // parse_stmt_impl lebih dulu). Di-lower ke `~a | b`.
                // Precedence 0 = PALING RENDAH (di bawah `||`): implikasi hanya
                // dikonsumsi saat parse_expr(0) (level ekspresi penuh). Ini
                // mencegah delay statement `#5 -> ev;` salah ter-parse sbg
                // delay `~5 | ev` (delay di-parse dgn parse_expr(1) —
                // lihat parse_stmt Token::Hash) — `->` harus jatuh ke
                // statement EventTrigger, bukan operator ekspresi.
                Token::Arrow => {
                    if 0 < min_prec {
                        break;
                    }
                    self.advance();
                    let rhs = self.parse_expr(2)?;
                    lhs = Expr::BinaryOp {
                        op: BinaryOp::LogicalOr,
                        lhs: Box::new(Expr::UnaryOp {
                            op: UnaryOp::Not,
                            expr: Box::new(lhs),
                        }),
                        rhs: Box::new(rhs),
                    };
                    continue;
                }
                Token::Question => {
                    // Precedence ternary PALING RENDAH di SystemVerilog — di bawah
                    // `||` (tabel: 1). Tanpa guard min_prec ini, RHS dari operator
                    // binary yang lebih rapat akan menelan `? :` sebagai ternary
                    // lokal: `m == 3 ? 1 : 0` ter-parse `m == (3 ? 1 : 0)`
                    // (= `m == 1`) alih-alih `(m == 3) ? 1 : 0`. Guard
                    // `0 < min_prec` → ternary hanya dibentuk di level ekspresi
                    // penuh (min_prec == 0); di RHS binary, break → ternary
                    // ditangani oleh loop pemanggil (LRM 1800 §11.4.11).
                    if 0 < min_prec {
                        break;
                    }
                    self.advance();
                    let true_expr = self.parse_expr(0)?;
                    self.expect(Token::Colon)?;
                    let false_expr = self.parse_expr(0)?;
                    return Ok(Expr::TernaryOp {
                        cond: Box::new(lhs),
                        true_expr: Box::new(true_expr),
                        false_expr: Box::new(false_expr),
                    });
                }
                Token::Inside => {
                    if 7 < min_prec {
                        break;
                    }
                    self.advance();
                    // `inside` dengan macro yang tidak ter-expand: `inside `MACRO_NAME`
                    // — macro hasilkan `{...}` tapi preprocessor tidak memiliki
                    // definisinya. Hasilnya token Ident setelah `inside`, bukan LBrace.
                    // Fallback: skip token Ident tersebut dan lanjut (return lhs as-is).
                    if self.peek() != &Token::LBrace {
                        // skip single ident (macro name) jika bukan LBrace
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                        lhs = Expr::Inside {
                            expr: Box::new(lhs),
                            range_list: vec![],
                        };
                        continue;
                    }
                    self.expect(Token::LBrace)?;
                    let mut range_list = Vec::new();
                    if self.peek() != &Token::RBrace {
                        loop {
                            // Range inside `{[a:b], c}` — `[a:b]` adalah range
                            // (bukan bit-select). Representasikan sebagai
                            // RangeSelect dengan base 0; const_eval Inside
                            // memperlakukan RangeSelect sebagai rentang.
                            if self.peek() == &Token::LBrack {
                                self.advance();
                                let msb = self.parse_expr(0)?;
                                self.expect(Token::Colon)?;
                                let lsb = self.parse_expr(0)?;
                                self.expect(Token::RBrack)?;
                                range_list.push(Expr::RangeSelect {
                                    expr: Box::new(Expr::Value(Value::Decimal(0))),
                                    msb: Box::new(msb),
                                    lsb: Box::new(lsb),
                                });
                            } else {
                                range_list.push(self.parse_expr(0)?);
                            }
                            if self.peek() == &Token::Comma {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(Token::RBrace)?;
                    lhs = Expr::Inside {
                        expr: Box::new(lhs),
                        range_list,
                    };
                    continue;
                }
                Token::Ident(ref s) if s == "dist" => {
                    if 7 < min_prec {
                        break;
                    }
                    self.advance();
                    self.expect(Token::LBrace)?;
                    let mut items = Vec::new();
                    if self.peek() != &Token::RBrace {
                        loop {
                            items.push(self.parse_dist_item()?);
                            if self.peek() == &Token::Comma {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(Token::RBrace)?;
                    lhs = Expr::Dist {
                        expr: Box::new(lhs),
                        items,
                    };
                    continue;
                }
                Token::Ident(ref s) if s == "with" => {
                    if 6 < min_prec {
                        break;
                    }
                    self.advance();
                    let with_expr = if self.peek() == &Token::LBrace {
                        self.advance();
                        // Randomize-with: coba parse body sbg constraint items
                        // (agar `with { addr > 200; }` constraint DIHORMATI
                        // solver — regresi F17). Bila body memuat konstruk
                        // constraint-only (if/soft/foreach, pola UVM) yang
                        // parser tidak dukung, fallback structural skip agar
                        // tidak desync (OpenTitan DV).
                        let saved = self.pos.get();
                        match self.parse_constraint_items() {
                            Ok(items) => {
                                let exprs: Vec<Expr> = items
                                    .into_iter()
                                    .filter_map(|ci| match ci {
                                        ConstraintItem::Expr(e) => Some(e),
                                        _ => None,
                                    })
                                    .collect();
                                self.expect(Token::RBrace)?;
                                exprs
                                    .into_iter()
                                    .reduce(|acc, e| Expr::BinaryOp {
                                        op: BinaryOp::LogicalAnd,
                                        lhs: Box::new(acc),
                                        rhs: Box::new(e),
                                    })
                                    .unwrap_or(Expr::Value(Value::Decimal(1)))
                            }
                            Err(_) => {
                                self.pos.set(saved);
                                let mut depth = 1usize;
                                while depth > 0 && self.peek() != &Token::Eof {
                                    match self.peek() {
                                        Token::LBrace => {
                                            depth += 1;
                                            self.advance();
                                        }
                                        Token::RBrace => {
                                            depth -= 1;
                                            self.advance();
                                        }
                                        _ => self.advance(),
                                    }
                                }
                                Expr::Value(Value::Decimal(1))
                            }
                        }
                    } else {
                        self.expect(Token::LParen)?;
                        let e = self.parse_expr(0)?;
                        self.expect(Token::RParen)?;
                        e
                    };
                    let old_lhs = std::mem::replace(&mut lhs, Expr::Value(Value::Decimal(0)));
                    match old_lhs {
                        Expr::MethodCall {
                            obj,
                            method,
                            args,
                            with_clause: None,
                        } => {
                            lhs = Expr::MethodCall {
                                obj,
                                method,
                                args,
                                with_clause: Some(Box::new(with_expr)),
                            };
                        }
                        // F17: pola UVM `!x.randomize() with { ... }` — `with`
                        // melekat ke MethodCall DALAM, `!` tetap di luar:
                        // `!(x.randomize() with { ... })`. Sebelumnya parser
                        // error "'with' clause can only follow a method call".
                        Expr::UnaryOp {
                            op: UnaryOp::Not,
                            expr,
                        } => {
                            // `!x.randomize() with {...}` — MethodCall dalam.
                            let inner_attached = match *expr {
                                Expr::MethodCall {
                                    obj,
                                    method,
                                    args,
                                    with_clause: None,
                                } => Some(Expr::MethodCall {
                                    obj,
                                    method,
                                    args,
                                    with_clause: Some(Box::new(with_expr.clone())),
                                }),
                                // `!std::randomize(x) with {...}` — FuncCall dalam
                                // (pola UVM std::randomize). Clause diabaikan.
                                Expr::FuncCall { .. }
                                | Expr::ScopedIdent { .. }
                                | Expr::Ident { .. } => Some(*expr),
                                _ => None,
                            };
                            if let Some(inner) = inner_attached {
                                lhs = Expr::UnaryOp {
                                    op: UnaryOp::Not,
                                    expr: Box::new(inner),
                                };
                            } else {
                                return Err(self.err("'with' clause can only follow a method call"));
                            }
                        }
                        // Pola DV UVM `std::randomize(x) with { ... }` /
                        // `void'(randomize(y) with { ... })` — `with` melekat
                        // pada panggilan fungsi (FuncCall), bukan hanya method
                        // call. Clause diabaikan; panggilan dipetahankan utuh.
                        Expr::FuncCall { .. } | Expr::ScopedIdent { .. } => {
                            lhs = old_lhs;
                        }
                        _ => return Err(self.err("'with' clause can only follow a method call")),
                    }
                    continue;
                }
                Token::LBrack => {
                    self.advance();
                    if self.peek() == &Token::RBrack {
                        self.advance();
                        continue;
                    }
                    let first = self.parse_expr(0)?;
                    if self.peek() == &Token::Colon {
                        self.advance();
                        let second = self.parse_expr(0)?;
                        self.expect(Token::RBrack)?;
                        lhs = Expr::RangeSelect {
                            expr: Box::new(lhs),
                            msb: Box::new(first),
                            lsb: Box::new(second),
                        };
                    } else if self.peek() == &Token::PlusColon {
                        self.advance();
                        let width = self.parse_expr(0)?;
                        self.expect(Token::RBrack)?;
                        lhs = Expr::PartSelect {
                            expr: Box::new(lhs),
                            base: Box::new(first),
                            width: Box::new(width),
                        };
                    } else if self.peek() == &Token::MinusColon {
                        self.advance();
                        let width = self.parse_expr(0)?;
                        self.expect(Token::RBrack)?;
                        lhs = Expr::PartSelect {
                            expr: Box::new(lhs),
                            base: Box::new(Expr::BinaryOp {
                                op: BinaryOp::Sub,
                                lhs: Box::new(first.clone()),
                                rhs: Box::new(Expr::BinaryOp {
                                    op: BinaryOp::Sub,
                                    lhs: Box::new(width.clone()),
                                    rhs: Box::new(Expr::Value(Value::Decimal(1))),
                                }),
                            }),
                            width: Box::new(width),
                        };
                    } else {
                        self.expect(Token::RBrack)?;
                        lhs = Expr::BitSelect {
                            expr: Box::new(lhs),
                            index: Box::new(first),
                        };
                    }
                    continue;
                }
                Token::Dot => {
                    self.advance();
                    let member = self.expect_ident()?;
                    if self.peek() == &Token::LParen {
                        self.advance();
                        let args = self.parse_call_args()?;
                        self.expect(Token::RParen)?;
                        lhs = Expr::MethodCall {
                            obj: Box::new(lhs),
                            method: member,
                            args,
                            with_clause: None,
                        };
                    } else {
                        lhs = Expr::MemberAccess {
                            obj: Box::new(lhs),
                            field: member,
                        };
                    }
                    continue;
                }
                // Cast dengan width dari ekspresi: `size'(expr)` per LRM 1800
                // (casting_type dapat berupa `constant_primary`). Contoh OpenTitan:
                // `data_q[$clog2(dm::DataCount)'(autoexecdata_idx)]` — di sini
                // `$clog2(...)` adalah ekspresi (bukan nama tipe). Parse_primary
                // sudah menangani Ident'/Number'/(type)' ; postfix ini menangkap
                // sisa bentuk (FuncCall', scoped', dsb).
                Token::Quote => {
                    if self.peek_ahead(1) != &Token::LParen {
                        break;
                    }
                    self.advance();
                    self.advance();
                    let expr = self.parse_expr(0)?;
                    self.expect(Token::RParen)?;
                    lhs = Expr::CastWidth {
                        width: Box::new(lhs),
                        expr: Box::new(expr),
                    };
                    continue;
                }
                // Post-increment / post-decrement: `expr++` / `expr--`
                // (LRM 1800 §11.4.1) — varian IncDec khusus agar side-effect
                // (write-back nilai ±1) dieksekusi evaluator, bukan sekadar
                // nilai `expr ± 1` (bug: `k = i--` → k salah, i tak berubah).
                Token::Increment | Token::Decrement => {
                    let is_inc = matches!(self.peek(), Token::Increment);
                    self.advance();
                    lhs = Expr::IncDec {
                        expr: Box::new(lhs),
                        inc: is_inc,
                        pre: false,
                    };
                    continue;
                }
                _ => None,
            };

            match op_info {
                Some((prec, op)) => {
                    if prec < min_prec {
                        break;
                    }
                    self.advance();
                    let rhs = self.parse_expr(prec + 1)?;
                    lhs = Expr::BinaryOp {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                None => break,
            }
        }

        Ok(lhs)
    }

    pub(crate) fn parse_primary_expr(&mut self) -> Result<Expr, SimError> {
        self.push_depth()?;
        let result = self.parse_primary_expr_impl();
        self.pop_depth();
        result
    }

    fn parse_primary_expr_impl(&mut self) -> Result<Expr, SimError> {
        let tok = self.peek().clone();
        match tok {
            // Constraint implication may use `soft` on the consequent:
            // `condition -> soft expression`.  Soft is a constraint modifier,
            // not part of the expression AST, so consume it and parse the
            // consequent normally.
            Token::Soft => {
                self.advance();
                self.parse_primary_expr_impl()
            }
            // `$` sebagai end-marker dalam slice queue `[1:$]` (LRM 1800 §7.12.4)
            // — contoh `tokens[1:$]` di `dv_utils_pkg::read_vmem`. Di sini `$`
            // berarti "sampai elemen terakhir" (bukan system function). Dikenali
            // dari token setelahnya (`]`, `,`, `)` atau `:` dalam range). Tanpa ini
            // `[1:$]` error "expected system function name" → package gagal parse.
            Token::Dollar
                if matches!(
                    self.peek_ahead(1),
                    Token::RBrack | Token::Comma | Token::RParen | Token::Colon
                ) =>
            {
                let dl = self.peek_line();
                let dc = self.peek_col();
                self.advance();
                Ok(Expr::Ident {
                    name: Symbol::intern("$"),
                    line: dl,
                    col: dc,
                })
            }
            Token::Dollar => {
                let sf_line = self.peek_line();
                let sf_col = self.peek_col();
                self.advance();
                let name_tok = self.peek().clone();
                let name_sym = match &name_tok {
                    Token::Ident(n) => {
                        self.advance();
                        *n
                    }
                    Token::Time => {
                        self.advance();
                        Symbol::intern("time")
                    }
                    Token::Real => {
                        self.advance();
                        Symbol::intern("real")
                    }
                    Token::RealTime => {
                        self.advance();
                        Symbol::intern("realtime")
                    }
                    Token::Signed => {
                        self.advance();
                        Symbol::intern("signed")
                    }
                    Token::Unsigned => {
                        self.advance();
                        Symbol::intern("unsigned")
                    }
                    // `$` sebagai ekspresi mandiri: `queue[$]` (indeks terakhir),
                    // `[0:$]` (range tak-berujung), `with ($countones(...) != 1)`.
                    // Bukan system function — representasikan sebagai ident khusus
                    // agar parse tidak gagal; elaborate tak memakai nilainya.
                    _ => {
                        return Ok(Expr::Ident {
                            name: Symbol::intern("$"),
                            line: sf_line,
                            col: sf_col,
                        })
                    }
                };
                // SAFETY: Stack buffer 128 bytes cukup untuk semua system function SV
                // ($display, $monitor, $urandom, dll.). Symbol.as_str() selalu valid UTF-8.
                // Untuk nama >127 chars (sangat jarang), fallback ke format!()
                let full_name = {
                    let name_str = name_sym.as_str();
                    if name_str.len() > 127 {
                        // Nama panjang: pakai heap alloc (sangat jarang)
                        Symbol::intern(&format!("${}", name_str))
                    } else {
                        let mut buf = [0u8; 128];
                        buf[0] = b'$';
                        let name_bytes = name_str.as_bytes();
                        // Safe: name_bytes valid UTF-8, buf[1..] cukup untuk copy
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                name_bytes.as_ptr(),
                                buf.as_mut_ptr().add(1),
                                name_bytes.len(),
                            );
                            Symbol::intern(std::str::from_utf8_unchecked(
                                &buf[..name_bytes.len() + 1],
                            ))
                        }
                    }
                };
                if self.peek() == &Token::LParen {
                    let fl = self.peek_line();
                    let fc = self.peek_col();
                    self.advance();
                    let args = self.parse_call_args()?;
                    self.expect(Token::RParen)?;
                    Ok(Expr::FuncCall {
                        name: full_name,
                        args,
                        line: fl,
                        col: fc,
                    })
                } else {
                    Ok(Expr::Ident {
                        name: full_name,
                        line: sf_line,
                        col: sf_col,
                    })
                }
            }
            Token::Ident(name) => {
                let line = self.peek_line();
                let col = self.peek_col();
                self.advance();
                // pkg::item::method resolution (with chaining support)
                if self.peek() == &Token::Scope {
                    let mut scope_path = name;
                    loop {
                        self.advance();
                        let sc_line = self.peek_line();
                        let sc_col = self.peek_col();
                        let item = self.expect_ident()?;
                        // Check for another :: (chaining: pkg::item::method)
                        if self.peek() == &Token::Scope {
                            scope_path = Symbol::intern(&format!("{}::{}", scope_path, item));
                            continue;
                        }
                        if self.peek() == &Token::LParen {
                            let fl = self.peek_line();
                            let fc = self.peek_col();
                            self.advance();
                            let args = self.parse_call_args()?;
                            self.expect(Token::RParen)?;
                            // `std::randomize(...) with { ... }` uses a
                            // verification-only constraint body. Consume it
                            // structurally here because scoped calls return
                            // directly instead of passing through postfix
                            // expression handling.
                            if matches!(self.peek(), Token::Ident(s) if s == "with") {
                                self.advance();
                                if self.peek() == &Token::LBrace {
                                    self.advance();
                                    let mut depth = 1usize;
                                    while depth > 0 && self.peek() != &Token::Eof {
                                        match self.peek() {
                                            Token::LBrace => {
                                                depth += 1;
                                                self.advance();
                                            }
                                            Token::RBrace => {
                                                depth -= 1;
                                                self.advance();
                                            }
                                            _ => self.advance(),
                                        }
                                    }
                                }
                            }
                            return Ok(Expr::FuncCall {
                                name: Symbol::intern(&format!("{}::{}", scope_path, item)),
                                args,
                                line: fl,
                                col: fc,
                            });
                        }
                        // Type cast scoped: `pkg::type'(expr)` atau `pkg::type'{...}` (struct pattern)
                        if self.peek() == &Token::Quote {
                            self.advance();
                            if self.peek() == &Token::LBrace {
                                self.advance();
                                return self.parse_struct_pattern_body();
                            }
                            self.expect(Token::LParen)?;
                            let expr = self.parse_expr(0)?;
                            self.expect(Token::RParen)?;
                            return Ok(Expr::Cast {
                                dtype: Symbol::intern(&format!("{}::{}", scope_path, item)),
                                expr: Box::new(expr),
                            });
                        }
                        return Ok(Expr::ScopedIdent {
                            package: scope_path,
                            item,
                            line: sc_line,
                            col: sc_col,
                        });
                    }
                }
                // Class#(Type)::method resolution
                if self.peek() == &Token::Hash {
                    self.advance();
                    self.expect(Token::LParen)?;
                    let mut type_specs = Vec::new();
                    loop {
                        if self.peek() == &Token::RParen {
                            break;
                        }
                        // Named param `.P(VALUE)` — pola DV UVM
                        // `Class#(.DeviceDataWidth(8))::type::new`. Item ini
                        // BUKAN tipe — parse `expr` (eksi `parse_type_expr`
                        // yang gagal dgn "expected type" pada Dot).
                        if self.peek() == &Token::Dot {
                            self.advance();
                            let _ = self.expect_ident()?;
                            self.expect(Token::LParen)?;
                            let _ = self.parse_expr(0)?;
                            self.expect(Token::RParen)?;
                        } else {
                            // Parameterized class calls accept both type
                            // arguments and constant expressions:
                            // `mem#(pkg::BytesPerBank / 4)`.  A scoped
                            // identifier followed by an operator is an
                            // expression, not a type.
                            let is_virtual_type = self.peek() == &Token::Virtual;
                            let is_type_arg = is_virtual_type
                                || self.is_type_token()
                                || (matches!(self.peek(), Token::Ident(_))
                                    && matches!(
                                        self.peek_ahead(1),
                                        Token::Ident(_) | Token::LBrack
                                    ));
                            if is_type_arg {
                                if is_virtual_type {
                                    self.advance();
                                }
                                type_specs.push(self.parse_type_expr()?);
                            } else {
                                let _ = self.parse_expr(0)?;
                            }
                        }
                        if self.peek() == &Token::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(Token::RParen)?;
                    let class_prefix = if type_specs.is_empty() {
                        name
                    } else {
                        let type_strs: Vec<String> =
                            type_specs.iter().map(|dt| dt.to_string()).collect();
                        let suffix = type_strs.join(",");
                        Symbol::intern(&format!("{}#{}", name, suffix))
                    };
                    if self.peek() == &Token::Scope {
                        let mut scope_path = class_prefix;
                        loop {
                            self.advance();
                            let sc_line = self.peek_line();
                            let sc_col = self.peek_col();
                            let item = self.expect_ident()?;
                            if self.peek() == &Token::Scope {
                                scope_path = Symbol::intern(&format!("{}::{}", scope_path, item));
                                continue;
                            }
                            if self.peek() == &Token::LParen {
                                if self.peek() == &Token::Quote
                                    && self.peek_ahead(1) == &Token::LParen
                                {
                                    self.advance();
                                    self.advance();
                                    let expr = self.parse_expr(0)?;
                                    self.expect(Token::RParen)?;
                                    return Ok(Expr::Cast {
                                        dtype: Symbol::intern(&format!("{}::{}", scope_path, item)),
                                        expr: Box::new(expr),
                                    });
                                }
                                let fl = self.peek_line();
                                let fc = self.peek_col();
                                self.advance();
                                let args = self.parse_call_args()?;
                                self.expect(Token::RParen)?;
                                return Ok(Expr::FuncCall {
                                    name: Symbol::intern(&format!("{}::{}", scope_path, item)),
                                    args,
                                    line: fl,
                                    col: fc,
                                });
                            }
                            return Ok(Expr::ScopedIdent {
                                package: scope_path,
                                item,
                                line: sc_line,
                                col: sc_col,
                            });
                        }
                    }
                    return Ok(Expr::Ident {
                        name: class_prefix,
                        line,
                        col,
                    });
                }
                // Type cast: type_name'(expr) — atau cast-to-struct pattern
                // `type'{member: value}` (OpenTitan `ram_1p_cfg_req_t'{req: x}`).
                if self.peek() == &Token::Quote {
                    self.advance();
                    if self.peek() == &Token::LBrace {
                        self.advance();
                        return self.parse_struct_pattern_body();
                    }
                    self.expect(Token::LParen)?;
                    let expr = self.parse_expr(0)?;
                    self.expect(Token::RParen)?;
                    return Ok(Expr::Cast {
                        dtype: name,
                        expr: Box::new(expr),
                    });
                }
                if self.peek() == &Token::LParen {
                    let fl = self.peek_line();
                    let fc = self.peek_col();
                    self.advance();
                    let args = self.parse_call_args()?;
                    self.expect(Token::RParen)?;
                    Ok(Expr::FuncCall {
                        name,
                        args,
                        line: fl,
                        col: fc,
                    })
                } else {
                    Ok(Expr::Ident { name, line, col })
                }
            }
            Token::Number {
                value,
                base,
                width,
                is_signed,
            } => {
                self.advance();
                if self.peek() == &Token::Quote && self.peek_ahead(1) == &Token::LParen {
                    self.advance();
                    self.advance();
                    let expr = self.parse_expr(0)?;
                    self.expect(Token::RParen)?;
                    let n = value.as_str().parse::<i64>().unwrap_or(0);
                    return Ok(Expr::Cast {
                        dtype: Symbol::intern(&format!("{}", n)),
                        expr: Box::new(expr),
                    });
                }
                let val = if let Some(base) = base {
                    match base {
                        2 => Expr::Value(Value::Binary {
                            bits: value.as_str().to_string(),
                            width,
                            is_signed,
                        }),
                        8 => Expr::Value(Value::Octal {
                            bits: value.as_str().to_string(),
                            width,
                            is_signed,
                        }),
                        10 => decimal_value_expr(&value, width, is_signed),
                        16 => Expr::Value(Value::Hex {
                            bits: value.as_str().to_string(),
                            width,
                            is_signed,
                        }),
                        _ => decimal_value_expr(&value, width, is_signed),
                    }
                } else {
                    if let Ok(n) = value.as_str().parse::<i64>() {
                        // Literal waktu (time literal): `1ns`, `5us`, `10ms`, `1s`,
                        // `100ps`, `1fs`, `1step` (LRM 1800 §6.17). FastLexer memecah
                        // `1ns` menjadi `Number` + `Ident('ns')`; tanpa penanganan ini
                        // `#(interval_ns * 1ns)` gagal parse (expected RParen, found
                        // 'ns') — seperti di `dv_utils_pkg::poll_for_stop` — sehingga
                        // SELURUH package hilang dari design (E3001 'Host'/'Device'
                        // tidak ter-resolve). Skala ke tick basis 1ns (Mivon default;
                        // design.timescale selalu None → basis 1ns). Unit < 1 tick
                        // (ps/fs/step) dibulatkan naik ke 1 agar `#1ps` tidak menjadi
                        // delay 0 (loop tak berujung).
                        let scaled = match self.peek() {
                            Token::Ident(u) if u == "s" => {
                                self.advance();
                                Some(n.saturating_mul(1_000_000_000))
                            }
                            Token::Ident(u) if u == "ms" => {
                                self.advance();
                                Some(n.saturating_mul(1_000_000))
                            }
                            Token::Ident(u) if u == "us" => {
                                self.advance();
                                Some(n.saturating_mul(1_000))
                            }
                            Token::Ident(u) if u == "ns" => {
                                self.advance();
                                Some(n)
                            }
                            Token::Ident(u) if u == "ps" || u == "fs" || u == "step" => {
                                self.advance();
                                Some(n.max(1))
                            }
                            _ => None,
                        };
                        Expr::Value(Value::Decimal(scaled.unwrap_or(n)))
                    } else {
                        // Melebihi i64 (dan bukan time literal): tetap angka —
                        // dulu jatuh ke Expr::Ident → implicit net diam-diam
                        // (fuzzer partsel seed=28).
                        decimal_value_expr(&value, None, false)
                    }
                };
                Ok(val)
            }
            Token::RealNum(s) => {
                self.advance();
                let mut val = s.as_str().parse::<f64>().unwrap_or(0.0);
                // Realtime literal dgn satuan waktu: `104166.667ns`, `1.5us`,
                // `2.5ms` (LRM 1800 §6.4/§22.7). Angka real diikuti token unit;
                // skala ke basis 1ns. Unit lebih kecil (ps/fs) dibulatkan ke ~ns.
                if let Token::Ident(u) = self.peek() {
                    let mult = match u.as_str() {
                        "s" => Some(1_000_000_000.0),
                        "ms" => Some(1_000_000.0),
                        "us" => Some(1_000.0),
                        "ns" => Some(1.0),
                        "ps" => Some(1e-3),
                        "fs" => Some(1e-6),
                        _ => None,
                    };
                    if let Some(m) = mult {
                        self.advance();
                        val *= m;
                    }
                }
                Ok(Expr::Value(Value::Real(val)))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::String(s.as_str().to_string()))
            }
            Token::New => {
                self.advance();
                if self.peek() == &Token::LBrack {
                    let fl = self.peek_line();
                    let fc = self.peek_col();
                    self.advance();
                    let size = self.parse_expr(0)?;
                    self.expect(Token::RBrack)?;
                    // `new[N](old)` — salin min(old,new) elemen pertama
                    // (LRM 1800 §7.5.2.1). Sebelumnya init dibuang diam-diam
                    // → resize kehilangan data (probe: new[6](d) → d3=0).
                    let mut new_args = vec![size];
                    if self.peek() == &Token::LParen {
                        self.advance();
                        let val = self.parse_expr(0)?;
                        self.expect(Token::RParen)?;
                        new_args.push(val);
                    }
                    Ok(Expr::FuncCall {
                        name: Symbol::intern("new"),
                        args: new_args,
                        line: fl,
                        col: fc,
                    })
                } else if self.peek() == &Token::LParen {
                    let fl = self.peek_line();
                    let fc = self.peek_col();
                    self.advance();
                    let args = self.parse_call_args()?;
                    self.expect(Token::RParen)?;
                    Ok(Expr::FuncCall {
                        name: Symbol::intern("new"),
                        args,
                        line: fl,
                        col: fc,
                    })
                } else {
                    // F41: `new` tanpa argumen (mis. `mailbox req_mbx = new;`)
                    // — representasikan sebagai panggilan new kosong.
                    let fl = self.peek_line();
                    let fc = self.peek_col();
                    Ok(Expr::FuncCall {
                        name: Symbol::intern("new"),
                        args: vec![],
                        line: fl,
                        col: fc,
                    })
                }
            }
            Token::This => {
                let th_line = self.peek_line();
                let th_col = self.peek_col();
                self.advance();
                Ok(Expr::Ident {
                    name: Symbol::intern("this"),
                    line: th_line,
                    col: th_col,
                })
            }
            Token::Null => {
                self.advance();
                Ok(Expr::Null)
            }
            Token::Plus
            | Token::Minus
            | Token::Tilde
            | Token::Amp
            | Token::Pipe
            | Token::Caret
            | Token::TildeAmp
            | Token::TildePipe
            | Token::CaretTilde => {
                let saved_tok = tok.clone();
                self.advance();
                let op = match saved_tok {
                    Token::Plus => UnaryOp::Plus,
                    Token::Minus => UnaryOp::Minus,
                    Token::Tilde => UnaryOp::BitNot,
                    Token::Amp => UnaryOp::ReductionAnd,
                    Token::Pipe => UnaryOp::ReductionOr,
                    Token::Caret => UnaryOp::ReductionXor,
                    Token::TildeAmp => UnaryOp::ReductionNand,
                    Token::TildePipe => UnaryOp::ReductionNor,
                    Token::CaretTilde => UnaryOp::ReductionXnor,
                    _ => unreachable!(),
                };
                let expr = self.parse_expr(12)?;
                Ok(Expr::UnaryOp {
                    op,
                    expr: Box::new(expr),
                })
            }
            Token::Not => {
                self.advance();
                let expr = self.parse_expr(12)?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            Token::LBrace => {
                self.advance();
                if matches!(
                    self.peek(),
                    Token::Shl | Token::Shr | Token::Sshl | Token::Sshr
                ) {
                    let op = if matches!(self.peek(), Token::Shl | Token::Sshl) {
                        String::from("<<")
                    } else {
                        String::from(">>")
                    };
                    self.advance();
                    let slice_size = if !matches!(self.peek(), Token::LBrace) {
                        Some(Box::new(self.parse_expr(0)?))
                    } else {
                        None
                    };
                    self.expect(Token::LBrace)?;
                    let mut slices = Vec::new();
                    loop {
                        if self.peek() == &Token::RBrace {
                            break;
                        }
                        slices.push(self.parse_expr(0)?);
                        if self.peek() == &Token::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(Token::RBrace)?;
                    self.expect(Token::RBrace)?;
                    return Ok(Expr::StreamingConcat {
                        op,
                        slice_size,
                        slices,
                    });
                }
                let mut exprs = Vec::new();
                loop {
                    if self.peek() == &Token::RBrace {
                        break;
                    }
                    let item = self.parse_expr(0)?;
                    if self.peek() == &Token::LBrace {
                        self.advance();
                        let mut inner_exprs = Vec::new();
                        loop {
                            inner_exprs.push(self.parse_expr(0)?);
                            if self.peek() == &Token::Comma {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        self.expect(Token::RBrace)?;
                        let inner = if inner_exprs.len() == 1 {
                            inner_exprs.into_iter().next().unwrap()
                        } else {
                            Expr::Concat(inner_exprs)
                        };
                        exprs.push(Expr::Replicate {
                            count: Box::new(item),
                            expr: Box::new(inner),
                        });
                    } else {
                        exprs.push(item);
                    }
                    if self.peek() == &Token::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(Token::RBrace)?;
                if exprs.len() == 1 {
                    Ok(exprs.into_iter().next().unwrap())
                } else {
                    Ok(Expr::Concat(exprs))
                }
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr(0)?;
                self.expect(Token::RParen)?;
                Ok(Expr::Paren(Box::new(expr)))
            }
            Token::FillLit(val) => {
                self.advance();
                Ok(Expr::FillLit(val))
            }
            Token::Auto => {
                self.advance();
                Ok(Expr::Ident {
                    name: Symbol::intern("automatic"),
                    line: 0,
                    col: 0,
                })
            }
            Token::String => {
                self.advance();
                Ok(Expr::Ident {
                    name: Symbol::intern("string"),
                    line: 0,
                    col: 0,
                })
            }
            Token::Class | Token::EndClass => {
                self.advance();
                Ok(Expr::Ident {
                    name: Symbol::intern("class"),
                    line: 0,
                    col: 0,
                })
            }
            Token::Quote => {
                self.advance();
                if self.peek() == &Token::LBrace {
                    self.advance();
                    self.parse_struct_pattern_body()
                } else {
                    Ok(Expr::FillLit(mivon_core::LogicVal::Zero))
                }
            }
            Token::Increment => {
                self.advance();
                let expr = self.parse_expr(12)?;
                Ok(Expr::IncDec {
                    expr: Box::new(expr),
                    inc: true,
                    pre: true,
                })
            }
            Token::Decrement => {
                self.advance();
                let expr = self.parse_expr(12)?;
                Ok(Expr::IncDec {
                    expr: Box::new(expr),
                    inc: false,
                    pre: true,
                })
            }
            Token::Void
            | Token::Int
            | Token::Integer
            | Token::Logic
            | Token::Bit
            | Token::Byte
            | Token::Shortint
            | Token::Longint
            | Token::Time
            | Token::Signed
            | Token::Unsigned
            | Token::Real
            | Token::RealTime => {
                self.advance();
                let type_name = match tok {
                    Token::Void => "void",
                    Token::Int => "int",
                    Token::Integer => "integer",
                    Token::Logic => "logic",
                    Token::Bit => "bit",
                    Token::Byte => "byte",
                    Token::Shortint => "shortint",
                    Token::Longint => "longint",
                    Token::Time => "time",
                    Token::Signed => "signed",
                    Token::Unsigned => "unsigned",
                    Token::Real => "real",
                    Token::RealTime => "realtime",
                    _ => unreachable!(),
                };
                if self.peek() == &Token::Quote {
                    self.advance();
                    self.expect(Token::LParen)?;
                    let expr = self.parse_expr(0)?;
                    self.expect(Token::RParen)?;
                    Ok(Expr::Cast {
                        dtype: Symbol::intern(type_name),
                        expr: Box::new(expr),
                    })
                } else {
                    Ok(Expr::Ident {
                        name: Symbol::intern(type_name),
                        line: 0,
                        col: 0,
                    })
                }
            }
            ref other => {
                // Lokasi diambil err() sendiri (peek_line/peek_col internal),
                // jadi tidak perlu diambil manual di sini.
                Err(self.err(format!("expected expression, found {:?}", other)))
            }
        }
    }

    /// Parse body struct pattern: preambilang pos di member PERTAMA (dopo `{`
    /// di-konsumi). `{name: value, value, default: value, ...}` sampai `}`.
    /// Dipakai untuk `'{...}` (struct literal) DAN cast-to-struct
    /// `type'{member: value}` (OpenTitan `ram_1p_cfg_req_t'{req: x}`).
    fn parse_struct_pattern_body(&mut self) -> Result<Expr, SimError> {
        let saved = self.pos.get();
        let mut members: Vec<StructLitMember> = Vec::new();
        let mut any_named = false;
        let mut ok = true;
        loop {
            if self.peek() == &Token::RBrace {
                self.advance();
                break;
            }
            if self.peek() == &Token::Eof {
                ok = false;
                break;
            }
            // `default: value` — keyword (bukan Ident), tangani dulu.
            if self.peek() == &Token::Default {
                self.advance();
                if self.peek() != &Token::Colon {
                    ok = false;
                    break;
                }
                self.advance();
                match self.parse_expr(0) {
                    Ok(val) => {
                        any_named = true;
                        members.push(StructLitMember::Default(val));
                    }
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            } else {
                match self.parse_expr(0) {
                    Ok(first) => {
                        if self.peek() == &Token::Colon {
                            // Named-field assignment pattern:
                            // `name: value` — nama harus Ident.
                            self.advance();
                            match self.parse_expr(0) {
                                Ok(val) => match first {
                                    Expr::Ident { name, .. } => {
                                        any_named = true;
                                        members.push(StructLitMember::Named(name, val));
                                    }
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                },
                                Err(_) => {
                                    ok = false;
                                    break;
                                }
                            }
                        } else {
                            members.push(StructLitMember::Positional(first));
                        }
                    }
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if self.peek() == &Token::Comma {
                self.advance();
            } else if self.peek() == &Token::RBrace {
                self.advance();
                break;
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            if any_named {
                // Pola struct bernama — pertahankan nama field agar
                // member access bisa di-const-eval.
                return Ok(Expr::StructLit { members });
            }
            // Semua posisional — perilaku lama (array literal).
            let mut elements = Vec::new();
            for m in members {
                if let StructLitMember::Positional(e) = m {
                    elements.push(e);
                }
            }
            if elements.len() == 1 {
                return Ok(elements.into_iter().next().unwrap());
            }
            return Ok(Expr::Concat(elements));
        }
        // Fallback: skip ke brace penutup (perilaku lama).
        self.pos.set(saved);
        let mut depth = 1usize;
        while depth > 0 && self.peek() != &Token::Eof {
            match self.peek() {
                Token::LBrace => {
                    depth += 1;
                    self.advance();
                }
                Token::RBrace => {
                    depth -= 1;
                    if depth > 0 {
                        self.advance();
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }
        if self.peek() == &Token::RBrace {
            self.advance();
        }
        Ok(Expr::FillLit(mivon_core::LogicVal::Zero))
    }
}

/// Bangun `Expr` untuk literal desimal (sized maupun unsized).
///
/// `#[inline(never)]` + fungsi terpisah: `parse_primary_expr_impl` sudah
/// bingkai stack raksasa (rekursi parser pada ekspresi fuzz ber-kurung
/// dalam melimpahi 2 MB — multivec fuzz), semua lokal helper ini tidak
/// boleh menambah bingkai induknya.
///
/// Aturan lebar/signedness:
/// - Sized (`4'sd9`) wajib mempertahankan lebar & signedness — dulu masuk
///   `Value::Decimal` yang membuang keduanya sehingga dievaluasi sebagai
///   konstanta 32-bit (fuzzer signed_fuzz seed=30: `4'sd9 <<< b` menggeser
///   di 32 bit alih-alih 4).
/// - Unsized tetap `Decimal` bila muat i64 (semantik §6.8.1 ROUND 36:
///   unsized decimal = signed); melebihi itu → pola biner/hex presisi
///   penuh, BUKAN `Expr::Ident` (dulu literal raksasa diam-diam jadi
///   implicit net — fuzzer partsel seed=28).
#[inline(never)]
fn decimal_value_expr(value: &Symbol, width: Option<usize>, is_signed: bool) -> Expr {
    let digits = value.as_str();
    match (width, digits.parse::<u64>()) {
        (Some(_), Ok(v)) => Expr::Value(Value::Binary {
            bits: format!("{:b}", v),
            width,
            is_signed,
        }),
        (Some(_), Err(_)) => Expr::Value(Value::Binary {
            bits: dec_str_to_bits(digits),
            width,
            is_signed,
        }),
        (None, _) => match digits.parse::<i64>() {
            Ok(n) => Expr::Value(Value::Decimal(n)),
            // Melebihi i64: hex mempertahankan nilai u64 eksak; di luar u64
            // → biner arbitrary-precision. Tetap angka, bukan identifier.
            Err(_) if digits.parse::<u128>().is_ok() => Expr::Value(Value::Hex {
                bits: format!("{:x}", digits.parse::<u128>().unwrap_or(0)),
                width: None,
                is_signed: false,
            }),
            Err(_) => Expr::Value(Value::Binary {
                bits: dec_str_to_bits(digits),
                width: None,
                is_signed: false,
            }),
        },
    }
}
