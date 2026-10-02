//! Parser — level module: module/program, param, port, item, seq spec, inst,
//! interface/modport, generate (for/if). 1 file = 1 tanggung jawab.

use super::Parser;
use crate::ast::*;
use crate::lexer::Tok;
use crate::MvError;

impl Parser {
    // ── Interface (MIVON-HDL.md §6.10) ──
    /// `interface name { in/out ports, sig, modport }` — definisi bersama
    /// yang di-emit ke `.svh`. Body: port (`in clk : bit`) & `sig x : T`
    /// sama-sama jadi deklarasi signal interface; `modport` menamai subset
    /// signal dengan arah untuk view koneksi.
    pub(crate) fn parse_interface(&mut self) -> Result<Interface, MvError> {
        self.expect(&Tok::Interface)?;
        let (l, c) = self.pos_line();
        let name = self.expect_ident()?;
        self.expect(&Tok::LBrace)?;
        let mut ports = Vec::new();
        let mut sigs = Vec::new();
        let mut modports = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::RBrace => {
                    self.advance();
                    break;
                }
                Tok::Eof => {
                    let (l, c) = self.pos_line();
                    return Err(MvError::new(
                        l,
                        c,
                        "interface tidak ditutup dengan '}'".to_string(),
                    ));
                }
                Tok::In | Tok::Out | Tok::Inout => {
                    let (pl, pc) = self.pos_line();
                    let dir = match self.peek().clone() {
                        Tok::In => Dir::In,
                        Tok::Out => Dir::Out,
                        _ => Dir::Inout,
                    };
                    self.advance();
                    let mut names = vec![self.expect_ident()?];
                    while self.eat(&Tok::Comma) {
                        names.push(self.expect_ident()?);
                    }
                    self.expect(&Tok::Colon)?;
                    let ty = self.parse_type()?;
                    ports.push(Port {
                        dir,
                        names,
                        ty,
                        line: pl,
                        col: pc,
                    });
                }
                Tok::Sig => {
                    let (sl, sc) = self.pos_line();
                    self.advance();
                    let mut names = vec![self.expect_ident()?];
                    while self.eat(&Tok::Comma) {
                        names.push(self.expect_ident()?);
                    }
                    self.expect(&Tok::Colon)?;
                    let ty = self.parse_type()?;
                    sigs.push((names, ty, sl, sc));
                }
                Tok::Modport => modports.push(self.parse_modport()?),
                _ => {
                    let (l, c) = self.pos_line();
                    return Err(MvError::new(
                        l,
                        c,
                        format!("item interface tidak dikenal: {:?}", self.peek()),
                    ));
                }
            }
        }
        Ok(Interface {
            name,
            ports,
            sigs,
            modports,
            line: l,
            col: c,
        })
    }

    /// `modport slave { in a, b; out c }` — deklarasi arah baru dimulai oleh
    /// keyword `in`/`out`/`inout` (nama signal tidak mungkin keyword),
    /// sehingga baris baru tanpa `;` juga ter-parse dengan benar.
    pub(crate) fn parse_modport(&mut self) -> Result<Modport, MvError> {
        self.expect(&Tok::Modport)?;
        let (l, c) = self.pos_line();
        let name = self.expect_ident()?;
        self.expect(&Tok::LBrace)?;
        let mut dirs = Vec::new();
        loop {
            let dir = match self.peek().clone() {
                Tok::In => {
                    self.advance();
                    Dir::In
                }
                Tok::Out => {
                    self.advance();
                    Dir::Out
                }
                Tok::Inout => {
                    self.advance();
                    Dir::Inout
                }
                _ => break,
            };
            let mut names = vec![self.expect_ident()?];
            while self.eat(&Tok::Comma) {
                names.push(self.expect_ident()?);
            }
            dirs.push((dir, names));
        }
        self.expect(&Tok::RBrace)?;
        Ok(Modport {
            name,
            dirs,
            line: l,
            col: c,
        })
    }

    // ── Module ──
    /// `program name { ... }` (MIVON-HDL.md §7.3) — testbench program.
    /// Body memakai item module (port, initial, ...) — struktur reuse `Module`.
    pub(crate) fn parse_program(&mut self) -> Result<Module, MvError> {
        self.advance(); // `program` (Ident)
        let (l, c) = self.pos_line();
        let name = self.expect_ident()?;
        self.expect(&Tok::LBrace)?;
        let mut items = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::RBrace => {
                    self.advance();
                    break;
                }
                Tok::Eof => {
                    let (l, c) = self.pos_line();
                    return Err(MvError::new(
                        l,
                        c,
                        "program tidak ditutup dengan '}'".to_string(),
                    ));
                }
                _ => items.push(self.parse_module_item()?),
            }
        }
        Ok(Module {
            name,
            params: Vec::new(),
            items,
            line: l,
            col: c,
        })
    }

    pub(crate) fn parse_module(&mut self) -> Result<Module, MvError> {
        self.expect(&Tok::Module)?;
        let (l, c) = self.pos_line();
        let name = self.expect_ident()?;
        let params = if self.eat(&Tok::Hash) {
            self.expect(&Tok::LParen)?;
            let mut ps = Vec::new();
            while !self.eat(&Tok::RParen) {
                ps.push(self.parse_param()?);
                self.eat(&Tok::Comma);
            }
            ps
        } else {
            Vec::new()
        };
        self.expect(&Tok::LBrace)?;
        let mut items = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::RBrace => {
                    self.advance();
                    break;
                }
                Tok::Eof => {
                    let (l, c) = self.pos_line();
                    return Err(MvError::new(
                        l,
                        c,
                        "module tidak ditutup dengan '}'".to_string(),
                    ));
                }
                _ => items.push(self.parse_module_item()?),
            }
        }
        Ok(Module {
            name,
            params,
            items,
            line: l,
            col: c,
        })
    }

    pub(crate) fn parse_param(&mut self) -> Result<Param, MvError> {
        let (l, c) = self.pos_line();
        // F32: bentuk (B) `type T = logic[7:0]` — kata kunci `type` di awal
        // (paralel dgn bentuk (A) `T : type = logic[7:0]`).
        let is_type_kw = self.peek() == &Tok::Type;
        if is_type_kw {
            self.advance();
        }
        let name = self.expect_ident()?;
        let ty = if self.eat(&Tok::Colon) {
            Some(self.parse_type()?)
        } else if is_type_kw {
            // F32 fix review: bentuk (B) `type T = ...` — set marker agar
            // check/codegen/collect_ctx konsisten (tanpa ini ty=None →
            // is_tp false bila tak ada default → `sig x : T` E2005).
            Some(MvType::Named("type".into(), l, c))
        } else {
            None
        };
        let is_type_marker = matches!(&ty, Some(MvType::Named(s, ..)) if s == "type");
        let (default, type_default) = if self.eat(&Tok::BlockingAssign) {
            if is_type_kw || is_type_marker {
                // type param: default adalah TIPE, bukan ekspresi nilai
                (None, Some(self.parse_type()?))
            } else {
                (Some(self.parse_expr()?), None)
            }
        } else {
            (None, None)
        };
        Ok(Param {
            name,
            ty,
            default,
            type_default,
            line: l,
            col: c,
        })
    }

    pub(crate) fn parse_module_item(&mut self) -> Result<MItem, MvError> {
        match self.peek().clone() {
            Tok::In | Tok::Out | Tok::Inout => {
                let (l, c) = self.pos_line();
                let dir = match self.peek().clone() {
                    Tok::In => Dir::In,
                    Tok::Out => Dir::Out,
                    _ => Dir::Inout,
                };
                self.advance();
                let mut names = vec![self.expect_ident()?];
                while self.eat(&Tok::Comma) {
                    names.push(self.expect_ident()?);
                }
                self.expect(&Tok::Colon)?;
                let ty = self.parse_type()?;
                Ok(MItem::Port(Port {
                    dir,
                    names,
                    ty,
                    line: l,
                    col: c,
                }))
            }
            Tok::Sig | Tok::Reg => {
                let (l, c) = self.pos_line();
                let is_reg = matches!(self.peek(), Tok::Reg);
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
                if is_reg {
                    Ok(MItem::Reg {
                        names,
                        ty,
                        init,
                        line: l,
                        col: c,
                    })
                } else {
                    Ok(MItem::Sig {
                        names,
                        ty,
                        init,
                        line: l,
                        col: c,
                    })
                }
            }
            Tok::Const => {
                let (l, c) = self.pos_line();
                self.advance();
                let name = self.expect_ident()?;
                let ty = if self.eat(&Tok::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.expect(&Tok::BlockingAssign)?;
                let value = self.parse_expr()?;
                Ok(MItem::Const {
                    name,
                    ty,
                    value,
                    line: l,
                    col: c,
                })
            }
            // typedef lokal module: `type X = ...` / `packed struct S` /
            // `enum E` / `union U` di dalam badan module.
            Tok::Type => Ok(MItem::Typedef(self.parse_typedef_alias()?)),
            Tok::Packed | Tok::Struct | Tok::Enum | Tok::Union => {
                Ok(MItem::Typedef(self.parse_typedef()?))
            }
            Tok::Use => {
                self.advance();
                let pkg = self.expect_ident()?;
                self.expect(&Tok::Scope)?;
                let item = if self.eat(&Tok::Star) {
                    "*".to_string()
                } else {
                    self.expect_ident()?
                };
                Ok(MItem::Use { pkg, item })
            }
            // `assert property ( … )` di level module — concurrent assertion
            // (LRM 1800 §14: module item, BUKAN statement prosedural).
            Tok::Assert => {
                self.advance();
                if self.is_ident("property") {
                    self.advance();
                    let raw = self.parse_assert_property_raw()?;
                    return Ok(MItem::AssertProperty(raw));
                }
                let (l, c) = self.pos_line();
                Err(MvError::new(
                    l,
                    c,
                    "di level module hanya 'assert property (…)' yang sah \
                     (concurrent assertion, LRM 1800 §14) — immediate \
                     'assert (…)' harus di dalam initial/always/seq/comb"
                        .to_string(),
                ))
            }
            // `assume property ( … )` di level module — mirror `assert property`.
            Tok::Assume => {
                self.advance();
                if self.is_ident("property") {
                    self.advance();
                    let raw = self.parse_assert_property_raw()?;
                    return Ok(MItem::AssumeProperty(raw));
                }
                let (l, c) = self.pos_line();
                Err(MvError::new(
                    l,
                    c,
                    "di level module hanya 'assume property (…)' yang sah \
                     (concurrent assertion, LRM 1800 §14) — immediate \
                     'assume (…)' harus di dalam initial/always/seq/comb"
                        .to_string(),
                ))
            }
            Tok::Seq => Ok(MItem::Seq(self.parse_seq_spec()?, self.parse_stmt()?)),
            Tok::Comb => {
                self.advance();
                Ok(MItem::Comb(self.parse_stmt()?))
            }
            Tok::Always => {
                self.advance();
                Ok(MItem::Always(self.parse_stmt()?))
            }
            Tok::Latch => {
                self.advance();
                Ok(MItem::Latch(self.parse_stmt()?))
            }
            Tok::Initial => {
                self.advance();
                Ok(MItem::Initial(self.parse_stmt()?))
            }
            Tok::Final => {
                self.advance();
                Ok(MItem::Final(self.parse_stmt()?))
            }
            Tok::Inst => Ok(self.parse_inst()?),
            Tok::For => {
                // generate for — optional `step` (`for i in 0..8 step 2`)
                self.advance();
                let var = self.expect_ident()?;
                self.expect(&Tok::In)?;
                let from = self.parse_expr()?;
                self.expect(&Tok::DotDot)?;
                let to = self.parse_expr()?;
                let step = self.parse_optional_step()?;
                let body = self.parse_module_item_block()?;
                Ok(MItem::GenFor {
                    var,
                    from,
                    to,
                    step,
                    body,
                })
            }
            Tok::If => {
                // generate if
                self.advance();
                self.expect(&Tok::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let then = self.parse_module_item_block()?;
                let els = if self.eat(&Tok::Else) {
                    self.parse_module_item_block()?
                } else {
                    Vec::new()
                };
                Ok(MItem::GenIf { cond, then, els })
            }
            Tok::Func => Ok(MItem::Func(self.parse_func()?)),
            Tok::Task => Ok(MItem::Task(self.parse_task()?)),
            _ => {
                let (l, c) = self.pos_line();
                Err(MvError::new(
                    l,
                    c,
                    format!("item module tidak dikenal: {:?}", self.peek()),
                ))
            }
        }
    }

    pub(crate) fn parse_module_item_block(&mut self) -> Result<Vec<MItem>, MvError> {
        self.expect(&Tok::LBrace)?;
        let mut items = Vec::new();
        while !self.eat(&Tok::RBrace) {
            items.push(self.parse_module_item()?);
        }
        Ok(items)
    }

    /// `seq(clk)` / `seq(clk, rst)` / `seq(clk, rst, sync)` / `seq(negedge clk, ...)`
    pub(crate) fn parse_seq_spec(&mut self) -> Result<SeqSpec, MvError> {
        self.expect(&Tok::Seq)?;
        let (l, c) = self.pos_line();
        self.expect(&Tok::LParen)?;
        let mut neg_edge = false;
        if matches!(self.peek(), Tok::NegEdge) {
            neg_edge = true;
            self.advance();
        } else if matches!(self.peek(), Tok::PosEdge) {
            self.advance();
        }
        // F26: clock bisa `clk` (signal) atau `iface.clk` (field port
        // interface) — parse postfix, ambil teks persis via raw_slice.
        let (sl, sc) = self.pos_line();
        let cexpr = self.parse_postfix_expr()?;
        let (el, ec) = self.pos_line();
        let clk = self.raw_slice(sl, sc, el, ec).trim().to_string();
        match cexpr {
            Expr::Ident(..) | Expr::Member(..) | Expr::Index(..) | Expr::Range(..) => {}
            _ => {
                return Err(MvError::new(
                    l,
                    c,
                    format!("clock seq harus berupa signal (mis. `clk` atau `iface.clk`), ditemukan: {clk}"),
                ));
            }
        }
        let mut reset = None;
        if self.eat(&Tok::Comma) {
            let rname = self.expect_ident()?;
            let active_low = rname.ends_with("_n") || rname.ends_with("_N");
            let mut sync = false;
            // BUG FIX: `sync` di-lex sebagai Tok::Sync (keyword), bukan Ident —
            // sebelumnya `matches!(_, Tok::Ident(s) if s == "sync")` selalu
            // false → `seq(clk, rst, sync)` tak pernah ter-parse (regresi F??).
            if self.eat(&Tok::Comma) && matches!(self.peek(), Tok::Sync) {
                sync = true;
                self.advance();
            }
            reset = Some((rname, active_low, sync));
        }
        self.expect(&Tok::RParen)?;
        Ok(SeqSpec {
            clk,
            neg_edge,
            reset,
            line: l,
            col: c,
        })
    }

    /// Daftar override parameter instansiasi: `#(8)` (positional, nama
    /// kosong) / `#(.DEPTH(4))` (named) / campuran. Konsumsi `Hash` + kurung.
    fn parse_inst_params(&mut self) -> Result<Vec<(String, Expr)>, MvError> {
        self.expect(&Tok::Hash)?;
        self.expect(&Tok::LParen)?;
        let mut params = Vec::new();
        while !self.eat(&Tok::RParen) {
            if self.peek() == &Tok::Dot {
                // named `.DEPTH(4)`
                self.advance();
                let pname = self.expect_ident()?;
                self.expect(&Tok::LParen)?;
                let pval = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                params.push((pname, pval));
            } else {
                // positional `#(8, 4)` — nama kosong (marker)
                let pval = self.parse_expr()?;
                params.push((String::new(), pval));
            }
            self.eat(&Tok::Comma);
        }
        Ok(params)
    }

    pub(crate) fn parse_inst(&mut self) -> Result<MItem, MvError> {
        self.expect(&Tok::Inst)?;
        // Catat posisi nama module utk error validasi koneksi port (F29).
        let (line, col) = self.pos_line();
        let module = self.expect_ident()?;
        // Override parameter boleh SEBELUM nama instance (gaya SV, yang juga
        // bentuk yang dipakai MIVON-HDL.md §6.7 `inst fifo #(.DEPTH(32)) u_fifo`)
        // maupun SETELAH nama (`inst fifo u_fifo #(.DEPTH(32))`) — keduanya
        // diterima, dan keduanya di-emit sebagai `#(...) name` di SV.
        let params_head = if matches!(self.peek(), Tok::Hash) {
            self.parse_inst_params()?
        } else {
            Vec::new()
        };
        let name = self.expect_ident()?;
        let dims = if self.eat(&Tok::LBrack) {
            let e = self.parse_expr()?;
            self.expect(&Tok::RBrack)?;
            Some(e)
        } else {
            None
        };
        let mut params = params_head;
        if matches!(self.peek(), Tok::Hash) {
            if !params.is_empty() {
                let (l, c) = self.pos_line();
                return Err(MvError::new(
                    l,
                    c,
                    "override parameter ditulis dua kali pada satu instansiasi".to_string(),
                ));
            }
            params = self.parse_inst_params()?;
        }
        let mut conns = Vec::new();
        if self.eat(&Tok::LParen) {
            while !self.eat(&Tok::RParen) {
                if self.eat(&Tok::Dot) {
                    let port = self.expect_ident()?;
                    let expr = if self.eat(&Tok::LParen) {
                        let e = self.parse_expr()?;
                        self.expect(&Tok::RParen)?;
                        Some(e)
                    } else {
                        None
                    };
                    conns.push(Conn::Named { port, expr });
                } else {
                    let e = self.parse_expr()?;
                    conns.push(Conn::Positional(e));
                }
                self.eat(&Tok::Comma);
            }
        }
        Ok(MItem::Inst {
            module,
            name,
            dims,
            params,
            conns,
            line,
            col,
        })
    }
}
