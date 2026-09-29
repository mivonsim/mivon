//! Parser — ekspresi (Pratt precedence) + tipe. 1 file = 1 tanggung jawab.

use super::Parser;
use crate::ast::*;
use crate::lexer::Tok;
use crate::MvError;

impl Parser {
    // ── Types ──
    pub(crate) fn parse_type(&mut self) -> Result<MvType, MvError> {
        let mut t = self.parse_base_type()?;
        // unpacked dims: `Type[N][M]` / queue `Type[$]`
        while self.eat(&Tok::LBrack) {
            // F40: `[$]` = queue (dynamic array) — SV hanya mengizinkan
            // queue sbg dim terakhir → break setelah terbaca.
            if self.peek() == &Tok::Dollar {
                self.advance();
                self.expect(&Tok::RBrack)?;
                t = MvType::Queue(Box::new(t));
                break;
            }
            let d = self.parse_expr()?;
            self.expect(&Tok::RBrack)?;
            t = MvType::Array(Box::new(t), vec![d]);
        }
        Ok(t)
    }

    pub(crate) fn parse_base_type(&mut self) -> Result<MvType, MvError> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                let (l, c) = self.pos_line();
                let s = s.clone();
                self.advance();
                // `logic[...]`
                if s == "logic" {
                    let range = if self.eat(&Tok::LBrack) {
                        let a = self.parse_expr()?;
                        if self.eat(&Tok::Colon) {
                            let b = self.parse_expr()?;
                            self.expect(&Tok::RBrack)?;
                            Some((a, b))
                        } else {
                            self.expect(&Tok::RBrack)?;
                            // `logic[N]` → `[N-1:0]`
                            Some((sub_one(a), Expr::Int(0)))
                        }
                    } else {
                        None
                    };
                    Ok(MvType::Logic(range))
                } else if s == "bit" {
                    Ok(MvType::Bit)
                } else if s == "int" {
                    Ok(MvType::Int)
                } else if s == "uint" {
                    Ok(MvType::Uint)
                } else if s == "longint" {
                    Ok(MvType::LongInt)
                } else if s == "ulongint" {
                    Ok(MvType::ULongInt)
                } else if s == "shortint" {
                    Ok(MvType::ShortInt)
                } else if s == "byte" {
                    Ok(MvType::Byte)
                } else if s == "real" {
                    Ok(MvType::Real)
                } else if s == "time" {
                    Ok(MvType::Time)
                } else if s == "string" {
                    Ok(MvType::Str)
                } else if s == "signed" {
                    let inner = self.parse_base_type()?;
                    Ok(MvType::Signed(Box::new(inner)))
                } else {
                    // User-defined type `State`, `Packet`, `Addr`
                    // scoped: `pkg::Type`
                    if self.eat(&Tok::Scope) {
                        let item = self.expect_ident()?;
                        Ok(MvType::Named(format!("{}::{}", s, item), l, c))
                    } else {
                        Ok(MvType::Named(s, l, c))
                    }
                }
            }
            Tok::Type => {
                // F32: kata kunci `type` sbg tipe — marker type parameter
                // (`T : type = logic[7:0]`). Bukan tipe nyata: validasi &
                // emisi ditangani khusus di check.rs / codegen.rs.
                let (l, c) = self.pos_line();
                self.advance();
                Ok(MvType::Named("type".into(), l, c))
            }
            _ => {
                let (l, c) = self.pos_line();
                Err(MvError::new(
                    l,
                    c,
                    format!("tipe tidak dikenal: {:?}", self.peek()),
                ))
            }
        }
    }

    // ── Expressions (Pratt) ──
    pub(crate) fn parse_expr(&mut self) -> Result<Expr, MvError> {
        self.parse_binary(1)
    }

    /// Tabel precedence: 1 = loosest, 12 = tightest (binary).
    pub(crate) fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, MvError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, prec) = match self.peek().clone() {
                Tok::PipePipe => ("||", 1),
                Tok::AmpAmp => ("&&", 2),
                Tok::Pipe => ("|", 3),
                Tok::Caret => ("^", 4),
                Tok::Amp => ("&", 5),
                Tok::Eq => ("==", 6),
                Tok::Neq => ("!=", 6),
                Tok::CaseEq => ("===", 6),
                Tok::CaseNeq => ("!==", 6),
                // `<=` di dalam ekspresi = relasional; di statement level
                // parse_assign_or_expr menangkapnya sebagai non-blocking.
                Tok::Lt => ("<", 7),
                Tok::NonBlockingAssign => ("<=", 7),
                Tok::Gt => (">", 7),
                Tok::Ge => (">=", 7),
                Tok::Shl => ("<<", 8),
                Tok::Shr => (">>", 8),
                Tok::Sshl => ("<<<", 8),
                Tok::Sshr => (">>>", 8),
                Tok::Plus => ("+", 9),
                Tok::Minus => ("-", 9),
                Tok::Star => ("*", 10),
                Tok::Slash => ("/", 10),
                Tok::Percent => ("%", 10),
                Tok::Power => ("**", 11),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.advance();
            let rhs = self.parse_binary(prec + 1)?;
            lhs = Expr::Binary(op.into(), Box::new(lhs), Box::new(rhs));
        }
        // `lhs inside { set }` / `lhs dist { items }` (F12) — hanya di level
        // ekspresi paling luar (min_prec 1). Set/dist bisa berisi range
        // `[lo:hi]` yang bukan ekspresi biasa.
        if min_prec <= 1 && self.eat(&Tok::Inside) {
            self.expect(&Tok::LBrace)?;
            let mut items = Vec::new();
            while !self.eat(&Tok::RBrace) {
                if matches!(self.peek(), Tok::LBrack) {
                    self.advance();
                    let lo = self.parse_expr()?;
                    self.expect(&Tok::Colon)?;
                    let hi = self.parse_expr()?;
                    self.expect(&Tok::RBrack)?;
                    items.push(InsideItem::Range(lo, hi));
                } else {
                    items.push(InsideItem::Value(self.parse_expr()?));
                }
                self.eat(&Tok::Comma);
            }
            lhs = Expr::Inside {
                expr: Box::new(lhs),
                items,
            };
        }
        if min_prec <= 1 && self.eat(&Tok::Dist) {
            self.expect(&Tok::LBrace)?;
            let mut items = Vec::new();
            while !self.eat(&Tok::RBrace) {
                items.push(self.parse_dist_item()?);
                self.eat(&Tok::Comma);
            }
            lhs = Expr::Dist {
                expr: Box::new(lhs),
                items,
            };
        }
        // Ternary
        if min_prec <= 1 && self.eat(&Tok::Question) {
            let then = self.parse_expr()?;
            self.expect(&Tok::Colon)?;
            let els = self.parse_expr()?;
            lhs = Expr::Ternary(Box::new(lhs), Box::new(then), Box::new(els));
        }
        Ok(lhs)
    }

    pub(crate) fn parse_unary(&mut self) -> Result<Expr, MvError> {
        match self.peek().clone() {
            Tok::Not => {
                self.advance();
                // Operand unary di-parse dengan min_prec 11 agar `**` (prec 11)
                // terikat lebih erat dari unary: `-a ** b` = `-(a ** b)` (SV).
                let e = self.parse_binary(11)?;
                Ok(Expr::Unary("!".into(), Box::new(e)))
            }
            Tok::Tilde => {
                self.advance();
                let e = self.parse_binary(11)?;
                Ok(Expr::Unary("~".into(), Box::new(e)))
            }
            Tok::Minus => {
                self.advance();
                let e = self.parse_binary(11)?;
                Ok(Expr::Unary("-".into(), Box::new(e)))
            }
            Tok::Plus => {
                self.advance();
                let e = self.parse_binary(11)?;
                Ok(Expr::Unary("+".into(), Box::new(e)))
            }
            Tok::Amp => {
                self.advance();
                let e = self.parse_binary(11)?;
                Ok(Expr::Unary("&".into(), Box::new(e)))
            }
            Tok::Pipe => {
                self.advance();
                let e = self.parse_binary(11)?;
                Ok(Expr::Unary("|".into(), Box::new(e)))
            }
            Tok::Caret => {
                self.advance();
                let e = self.parse_binary(11)?;
                Ok(Expr::Unary("^".into(), Box::new(e)))
            }
            // F37: prefix `++x` / `--x` di level EKSPRESI (RHS): `j = ++i`.
            // Operand di-parse postfix (memakan `[i]`/`.f`/`(args)` bila ada).
            Tok::PlusPlus | Tok::MinusMinus => {
                let inc = matches!(self.peek(), Tok::PlusPlus);
                self.advance();
                let e = self.parse_postfix_expr()?;
                Ok(Expr::IncDec {
                    inc,
                    pre: true,
                    expr: Box::new(e),
                })
            }
            _ => self.parse_postfix_expr(),
        }
    }

    pub(crate) fn parse_postfix_expr(&mut self) -> Result<Expr, MvError> {
        self.parse_postfix_expr_inner(false)
    }

    /// F37: varian utk lhs statement — postfix `++`/`--` DIPERBOLEHKAN
    /// (`i++` baris sendiri); ekspresi RHS (`j = i++`) tetap ditolak.
    /// Postfix di-`break`, pemanggil (parse_assign_or_expr arm F36) yang
    /// mengubahnya jadi `Stmt::IncDec`.
    pub(crate) fn parse_postfix_expr_stmt(&mut self) -> Result<Expr, MvError> {
        self.parse_postfix_expr_inner(true)
    }

    pub(crate) fn parse_postfix_expr_inner(
        &mut self,
        allow_postfix: bool,
    ) -> Result<Expr, MvError> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek().clone() {
                Tok::LBrack => {
                    self.advance();
                    let a = self.parse_expr()?;
                    if self.eat(&Tok::Colon) {
                        let b = self.parse_expr()?;
                        self.expect(&Tok::RBrack)?;
                        e = Expr::Range(Box::new(e), Box::new(a), Box::new(b));
                    } else {
                        self.expect(&Tok::RBrack)?;
                        e = Expr::Index(Box::new(e), Box::new(a));
                    }
                }
                Tok::Dot => {
                    let (l, c) = self.pos_line();
                    self.advance();
                    let f = self.expect_ident()?;
                    e = Expr::Member(Box::new(e), f, l, c);
                }
                Tok::LParen => {
                    self.advance();
                    let mut args = Vec::new();
                    while !self.eat(&Tok::RParen) {
                        args.push(self.parse_call_arg()?);
                        self.eat(&Tok::Comma);
                    }
                    match e {
                        Expr::Ident(name, ..) => e = Expr::Call(name, args),
                        Expr::Scoped(p, i, ..) => e = Expr::Call(format!("{}::{}", p, i), args),
                        // method call `obj.method(args)` — termasuk `this`/`super`
                        Expr::Member(obj, method, ..) => e = Expr::MethodCall { obj, method, args },
                        Expr::MethodCall { obj, method, .. } => {
                            e = Expr::MethodCall { obj, method, args }
                        }
                        _ => {
                            let (l, c) = self.pos_line();
                            return Err(MvError::new(
                                l,
                                c,
                                "hanya fungsi yang bisa dipanggil".to_string(),
                            ));
                        }
                    }
                }
                // F37: postfix `x++`/`x--` di akhir ekspresi.
                Tok::PlusPlus | Tok::MinusMinus => {
                    // Guard baris: `++`/`--` di baris BERBEDA dari akhir
                    // ekspresi = statement prefix baru (`++i` setelah
                    // `$display(...)`), bukan postfix — break biar statement
                    // berikutnya yang menanganinya.
                    let end_line =
                        self.toks[(self.pos.saturating_sub(1)).min(self.toks.len() - 1)].1;
                    if self.toks[self.pos].1 != end_line {
                        break;
                    }
                    if allow_postfix {
                        // lhs statement: serahkan ke parse_assign_or_expr
                        // (arm F36) yang mengubahnya jadi Stmt::IncDec.
                        break;
                    }
                    // RHS ekspresi: postfix TIDAK didukung di .mv (side-effect
                    // dalam ekspresi tidak bisa diwakili SV) — error jelas di
                    // level .mv, bukan menghasilkan SV invalid.
                    let (l, c) = self.pos_line();
                    let op = if matches!(self.peek(), Tok::PlusPlus) {
                        "++"
                    } else {
                        "--"
                    };
                    return Err(MvError::new(
                        l,
                        c,
                        format!("postfix {op} hanya didukung sebagai statement (baris sendiri), bukan di dalam ekspresi"),
                    ));
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// Argumen call: normal ekspresi, atau NAMED `name = expr` (Ident + `=`)
    /// → `Expr::NamedArg` (emisi SV `.name(expr)`). Assignment `=` bukan
    /// ekspresi valid di argumen — deteksi aman.
    pub(crate) fn parse_call_arg(&mut self) -> Result<Expr, MvError> {
        if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::BlockingAssign {
            let name = self.expect_ident()?;
            self.advance(); // `=`
            let expr = self.parse_expr()?;
            return Ok(Expr::NamedArg {
                name,
                expr: Box::new(expr),
            });
        }
        self.parse_expr()
    }

    pub(crate) fn parse_primary(&mut self) -> Result<Expr, MvError> {
        match self.peek().clone() {
            Tok::Int(v) => {
                self.advance();
                Ok(Expr::Int(v))
            }
            Tok::Sized(w, b, d) => {
                let (l, c) = self.pos_line();
                self.advance();
                Ok(Expr::Sized(w, b, d, l, c))
            }
            Tok::Real(v) => {
                self.advance();
                Ok(Expr::Real(v))
            }
            Tok::Fill(c) => {
                self.advance();
                Ok(Expr::Fill(c))
            }
            Tok::Str(s) => {
                self.advance();
                Ok(Expr::Str(s))
            }
            Tok::Ident(s) => {
                let (l, c) = self.pos_line();
                // F33: type cast `T'(expr)` / `logic'(x)` / `Word16'(x)` /
                // `pkg::T'(x)` — ident (tipe) diikuti Quote. parse_base_type
                // menangani tipe dasar, user-defined, dan scoped `pkg::T`.
                if self.peek_at(1) == &Tok::Quote {
                    let ty = self.parse_base_type()?;
                    self.expect(&Tok::Quote)?;
                    self.expect(&Tok::LParen)?;
                    let e = self.parse_expr()?;
                    self.expect(&Tok::RParen)?;
                    return Ok(Expr::Cast {
                        ty: Box::new(ty),
                        expr: Box::new(e),
                        line: l,
                        col: c,
                    });
                }
                let s = s.clone();
                self.advance();
                // scoped `pkg::item`
                if self.eat(&Tok::Scope) {
                    let item = self.expect_ident()?;
                    Ok(Expr::Scoped(s, item, l, c))
                } else {
                    Ok(Expr::Ident(s, l, c))
                }
            }
            // `@(posedge clk)` — edge diwakili unary "posedge"/"negedge"
            Tok::PosEdge | Tok::NegEdge => {
                let neg = matches!(self.peek(), Tok::NegEdge);
                self.advance();
                let inner = self.parse_primary()?;
                let op = if neg { "negedge" } else { "posedge" };
                Ok(Expr::Unary(op.into(), Box::new(inner)))
            }
            Tok::LParen => {
                self.advance();
                let e = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                Ok(Expr::Paren(Box::new(e)))
            }
            // Array literal `'{e0, e1, ...}` — assignment pattern unpacked
            // array (ROM/LUT). `Quote` utk cast `T'(x)` ditangani di arm
            // Ident; di sini `'` diikuti `{`.
            Tok::Quote => {
                self.advance();
                if self.peek() != &Tok::LBrace {
                    // Quote tanpa `{` (mis. `logic[7:0]'(a)` tersisa `'(`) —
                    // pesan error lama (F33): "ekspresi tidak valid".
                    let (l, c) = self.pos_line();
                    return Err(MvError::new(
                        l,
                        c,
                        format!("ekspresi tidak valid: {:?}", self.peek()),
                    ));
                }
                self.advance();
                let mut items = Vec::new();
                while !self.eat(&Tok::RBrace) {
                    items.push(self.parse_expr()?);
                    self.eat(&Tok::Comma);
                }
                Ok(Expr::ArrayLit(items))
            }
            Tok::LBrace => {
                // concat `{a, b}` atau replication `{n{a}}`
                self.advance();
                let first = self.parse_expr()?;
                if self.eat(&Tok::LBrace) {
                    // replication `{n{expr}}`
                    let inner = self.parse_expr()?;
                    self.expect(&Tok::RBrace)?;
                    self.expect(&Tok::RBrace)?;
                    Ok(Expr::Replicate(Box::new(first), Box::new(inner)))
                } else {
                    let mut parts = vec![first];
                    while self.eat(&Tok::Comma) {
                        parts.push(self.parse_expr()?);
                    }
                    self.expect(&Tok::RBrace)?;
                    Ok(Expr::Concat(parts))
                }
            }
            _ => {
                let (l, c) = self.pos_line();
                Err(MvError::new(
                    l,
                    c,
                    format!("ekspresi tidak valid: {:?}", self.peek()),
                ))
            }
        }
    }
}

/// `expr - 1` (untuk `logic[N]` → `[N-1:0]`). Konstanta langsung di-fold.
pub(super) fn sub_one(e: Expr) -> Expr {
    match e {
        Expr::Int(v) => Expr::Int(v - 1),
        other => Expr::Binary("-".into(), Box::new(other), Box::new(Expr::Int(1))),
    }
}
