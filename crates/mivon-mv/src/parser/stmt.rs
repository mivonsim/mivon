//! Parser — statement: blok, if/else, case (qualifier/casez/casex), loop
//! (for/while/do/repeat/forever/wait), event/delay/trigger, fork/join,
//! return/break/continue, var-decl, assert/assert-property, assignment
//! (`=`/`<=`/compound/incdec), escape hatch `@sv`. 1 file = 1 tanggung jawab.

use super::Parser;
use crate::ast::*;
use crate::lexer::Tok;
use crate::MvError;

impl Parser {
    /// F26: parse body `case (...)` setelah keyword — items `val: stmt`,
    /// `a, b: stmt`, dan `default: stmt`. `qual` = priority/unique/unique0
    /// (None utk biasa), `kind` = "case"/"casez"/"casex".
    pub(crate) fn parse_case_body(
        &mut self,
        qual: Option<String>,
        kind: String,
    ) -> Result<Stmt, MvError> {
        self.expect(&Tok::LParen)?;
        let expr = self.parse_expr()?;
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::LBrace)?;
        let mut items = Vec::new();
        let mut default: Option<Box<Stmt>> = None;
        while !self.eat(&Tok::RBrace) {
            if self.eat(&Tok::Default) {
                self.expect(&Tok::Colon)?;
                default = Some(Box::new(self.parse_stmt()?));
            } else {
                let mut vals = vec![self.parse_expr()?];
                while self.eat(&Tok::Comma) {
                    vals.push(self.parse_expr()?);
                }
                self.expect(&Tok::Colon)?;
                let body = self.parse_stmt()?;
                items.push((vals, body));
            }
        }
        Ok(Stmt::Case {
            expr,
            items,
            default,
            qual,
            kind,
        })
    }

    pub(crate) fn parse_stmt_block(&mut self) -> Result<Vec<Stmt>, MvError> {
        self.expect(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !self.eat(&Tok::RBrace) {
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    // ── Statements ──
    pub(crate) fn parse_stmt(&mut self) -> Result<Stmt, MvError> {
        match self.peek().clone() {
            Tok::LBrace => {
                let body = self.parse_stmt_block()?;
                Ok(Stmt::Block(body))
            }
            // F47: `label : { stmt* }` — blok bernama. Pola ident diikuti
            // `:` lalu `{` hanya valid di sini (case arm di-parse via
            // parse_case_body, labeled statement tidak didukung `.mv`).
            Tok::Ident(_)
                if matches!(self.peek_at(1), Tok::Colon)
                    && matches!(self.peek_at(2), Tok::LBrace) =>
            {
                let name = self.expect_ident()?;
                self.expect(&Tok::Colon)?;
                let stmts = self.parse_stmt_block()?;
                Ok(Stmt::NamedBlock { name, stmts })
            }
            Tok::If => {
                self.advance();
                self.expect(&Tok::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let then = self.parse_stmt()?;
                let els = if self.eat(&Tok::Else) {
                    Some(Box::new(self.parse_stmt()?))
                } else {
                    None
                };
                Ok(Stmt::If {
                    cond,
                    then: Box::new(then),
                    els,
                })
            }
            // F26: case qualifier + casez/casex — `priority case (...)`,
            // `unique casez (...)`, `casex (...)`. Qualifier/kind dibaca dulu,
            // lalu body case di-parse oleh parse_case_body.
            Tok::Priority | Tok::Unique | Tok::Unique0 => {
                let qual = match self.peek() {
                    Tok::Priority => "priority",
                    Tok::Unique => "unique",
                    _ => "unique0",
                }
                .to_string();
                self.advance();
                let kind = match self.peek() {
                    Tok::Case => "case",
                    Tok::Casez => "casez",
                    Tok::Casex => "casex",
                    _ => {
                        let (l, c) = self.pos_line();
                        return Err(MvError::new(
                            l,
                            c,
                            "diharapkan 'case'/'casez'/'casex' setelah qualifier".to_string(),
                        ));
                    }
                }
                .to_string();
                self.advance();
                self.parse_case_body(Some(qual), kind)
            }
            Tok::Casez | Tok::Casex => {
                let kind = match self.peek() {
                    Tok::Casez => "casez",
                    _ => "casex",
                }
                .to_string();
                self.advance();
                self.parse_case_body(None, kind)
            }
            Tok::Case => {
                self.advance();
                self.parse_case_body(None, "case".to_string())
            }
            Tok::For => {
                self.advance();
                let var = self.expect_ident()?;
                self.expect(&Tok::In)?;
                let from = self.parse_expr()?;
                self.expect(&Tok::DotDot)?;
                let to = self.parse_expr()?;
                let step = self.parse_optional_step()?;
                let body = self.parse_stmt()?;
                Ok(Stmt::For {
                    var,
                    from,
                    to,
                    step,
                    body: Box::new(body),
                })
            }
            Tok::While => {
                self.advance();
                self.expect(&Tok::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let body = self.parse_stmt()?;
                Ok(Stmt::While {
                    cond,
                    body: Box::new(body),
                })
            }
            // F38: `do { body } while (cond)` — loop post-test.
            Tok::Do => {
                self.advance();
                let body = self.parse_stmt()?;
                self.expect(&Tok::While)?;
                self.expect(&Tok::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                self.eat(&Tok::Semi);
                Ok(Stmt::DoWhile {
                    cond,
                    body: Box::new(body),
                })
            }
            // F38: event trigger `->ev` — memicu event named (emisi `-> ev;`).
            // Target HANYA ident: parser SV `Stmt::EventTrigger` menerima nama
            // ident saja (`expect_ident`), jadi `-> obj.sig`/`-> q[0]` ditolak
            // di level .mv dengan error jelas — bukan SV invalid hasil generate.
            Tok::Arrow => {
                self.advance();
                let (l, c) = self.pos_line();
                let name = self.expect_ident()?;
                self.eat(&Tok::Semi);
                Ok(Stmt::EventTrigger(Expr::Ident(name, l, c)))
            }
            // `foreach (arr[i]) { body }` — loop elemen array unpacked
            Tok::Foreach => {
                self.advance();
                self.expect(&Tok::LParen)?;
                let arr = self.expect_ident()?;
                let mut inds = Vec::new();
                while self.eat(&Tok::LBrack) {
                    inds.push(self.expect_ident()?);
                    self.expect(&Tok::RBrack)?;
                }
                self.expect(&Tok::RParen)?;
                let body = self.parse_stmt()?;
                Ok(Stmt::Foreach {
                    arr,
                    inds,
                    body: Box::new(body),
                })
            }
            // F39: `fork { stmt* } { stmt* } ... join / join_any / join_none`
            // — branch konkurren, masing-masing blok `{ ... }`. Diakhiri salah
            // satu keyword join (bukan `}` lagi).
            Tok::Fork => {
                self.advance();
                let mut branches = Vec::new();
                loop {
                    match self.peek() {
                        Tok::Join | Tok::JoinAny | Tok::JoinNone => break,
                        Tok::LBrace => {
                            let b = self.parse_stmt_block()?;
                            branches.push(Stmt::Block(b));
                        }
                        _ => {
                            let (l, c) = self.pos_line();
                            return Err(MvError::new(
                                l,
                                c,
                                "tiap branch fork harus blok '{ ... }' — diakhiri 'join'/'join_any'/'join_none'".to_string(),
                            ));
                        }
                    }
                }
                let join = match self.peek() {
                    Tok::Join => {
                        self.advance();
                        ForkJoin::Join
                    }
                    Tok::JoinAny => {
                        self.advance();
                        ForkJoin::JoinAny
                    }
                    Tok::JoinNone => {
                        self.advance();
                        ForkJoin::JoinNone
                    }
                    _ => {
                        let (l, c) = self.pos_line();
                        return Err(MvError::new(
                            l,
                            c,
                            "diharapkan 'join' / 'join_any' / 'join_none' setelah blok fork"
                                .to_string(),
                        ));
                    }
                };
                Ok(Stmt::Fork { branches, join })
            }
            Tok::Repeat => {
                self.advance();
                self.expect(&Tok::LParen)?;
                let count = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let body = self.parse_stmt()?;
                Ok(Stmt::Repeat {
                    count,
                    body: Box::new(body),
                })
            }
            Tok::Forever => {
                self.advance();
                let body = self.parse_stmt()?;
                Ok(Stmt::Forever(Box::new(body)))
            }
            Tok::Wait => {
                self.advance();
                // F45: `wait fork;` — varian khusus tanpa paren (seperti SV).
                if matches!(self.peek(), Tok::Fork) {
                    self.advance();
                    self.eat(&Tok::Semi);
                    return Ok(Stmt::WaitFork);
                }
                self.expect(&Tok::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let body = self.parse_stmt()?;
                Ok(Stmt::Wait {
                    cond,
                    body: Box::new(body),
                })
            }
            // F45: `disable fork;` / `disable <label>;`
            Tok::Disable => {
                self.advance();
                let name = if matches!(self.peek(), Tok::Fork) {
                    self.advance();
                    "fork".to_string()
                } else {
                    self.expect_ident()?
                };
                self.eat(&Tok::Semi);
                Ok(Stmt::Disable { name })
            }
            // F46: `force lhs = rhs;` — lhs pola lvalue (ident/member/index).
            Tok::Force => {
                self.advance();
                let (l, c) = self.pos_line();
                let lhs = self.parse_postfix_expr_stmt()?;
                self.expect(&Tok::BlockingAssign)?;
                let rhs = self.parse_expr()?;
                self.eat(&Tok::Semi);
                Ok(Stmt::Force { lhs, rhs, line: l, col: c })
            }
            // F46: `release target;`
            Tok::Release => {
                self.advance();
                let target = self.parse_postfix_expr_stmt()?;
                self.eat(&Tok::Semi);
                Ok(Stmt::Release { target })
            }
            Tok::At => {
                self.advance();
                self.expect(&Tok::LParen)?;
                let expr = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                // Body opsional: `@(event)` tanpa statement berikutnya = event
                // control statement tunggal (SV: `repeat (n) @(posedge clk);`).
                let body = if matches!(
                    self.peek(),
                    Tok::RBrace | Tok::Semi | Tok::Else
                ) {
                    None
                } else {
                    Some(Box::new(self.parse_stmt()?))
                };
                Ok(Stmt::Event { expr, body })
            }
            Tok::Hash => {
                self.advance();
                let amt = self.parse_expr()?;
                // Delay tanpa body (`#100` di akhir method/block) = delay-only
                let body = if matches!(self.peek(), Tok::RBrace) {
                    Stmt::Block(Vec::new())
                } else {
                    self.parse_stmt()?
                };
                Ok(Stmt::Delay {
                    amt,
                    body: Box::new(body),
                })
            }
            Tok::Return => {
                self.advance();
                let v = if matches!(self.peek(), Tok::RBrace) || matches!(self.peek(), Tok::Semi) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                self.eat(&Tok::Semi);
                Ok(Stmt::Return(v))
            }
            Tok::Break => {
                self.advance();
                self.eat(&Tok::Semi);
                Ok(Stmt::Break)
            }
            Tok::Continue => {
                self.advance();
                self.eat(&Tok::Semi);
                Ok(Stmt::Continue)
            }
            Tok::Var => {
                self.advance();
                let mut names = vec![self.expect_ident()?];
                while self.eat(&Tok::Comma) {
                    names.push(self.expect_ident()?);
                }
                self.expect(&Tok::Colon)?;
                let ty = self.parse_type()?;
                let init = if self.eat(&Tok::BlockingAssign) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                Ok(Stmt::VarDecl { names, ty, init })
            }
            Tok::Assert => {
                self.advance();
                // `assert property (...)` — concurrent assertion. Body diambil
                // RAW (teks persis `(...)`) karena berisi operator SVA (`|->`,
                // `##`, `[*]`) yang bukan token .mv. Emisi 1:1 (MIVON-HDL.md §7.2).
                if self.is_ident("property") {
                    self.advance();
                    let (sl, sc) = self.pos_line(); // posisi `(`
                    self.expect(&Tok::LParen)?;
                    let mut depth = 1usize;
                    loop {
                        match self.peek() {
                            Tok::Eof => {
                                let (l, c) = self.pos_line();
                                return Err(MvError::new(
                                    l,
                                    c,
                                    "assert property tidak ditutup".to_string(),
                                ));
                            }
                            Tok::LParen => {
                                depth += 1;
                                self.advance();
                            }
                            Tok::RParen => {
                                let (l, c) = self.pos_line();
                                depth -= 1;
                                self.advance();
                                if depth == 0 {
                                    let raw = self.raw_slice(sl, sc, l, c + 1);
                                    return Ok(Stmt::AssertProperty(raw));
                                }
                            }
                            _ => {
                                self.advance();
                            }
                        }
                    }
                }
                self.expect(&Tok::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let pass = if matches!(self.peek(), Tok::Else) || matches!(self.peek(), Tok::RBrace)
                {
                    None
                } else {
                    Some(Box::new(self.parse_stmt()?))
                };
                let fail = if self.eat(&Tok::Else) {
                    Some(Box::new(self.parse_stmt()?))
                } else {
                    None
                };
                Ok(Stmt::Assert { cond, pass, fail })
            }
            // F37: prefix `++lhs` / `--lhs` di level statement. Hasil sama
            // dengan postfix (`lhs++`); di-emit sesuai aslinya.
            Tok::PlusPlus | Tok::MinusMinus => {
                let inc = matches!(self.peek(), Tok::PlusPlus);
                let (l, c) = self.pos_line();
                self.advance();
                let lhs = self.parse_postfix_expr()?;
                Ok(Stmt::IncDec {
                    lhs,
                    inc,
                    pre: true,
                    line: l,
                    col: c,
                })
            }
            // Escape hatch `@sv { ... }` — body sudah jadi satu token RawSvh
            // (di-level lexer); emit verbatim.
            Tok::RawSvh(_) => {
                let (l, c) = self.pos_line();
                match self.peek().clone() {
                    Tok::RawSvh(body) => {
                        self.advance();
                        Ok(Stmt::RawSvh(body))
                    }
                    _ => Err(MvError::new(l, c, "RawSvh tanpa body".to_string())),
                }
            }
            _ => self.parse_assign_or_expr(),
        }
    }

    /// Statement di level statement: assignment (`=`/`<=`) atau ekspresi.
    pub(crate) fn parse_assign_or_expr(&mut self) -> Result<Stmt, MvError> {
        // Cek pola lvalue: Ident (postfix)* diikuti `=` / `<=`
        if matches!(self.peek(), Tok::Ident(_)) {
            // F11: posisi statement untuk error assignment (E2002/E2003/E2004)
            let (l, c) = self.pos_line();
            // Simpan posisi untuk backtrack
            let save = self.pos;
            // F37: varian stmt — lhs boleh postfix `i++` (arm F36 di bawah),
            // ekspresi RHS (`j = i++`) tetap ditolak.
            let lhs = match self.parse_postfix_expr_stmt() {
                Ok(e) => e,
                Err(_) => {
                    self.pos = save;
                    let e = self.parse_expr()?;
                    return Ok(Stmt::ExprStmt(e));
                }
            };
            match self.peek().clone() {
                Tok::BlockingAssign => {
                    self.advance();
                    let rhs = self.parse_expr()?;
                    return Ok(Stmt::Assign {
                        lhs,
                        rhs,
                        nba: false,
                        line: l,
                        col: c,
                    });
                }
                Tok::NonBlockingAssign => {
                    self.advance();
                    let rhs = self.parse_expr()?;
                    return Ok(Stmt::Assign {
                        lhs,
                        rhs,
                        nba: true,
                        line: l,
                        col: c,
                    });
                }
                // F36: postfix `lhs++` / `lhs--`
                Tok::PlusPlus | Tok::MinusMinus => {
                    // F37 fix: guard baris — `++`/`--` di baris BERBEDA dari
                    // akhir lhs = statement prefix baru (`--i` setelah
                    // `$display(...)`), bukan postfix dari statement ini.
                    let end_line =
                        self.toks[(self.pos.saturating_sub(1)).min(self.toks.len() - 1)].1;
                    if self.toks[self.pos].1 != end_line {
                        self.pos = save;
                        let e = self.parse_expr()?;
                        return Ok(Stmt::ExprStmt(e));
                    }
                    let inc = matches!(self.peek(), Tok::PlusPlus);
                    self.advance();
                    return Ok(Stmt::IncDec {
                        lhs,
                        inc,
                        pre: false,
                        line: l,
                        col: c,
                    });
                }
                // F36: compound `lhs += rhs` dst.
                Tok::PlusEq
                | Tok::MinusEq
                | Tok::StarEq
                | Tok::SlashEq
                | Tok::PercentEq
                | Tok::ShlEq
                | Tok::SshrEq
                | Tok::AndEq
                | Tok::OrEq
                | Tok::XorEq => {
                    let op = match self.peek().clone() {
                        Tok::PlusEq => "+=".to_string(),
                        Tok::MinusEq => "-=".to_string(),
                        Tok::StarEq => "*=".to_string(),
                        Tok::SlashEq => "/=".to_string(),
                        Tok::PercentEq => "%=".to_string(),
                        Tok::ShlEq => "<<=".to_string(),
                        Tok::SshrEq => ">>=".to_string(),
                        Tok::AndEq => "&=".to_string(),
                        Tok::OrEq => "|=".to_string(),
                        Tok::XorEq => "^=".to_string(),
                        _ => unreachable!(),
                    };
                    self.advance();
                    let rhs = self.parse_expr()?;
                    return Ok(Stmt::CompoundAssign {
                        lhs,
                        rhs,
                        op,
                        line: l,
                        col: c,
                    });
                }
                _ => {
                    self.pos = save;
                    let e = self.parse_expr()?;
                    return Ok(Stmt::ExprStmt(e));
                }
            }
        }
        let e = self.parse_expr()?;
        Ok(Stmt::ExprStmt(e))
    }
}
