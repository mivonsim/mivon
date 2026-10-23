// Parser submodule: procedural block / function / task / generate parsing
// Tanggung jawab: parse_always, parse_initial, parse_final, parse_sensitivity_events,
// parse_sensitivity_list, parse_assign, parse_delay,
// parse_function, parse_task, parse_generate_block/Item/BlockBody

use super::Parser;
use crate::lexer::*;
use mivon_ast::types::const_eval_simple;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

impl Parser {
    /// Skip dimensi unpacked array setelah nama port/deklarasi:
    /// `arr[]`, `arr[$]` (queue), `arr[N]`, `arr[msb:lsb]`, `arr[2][3]`.
    /// Dimensi unpacked tidak mempengaruhi parse port — hanya dikonsumsi agar
    /// `]` kosong / `$` tidak gagal di `parse_expr` (pola `ref bit [7:0] arr[]`
    /// dan `const ref int int_q[$]` umum di OpenTitan DV).
    pub(crate) fn skip_unpacked_dims(&mut self) -> Result<(), SimError> {
        while self.peek() == &Token::LBrack {
            self.advance(); // '['
            if self.peek() == &Token::Dollar && self.peek_ahead(1) == &Token::RBrack {
                self.advance(); // '$' — queue `[$]`
            } else if self.peek() != &Token::RBrack {
                self.parse_expr(0)?;
                if self.peek() == &Token::Colon {
                    self.advance();
                    self.parse_expr(0)?;
                }
            }
            self.expect(Token::RBrack)?;
        }
        Ok(())
    }

    /// Blind-skip dimensi unpacked LANJUTAN `[..][..]` yang TIDAK bisa
    /// diparse sebagai fixed dim (eksotik: key-type `[string]`/`[int]`,
    /// dynamic `[]`, queue `[$]`, atau isi bukan ekspresi). Dipakai F39
    /// sebagai fallback saat dim lanjutan array multi-dimensi tidak
    /// representable di AST — perilaku lama (skip buta) dipertahankan.
    pub(crate) fn skip_extra_unpacked_dims(&mut self) {
        while self.peek() == &Token::LBrack {
            self.advance(); // '['
            let mut bdepth = 0i32;
            loop {
                match self.peek() {
                    Token::Eof => break,
                    Token::LParen | Token::LBrace | Token::LBrack => {
                        bdepth += 1;
                        self.advance();
                    }
                    Token::RParen | Token::RBrace => {
                        bdepth = bdepth.saturating_sub(1);
                        self.advance();
                    }
                    Token::RBrack => {
                        if bdepth <= 0 {
                            self.advance();
                            break;
                        }
                        bdepth -= 1;
                        self.advance();
                    }
                    _ => {
                        self.advance();
                    }
                }
            }
        }
    }

    /// Parse range packed `[msb:lsb]` ATAU lewati dimensi unpacked (`[N]`,
    /// `[]`) setelah nama port. Mengembalikan `Some(range)` hanya untuk
    /// `[expr:expr]`. Saat `parse_range` gagal (mis. `[8]` single-ekspresi),
    /// posisi di-restore lalu bracket dikonsumsi sebagai dimensi unpacked.
    pub(crate) fn parse_port_dims(&mut self) -> Result<Option<ExprRange>, SimError> {
        if self.peek() != &Token::LBrack {
            return Ok(None);
        }
        let saved = self.pos.get();
        if let Ok(Some(er)) = self.parse_range() {
            return Ok(Some(er));
        }
        self.pos.set(saved);
        while self.peek() == &Token::LBrack {
            self.advance();
            if self.peek() == &Token::Dollar && self.peek_ahead(1) == &Token::RBrack {
                self.advance(); // '$' — queue `[$]`
            } else if self.peek() != &Token::RBrack {
                let _ = self.parse_expr(0);
                if self.peek() == &Token::Colon {
                    self.advance();
                    let _ = self.parse_expr(0);
                }
            }
            self.expect(Token::RBrack)?;
        }
        Ok(None)
    }

    pub(crate) fn parse_always(&mut self) -> Result<AlwaysBlock, SimError> {
        let kind = match self.peek() {
            Token::Always => {
                self.advance();
                AlwaysKind::Always
            }
            Token::AlwaysComb => {
                self.advance();
                AlwaysKind::AlwaysComb
            }
            Token::AlwaysFF => {
                self.advance();
                AlwaysKind::AlwaysFF
            }
            Token::AlwaysLatch => {
                self.advance();
                AlwaysKind::AlwaysLatch
            }
            _ => unreachable!(),
        };

        // Skip attribute annotations after the keyword, e.g.
        // `always_ff (* xprop_off *) @(posedge clk)` (OpenTitan AST/SVA).
        // Tanpa ini, `(*` dianggap awal blok statement → sensitivity list
        // hilang → "always_ff requires sensitivity list".
        if self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
            self.skip_attribute();
        }

        let sensitivity = if self.peek() == &Token::At {
            self.advance();
            Some(self.parse_sensitivity_list()?)
        } else {
            None
        };

        let stmts = self.parse_stmt_block()?;

        Ok(AlwaysBlock {
            kind,
            sensitivity,
            stmts,
        })
    }

    pub(crate) fn parse_initial(&mut self) -> Result<InitialBlock, SimError> {
        self.advance();
        let stmts = self.parse_stmt_block()?;
        Ok(InitialBlock { stmts })
    }

    pub(crate) fn parse_final(&mut self) -> Result<InitialBlock, SimError> {
        self.advance();
        let stmts = self.parse_stmt_block()?;
        Ok(InitialBlock { stmts })
    }

    pub(crate) fn parse_sensitivity_events(&mut self) -> Result<Vec<SensitivityEvent>, SimError> {
        let mut events = Vec::new();
        loop {
            if self.peek() == &Token::RParen {
                break;
            }
            let event = if self.peek() == &Token::Star {
                self.advance();
                SensitivityEvent::Wildcard
            } else if self.peek() == &Token::PosEdge {
                self.advance();
                // parse_expr (bukan parse_primary) agar `@(posedge sig[idx])`
                // dan member access ikut terdukung.
                let expr = self.parse_expr(0)?;
                SensitivityEvent::PosEdge(expr)
            } else if self.peek() == &Token::NegEdge {
                self.advance();
                let expr = self.parse_expr(0)?;
                SensitivityEvent::NegEdge(expr)
            } else {
                let expr = self.parse_expr(0)?;
                SensitivityEvent::Level(expr)
            };
            // LANG-27: `@(posedge clk iff (en))` — guard kondisi event.
            // `iff` di-lex sebagai Ident("iff") (tidak ada token khusus).
            let event = if matches!(self.peek(), Token::Ident(s) if s == "iff") {
                self.advance();
                let cond = self.parse_expr(0)?;
                SensitivityEvent::Iff {
                    event: Box::new(event),
                    cond,
                }
            } else {
                event
            };
            events.push(event);
            if self.peek() == &Token::Or || self.peek() == &Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(events)
    }

    pub(crate) fn parse_sensitivity_list(&mut self) -> Result<SensitivityList, SimError> {
        // Handle @* or @(*)
        if self.peek() == &Token::Star {
            self.advance();
            return Ok(SensitivityList {
                events: vec![SensitivityEvent::Wildcard],
            });
        }
        if self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
            self.advance(); // (
            self.advance(); // *
                            // Check for @( * ) — closing paren may or may not be present
            if self.peek() == &Token::RParen {
                self.advance();
            }
            return Ok(SensitivityList {
                events: vec![SensitivityEvent::Wildcard],
            });
        }
        self.expect(Token::LParen)?;
        let events = self.parse_sensitivity_events()?;
        self.expect(Token::RParen)?;
        Ok(SensitivityList { events })
    }

    pub(crate) fn parse_assign(&mut self) -> Result<Vec<ContinuousAssign>, SimError> {
        self.advance();

        // Drive strength: `assign (weak0, weak1) net = ...;` / `(strong0, pull1)`.
        // Strength tokens bukan keyword (weak0/strong1/pull0/highz1 lex sebagai
        // ident) sehingga deteksi pola `( strength , strength )` manual.
        if self.peek() == &Token::LParen {
            let saved = self.pos.get();
            self.advance(); // '('
            let s1 = self.peek().clone();
            self.advance();
            if self.peek() == &Token::Comma {
                self.advance();
                let s2 = self.peek().clone();
                self.advance();
                if self.peek() == &Token::RParen
                    && Self::is_drive_strength(&s1)
                    && Self::is_drive_strength(&s2)
                {
                    self.advance(); // ')'
                } else {
                    self.pos.set(saved);
                }
            } else {
                self.pos.set(saved);
            }
        }

        let delay = if self.peek() == &Token::Hash {
            Some(self.parse_delay()?)
        } else {
            None
        };

        // LRM 1800 §10.3: `assign a=b, c=d, ...;` — list koma. Tiap
        // pasangan punya lhs=rhs sendiri; delay awal berlaku utk semua
        // (bentuk `#d a=b, c=d` jarang pakai delay per-item).
        let mut out = Vec::new();
        loop {
            let lhs = self.parse_expr(0)?;
            self.expect(Token::BlockingAssign)?;
            let rhs = self.parse_expr(0)?;
            out.push(ContinuousAssign {
                lhs,
                rhs,
                delay: delay.clone(),
            });
            if self.peek() == &Token::Comma {
                self.advance();
                continue;
            }
            break;
        }
        self.skip_semi();

        Ok(out)
    }

    /// Apakah token ini keyword drive-strength net (LRM 1800 §6.7.2)?
    fn is_drive_strength(tok: &Token) -> bool {
        const STRENGTHS: &[&str] = &[
            "supply0", "strong0", "pull0", "weak0", "highz0", "supply1", "strong1", "pull1",
            "weak1", "highz1",
        ];
        match tok {
            Token::Ident(name) => STRENGTHS.iter().any(|s| *name == Symbol::intern(s)),
            Token::Supply0 | Token::Supply1 => true,
            _ => false,
        }
    }

    pub(crate) fn parse_delay(&mut self) -> Result<Delay, SimError> {
        self.advance();
        // Bentuk polos `#expr` (ex. `assign #1ps net = ...;`) — satu delay
        // tanpa kurung; time literal (`1ns`, `1ps`) di-parse oleh parse_expr.
        if self.peek() != &Token::LParen {
            let d = self.parse_expr(0)?;
            return Ok(Delay {
                rise: Some(d),
                fall: None,
                turnoff: None,
            });
        }
        self.expect(Token::LParen)?;
        let rise = Some(self.parse_expr(0)?);
        let fall = if self.peek() == &Token::Comma {
            self.advance();
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        let turnoff = if self.peek() == &Token::Comma {
            self.advance();
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        self.expect(Token::RParen)?;
        Ok(Delay {
            rise,
            fall,
            turnoff,
        })
    }

    /// Intra-assignment delay: `lhs = #(expr) rhs`, `lhs <= #expr rhs`
    /// (LRM 1800 §10.4.3). Hanya `#(rise, fall, turnoff)` atau `#expr` polos.
    /// Mengembalikan `None` bila token berikut bukan `#`.
    pub(crate) fn parse_intra_assign_delay(&mut self) -> Result<Option<Delay>, SimError> {
        if self.peek() != &Token::Hash {
            return Ok(None);
        }
        self.advance(); // consume '#'
        if self.peek() == &Token::LParen {
            // #(rise, fall, turnoff) — pakai parse_delay dari posisi sebelum
            // `#` tidak mungkin (sudah di-consume), jadi parse manual di sini.
            self.advance(); // consume '('
            let rise = Some(self.parse_expr(0)?);
            let fall = if self.peek() == &Token::Comma {
                self.advance();
                Some(self.parse_expr(0)?)
            } else {
                None
            };
            let turnoff = if self.peek() == &Token::Comma {
                self.advance();
                Some(self.parse_expr(0)?)
            } else {
                None
            };
            self.expect(Token::RParen)?;
            Ok(Some(Delay {
                rise,
                fall,
                turnoff,
            }))
        } else {
            let d = self.parse_expr(0)?;
            Ok(Some(Delay {
                rise: Some(d),
                fall: None,
                turnoff: None,
            }))
        }
    }

    pub(crate) fn parse_function(&mut self, virtual_flag: bool) -> Result<FunctionDecl, SimError> {
        self.advance(); // consume 'function'
                        // Capture optional 'static' qualifier
        let is_static = if matches!(self.peek(), Token::Static) {
            self.advance();
            true
        } else {
            if matches!(self.peek(), Token::Auto) {
                self.advance();
            }
            false
        };
        // Parse optional return type
        let return_type = match self.peek() {
            Token::Void => {
                self.advance();
                Some(Box::new(DataType::Void))
            }
            Token::Int => {
                self.advance();
                Some(Box::new(DataType::Int))
            }
            Token::Integer => {
                self.advance();
                Some(Box::new(DataType::Integer))
            }
            Token::String => {
                self.advance();
                Some(Box::new(DataType::String))
            }
            Token::Byte => {
                self.advance();
                Some(Box::new(DataType::Byte))
            }
            Token::Shortint => {
                self.advance();
                Some(Box::new(DataType::Shortint))
            }
            Token::Longint => {
                self.advance();
                Some(Box::new(DataType::Longint))
            }
            Token::Time => {
                self.advance();
                Some(Box::new(DataType::Time))
            }
            Token::Real => {
                self.advance();
                Some(Box::new(DataType::Real))
            }
            Token::RealTime => {
                self.advance();
                Some(Box::new(DataType::Realtime))
            }
            Token::Bit => {
                self.advance();
                Some(Box::new(DataType::Bit))
            }
            Token::Logic => {
                self.advance();
                Some(Box::new(DataType::Logic))
            }
            Token::Signed => {
                self.advance();
                Some(Box::new(DataType::Signed(Box::new(DataType::Logic))))
            }
            Token::Ident(name) if self.type_param_names.contains(name) => {
                let tp_name = *name;
                self.advance();
                Some(Box::new(DataType::UserDefined(tp_name)))
            }
            Token::Ident(_)
                if matches!(
                    self.peek_ahead(1),
                    Token::Ident(_) | Token::LBrack | Token::Scope
                ) =>
            {
                let saved = self.pos.get();
                let first = self.expect_ident()?;
                let mut result = None;
                if self.peek() == &Token::Scope {
                    // pkg::type (hanya 1 level — method name di-handle function name parser)
                    self.advance();
                    match self.peek() {
                        Token::Ident(_) => {
                            let second = self.expect_ident()?;
                            // Ambigu: `pkg::type name(...)` (return type) vs
                            // `ClassName::method(...)` (out-of-body method).
                            // Setelah `::ident` masih ada ident/lbrack → return type
                            // package-qualified; langsung `(` → class-method prefix,
                            // kembalikan posisi agar return type = None.
                            if matches!(self.peek(), Token::Ident(_) | Token::LBrack) {
                                result = Some(Box::new(DataType::UserDefined(Symbol::intern(
                                    &format!("{}::{}", first, second),
                                ))));
                            } else if matches!(self.peek(), Token::LParen) {
                                self.pos.set(saved);
                            } else {
                                result = Some(Box::new(DataType::UserDefined(Symbol::intern(
                                    &format!("{}::{}", first, second),
                                ))));
                            }
                        }
                        // `ClassName::new(...)` — out-of-body constructor.
                        // Jangan konsume `::` di return-type parser; kembalikan
                        // posisi agar function-name parser menangani `::new`.
                        Token::New => {
                            self.pos.set(saved);
                        }
                        _ => result = Some(Box::new(DataType::UserDefined(first))),
                    }
                } else {
                    result = Some(Box::new(DataType::UserDefined(first)));
                }
                result
            }
            _ => None,
        };
        if self.peek() == &Token::Signed {
            self.advance();
        }
        if self.peek() == &Token::Unsigned {
            self.advance();
        }
        let range = if self.peek() == &Token::LBrack {
            self.parse_range()?
        } else {
            None
        };
        self.skip_extra_packed_dims()?;
        // Handle out-of-body method: type sudah consume `ClassName`, sekarang `::method`
        // atau function name langsung (class_name sudah di-return sebagai type oleh type parser)
        let name = if self.peek() == &Token::Scope {
            // type parser return `ClassName` — consume `::` lalu ambil method name
            self.advance();
            match self.peek() {
                Token::Ident(_) => self.expect_ident()?,
                Token::New => {
                    self.advance();
                    Symbol::intern("new")
                }
                _ => return Err(self.err_with_code(DiagCode::ExpectedToken, "expected method name after ::")),
            }
        } else {
            let name_tok = self.peek().clone();
            match &name_tok {
                Token::Ident(n) => {
                    self.advance();
                    let method_name = *n;
                    // Out-of-class method: `function [type] ClassName :: method_name`
                    // ClassName was taken above; `::method_name` is the actual method.
                    if self.peek() == &Token::Scope {
                        self.advance(); // consume `::`
                        match self.peek() {
                            Token::Ident(_) => self.expect_ident()?,
                            Token::New => {
                                self.advance();
                                Symbol::intern("new")
                            }
                            _ => method_name, // fallback
                        }
                    } else {
                        method_name
                    }
                }
                Token::New => {
                    self.advance();
                    Symbol::intern("new")
                }
                _ => return Err(self.err_with_code(DiagCode::ExpectedToken, "expected function name")),
            }
        };
        // Parse ANSI-style port list in parens (e.g., function new(int level, string name))
        let mut ports = Vec::new();
        let mut decls = Vec::new();
        let mut last_direction: Option<PortDirection> = None;
        if self.peek() == &Token::LParen {
            self.advance();
            while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                // Track whether we saw int/integer for 32-bit default width
                let is_int = matches!(self.peek(), Token::Int | Token::Integer);
                // Skip type keywords and direction keywords
                if matches!(
                    self.peek(),
                    Token::Int
                        | Token::Integer
                        | Token::String
                        | Token::Void
                        | Token::Reg
                        | Token::Logic
                        | Token::Wire
                        | Token::Signed
                        | Token::Unsigned
                        | Token::Input
                        | Token::Output
                        | Token::Inout
                        | Token::Ref
                        | Token::Byte
                        | Token::Shortint
                        | Token::Longint
                        | Token::Bit
                        | Token::Time
                        | Token::Real
                        | Token::RealTime
                        | Token::WReal
                ) {
                    if matches!(
                        self.peek(),
                        Token::Input | Token::Output | Token::Inout | Token::Ref
                    ) {
                        last_direction = Some(match self.peek() {
                            Token::Input => PortDirection::Input,
                            Token::Output => PortDirection::Output,
                            Token::Ref => PortDirection::Ref,
                            _ => PortDirection::Inout,
                        });
                    }
                    self.advance();
                } else if let Token::Ident(name) = self.peek() {
                    if self.type_param_names.contains(name) {
                        self.advance();
                    } else if self.peek_ahead(1) == &Token::Scope {
                        // Scoped type `pkg::type name` (mis.
                        // `prim_mubi_pkg::mubi4_t val` di lc_ctrl_pkg). Konsumsi
                        // `pkg :: type` sebagai TIPE — BUKAN nama port. Sebelumnya
                        // `pkg` jadi port pertama & `val` bergeser → inline
                        // substitution port tidak cocok → "signal 'val' not found".
                        self.advance(); // pkg
                        self.advance(); // ::
                        self.advance(); // type
                    } else if matches!(self.peek_ahead(1), Token::Ident(_)) {
                        // User-defined type name diikuti nama port (`mubi4_t a`).
                        self.advance();
                    } else if self.peek_ahead(1) == &Token::LBrack
                        && (self.peek_ahead(2) == &Token::RBrack
                            || self.peek_ahead(2) == &Token::Dollar)
                    {
                        // `name []` / `name [$]` — nama port dgn dimensi unpacked
                        // kosong/queue (BUKAN tipe). Jangan advance: inner loop
                        // akan memakannya sebagai nama port lalu `skip_unpacked_dims`
                        // memakan `[]` / `[$]`.
                    } else if matches!(self.peek_ahead(1), Token::LBrack) {
                        // Tipe user-defined dgn packed range (`foo_t [7:0] name`).
                        self.advance();
                    }
                } else {
                    // Unknown token in port list — advance to avoid infinite loop
                    self.advance();
                    continue;
                }
                // Parse range like [7:0] — simpan ke expr_range (dan range
                // bila konstanta) agar lebar port function bisa di-resolve.
                // Sebelumnya range dibuang → `func_port_width` selalu 1 →
                // width mismatch saat inlining function package.
                let mut expr_range = None;
                let mut range = None;
                if let Some(er) = self.parse_port_dims()? {
                    if let (Ok(m), Ok(l)) = (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                    {
                        range = Some(Range {
                            msb: m as usize,
                            lsb: l as usize,
                        });
                    } else {
                        expr_range = Some(er);
                    }
                } else if is_int {
                    range = Some(Range { msb: 31, lsb: 0 });
                }
                self.skip_extra_packed_dims()?;
                // Parse port name(s)
                while let Token::Ident(pname) = self.peek() {
                    // Jika setelah nama ada ident lagi, ini bukan nama
                    // port kedua melainkan tipe port baru (`(mubi4_t a, mubi4_t b)`).
                    // Inner loop hanya boleh memakan nama dgn tipe yang sama.
                    // Catatan: `Ident LBrack` TIDAK boleh break — itu unpacked
                    // array dim (`logic [7:0] mat_a [8]`), bukan tipe baru.
                    // KECUALI pola `foo_t [7:0] name`: ident pertama adalah
                    // TIPE user-defined (bukan nama), `[7:0]` packed range,
                    // dan ident setelah `]` adalah nama port. Tanpa break di
                    // sini, `foo_t` dimakan sebagai nama port pertama & `name`
                    // sebagai port kedua → formal bergeser → error E2001.
                    if matches!(self.peek_ahead(1), Token::Ident(_) | Token::Scope)
                        || (self.peek_ahead(1) == &Token::LBrack
                            && self.peek_packed_range_followed_by_ident())
                    {
                        break;
                    }
                    let pn = *pname;
                    self.advance();
                    self.skip_unpacked_dims()?;
                    // F43: default port (`task f(int x = 5, bit r = 1'b1)`) —
                    // tangkap ekspresi default agar inline bisa memberi nilai
                    // saat call tidak me-pass port ini.
                    let default = if self.peek() == &Token::BlockingAssign {
                        self.advance();
                        Some(self.parse_expr(0)?)
                    } else {
                        None
                    };
                    ports.push(FunctionPort {
                        name: pn,
                        range: range.clone(),
                        expr_range: expr_range.clone(),
                        direction: last_direction,
                        default,
                    });
                    if self.peek() == &Token::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RParen)?;
        }
        if self.peek() == &Token::Semi {
            self.advance();
        }
        // Parse ports and declarations until 'begin' or statement.
        // `stmts` diisi dari blok `begin...end` (bila ada) dan/atau statement
        // sisa body function — begin...end TIDAK harus jadi statement terakhir.
        let mut stmts: Vec<Stmt> = Vec::new();
        loop {
            match self.peek() {
                Token::Input | Token::Output | Token::Inout | Token::Ref => {
                    let direction = match self.peek() {
                        Token::Input => {
                            self.advance();
                            PortDirection::Input
                        }
                        Token::Output => {
                            self.advance();
                            PortDirection::Output
                        }
                        Token::Ref => {
                            self.advance();
                            PortDirection::Ref
                        }
                        _ => {
                            self.advance();
                            PortDirection::Inout
                        }
                    };
                    // F34: konsumsi keyword tipe dasar SETELAH direction pada
                    // port non-ANSI (`input int x;`, `input logic [7:0] x;`).
                    // Sebelumnya `int` tidak dikonsumsi → port `x` tidak masuk
                    // func.ports (masuk func.decls) → inline membuat temp ganda
                    // (`_arg0` vs `x`) → argumen tak pernah sampai ke body.
                    let mut port_is_int = false;
                    while matches!(
                        self.peek(),
                        Token::Int
                            | Token::Integer
                            | Token::Reg
                            | Token::Logic
                            | Token::Wire
                            | Token::Bit
                            | Token::Byte
                            | Token::Shortint
                            | Token::Longint
                            | Token::Time
                            | Token::String
                            | Token::Real
                            | Token::RealTime
                            | Token::WReal
                            | Token::Signed
                            | Token::Unsigned
                    ) {
                        if matches!(self.peek(), Token::Int | Token::Integer) {
                            port_is_int = true;
                        }
                        self.advance();
                    }
                    let mut port_range = if self.peek() == &Token::LBrack {
                        let er = self.parse_port_dims()?; // `[msb:lsb]` atau skip unpacked
                        er.as_ref().and_then(|er| {
                            if let (Ok(m), Ok(l)) =
                                (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                            {
                                Some(Range {
                                    msb: m as usize,
                                    lsb: l as usize,
                                })
                            } else {
                                None
                            }
                        })
                    } else if port_is_int {
                        Some(Range { msb: 31, lsb: 0 })
                    } else {
                        None
                    };
                    // Tipe user-defined non-ANSI: `input foo_t a;` atau
                    // `input foo_t [7:0] a;` — `foo_t` bukan keyword dasar,
                    // jadi loop keyword di atas tidak mengkonsumsinya. Konsumsi
                    // tipe (dan packed range) DI SINI agar inner loop hanya
                    // melihat nama port. Catatan: break di inner loop saja
                    // TIDAK cukup — `foo_t` yang tersisa membuat loop luar
                    // `Token::Ident` dengan peek_ahead(1)=LBrack jatuh ke
                    // `_ => break` → seluruh port list berhenti & sisa
                    // deklarasi masuk body → error E1005/E2001.
                    if matches!(self.peek(), Token::Ident(_))
                        && (matches!(self.peek_ahead(1), Token::Ident(_))
                            || (self.peek_ahead(1) == &Token::LBrack
                                && self.peek_packed_range_followed_by_ident()))
                    {
                        self.advance(); // konsumsi tipe `foo_t`
                        if self.peek() == &Token::LBrack {
                            if let Some(er) = self.parse_port_dims()? {
                                // `er` sudah ExprRange (bukan Option) — langsung
                                // pakai msb/lsb, jangan `.as_ref()` (E0599).
                                port_range = if let (Ok(m), Ok(l)) =
                                    (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                                {
                                    Some(Range {
                                        msb: m as usize,
                                        lsb: l as usize,
                                    })
                                } else {
                                    None
                                };
                            }
                        }
                    }
                    while let Token::Ident(pname) = self.peek() {
                        let pn = *pname;
                        self.advance();
                        self.skip_unpacked_dims()?;
                        let default = if self.peek() == &Token::BlockingAssign {
                            self.advance();
                            Some(self.parse_expr(0)?)
                        } else {
                            None
                        };
                        ports.push(FunctionPort {
                            name: pn,
                            range: port_range.clone(),
                            expr_range: None,
                            direction: Some(direction),
                            default,
                        });
                        if self.peek() == &Token::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.skip_semi();
                }
                Token::Wire | Token::Reg | Token::Logic | Token::Int | Token::Integer => {
                    let decl = self.parse_decl()?;
                    decls.push(decl);
                }
                Token::Bit
                | Token::Byte
                | Token::Shortint
                | Token::Longint
                | Token::Time
                | Token::String
                | Token::Real
                | Token::RealTime
                | Token::WReal
                | Token::Mailbox
                | Token::Semaphore => {
                    let decl = self.parse_decl()?;
                    decls.push(decl);
                }
                Token::Ident(_) => {
                    // User-defined type declaration: ident followed by ident or ::
                    // KECUALI pola `pkg::func(...)`: ident pertama adalah PACKAGE,
                    // `::` scope, dan `(` memulai call — ini STATEMENT (bukan
                    // deklarasi). Sebelumnya `uvm_config_db::set(this, ...)`
                    // dikira deklarasi `pkg::type name` → parse_decl gagal di
                    // koma pertama → parse_function Err → method class tidak
                    // terdaftar → build_phase (berisi uvm_config_db::set)
                    // hilang diam-diam. Sama seperti is_decl_stmt_start.
                    // F35: pola `sp2v_e [NumSlicesCtr-1:0] out;` — tipe
                    // user-defined dengan packed range diikuti nama variabel
                    // (`peek_packed_range_followed_by_ident`). Sebelumnya
                    // jatuh ke `_ => break` → deklarasi masuk body statement
                    // → inliner tidak rename → E2001 'out' not found.
                    match self.peek_ahead(1) {
                        Token::Ident(_) | Token::Scope => {
                            let is_scoped_call = matches!(self.peek_ahead(1), Token::Scope)
                                && matches!(self.peek_ahead(3), Token::LParen);
                            if is_scoped_call {
                                break;
                            }
                            let decl = self.parse_decl()?;
                            decls.push(decl);
                        }
                        Token::LBrack if self.peek_packed_range_followed_by_ident() => {
                            let decl = self.parse_decl()?;
                            decls.push(decl);
                        }
                        _ => break,
                    }
                }
                Token::Auto | Token::Static => {
                    // automatic/static variable declaration in function body
                    let is_static = matches!(self.peek(), Token::Static);
                    self.advance();
                    // Try to parse as declaration
                    if let Ok(mut decl) = self.parse_decl() {
                        if is_static {
                            for var in &mut decl.names {
                                var.is_static = true;
                            }
                        }
                        decls.push(decl);
                    } else {
                        return Err(self.err_with_code(DiagCode::ExpectedToken, "expected declaration after automatic/static"));
                    }
                }
                Token::Begin => {
                    // `begin...end` TIDAK harus statement terakhir: statement
                    // lain (return, assignment, foreach, dst.) boleh mengikuti
                    // sebelum `endfunction`. Simpan blok sebagai `stmts` awal
                    // lalu break ke loop statement kedua di bawah.
                    stmts = self.parse_stmt_block()?;
                    break;
                }
                Token::EndFunction => {
                    self.advance(); // consume 'endfunction'
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_) | Token::New) {
                            self.advance();
                        }
                    }
                    return Ok(FunctionDecl {
                        name,
                        range,
                        return_type,
                        ports,
                        decls,
                        stmts: vec![],
                        virtual_flag,
                        is_static,
                    });
                }
                _ => break,
            }
        }
        // Parse statements until endfunction — sisa body setelah `begin...end`
        // (atau body penuh bila tidak ada blok begin).
        loop {
            if matches!(
                self.peek(),
                Token::EndFunction
                    | Token::End
                    | Token::EndClass
                    | Token::EndInterface
                    | Token::EndPackage
                    | Token::Eof
            ) {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        match self.peek() {
            Token::EndFunction => {
                self.advance();
            }
            _ => {
                return Err(self.err_with_code(DiagCode::ExpectedToken, "expected endfunction"));
            }
        }
        if self.peek() == &Token::Colon {
            self.advance();
            if matches!(self.peek(), Token::Ident(_) | Token::New) {
                self.advance();
            }
        }
        Ok(FunctionDecl {
            name,
            range,
            return_type,
            ports,
            decls,
            stmts,
            virtual_flag,
            is_static,
        })
    }

    pub(crate) fn parse_task(&mut self, virtual_flag: bool) -> Result<TaskDecl, SimError> {
        self.advance(); // consume 'task'
                        // Capture optional 'static' qualifier
        let is_static = if matches!(self.peek(), Token::Static) {
            self.advance();
            true
        } else {
            if matches!(self.peek(), Token::Auto) {
                self.advance();
            }
            false
        };
        let name = self.expect_ident()?;
        // Handle out-of-body method: class_name :: method_name
        let name = if self.peek() == &Token::Scope {
            self.advance(); // consume ::
            let tok = self.peek().clone();
            match &tok {
                Token::Ident(m) => {
                    self.advance();
                    *m
                }
                _ => {
                    return Err(self.err_with_code(DiagCode::ExpectedToken, "expected method name after ::"));
                }
            }
        } else {
            name
        };
        let mut ports = Vec::new();
        let mut decls = Vec::new();
        let mut last_direction: Option<PortDirection> = None;
        // Parse ANSI-style port list in parens (e.g., task set_val(input [7:0] x))
        if self.peek() == &Token::LParen {
            self.advance();
            while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                // Comma setelah default value (mis. `task f(uint a = 10, int b)`)
                // tidak dikonsumsi oleh inner ident loop — consume di sini agar
                // loop tidak berputar tanpa progres.
                if self.peek() == &Token::Comma {
                    self.advance();
                    continue;
                }
                let is_int = matches!(self.peek(), Token::Int | Token::Integer);
                if matches!(
                    self.peek(),
                    Token::Int
                        | Token::Integer
                        | Token::String
                        | Token::Void
                        | Token::Reg
                        | Token::Logic
                        | Token::Wire
                        | Token::Signed
                        | Token::Input
                        | Token::Output
                        | Token::Inout
                        | Token::Ref
                ) {
                    if matches!(
                        self.peek(),
                        Token::Input | Token::Output | Token::Inout | Token::Ref
                    ) {
                        last_direction = Some(match self.peek() {
                            Token::Input => PortDirection::Input,
                            Token::Output => PortDirection::Output,
                            Token::Ref => PortDirection::Ref,
                            _ => PortDirection::Inout,
                        });
                    }
                    self.advance();
                } else if let Token::Ident(_) = self.peek() {
                    if self.peek_ahead(1) == &Token::Scope {
                        // Scoped type `pkg::type name` — konsumsi `pkg :: type`
                        // sebagai tipe, bukan nama port.
                        self.advance(); // pkg
                        self.advance(); // ::
                        self.advance(); // type
                    } else if matches!(self.peek_ahead(1), Token::Ident(_)) {
                        // User-defined type `foo_t name` — skip tipe.
                        self.advance();
                    } else if self.peek_ahead(1) == &Token::LBrack
                        && (self.peek_ahead(2) == &Token::RBrack
                            || self.peek_ahead(2) == &Token::Dollar)
                    {
                        // `name []` / `name [$]` — nama port dgn dimensi unpacked
                        // kosong/queue (BUKAN tipe). Jangan advance: inner loop
                        // akan memakannya sebagai nama port lalu `skip_unpacked_dims`
                        // memakan `[]` / `[$]`.
                    } else if matches!(self.peek_ahead(1), Token::LBrack) {
                        // Tipe user-defined dgn packed range (`foo_t [7:0] name`).
                        self.advance();
                    }
                    // else: nama port biasa — inner loop bawah yang memproses.
                } else if !matches!(self.peek(), Token::LBrack | Token::Comma) {
                    self.advance();
                    continue;
                }
                let range: Option<Range> = if let Ok(Some(er)) = self.parse_port_dims() {
                    if let (Ok(m), Ok(l)) = (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                    {
                        Some(Range {
                            msb: m as usize,
                            lsb: l as usize,
                        })
                    } else {
                        None
                    }
                } else if is_int {
                    Some(Range { msb: 31, lsb: 0 })
                } else {
                    None
                };
                while let Token::Ident(pname) = self.peek() {
                    // Pola `foo_t [7:0] name` — ident pertama adalah TIPE
                    // user-defined (bukan nama port); loop luar yang menangani
                    // tipe/range berikutnya. Tanpa break, `foo_t` dimakan
                    // sebagai nama port & `name` sebagai port kedua → formal
                    // bergeser → error E2001 (sama seperti parse_function).
                    // `Ident Ident` juga break: `(foo_t a, foo_t b)` — setelah
                    // koma, `foo_t` adalah tipe port baru, bukan nama.
                    // F39: `pkg::type name` — ident pertama (`pkg`) diikuti
                    // Scope (`::`) juga break; loop luar yang mengkonsumsi
                    // `pkg :: type`. Tanpa ini `pkg_t::t_t x` salah jadi
                    // dua port (`pkg_t`, `x`) → inline tak rename formal
                    // berikutnya → E2001 'wdata' not found.
                    if matches!(self.peek_ahead(1), Token::Ident(_) | Token::Scope)
                        || (self.peek_ahead(1) == &Token::LBrack
                            && self.peek_packed_range_followed_by_ident())
                    {
                        break;
                    }
                    let pn = *pname;
                    self.advance();
                    let default = if self.peek() == &Token::BlockingAssign {
                        self.advance();
                        Some(self.parse_expr(0)?)
                    } else {
                        None
                    };
                    ports.push(FunctionPort {
                        name: pn,
                        range: range.clone(),
                        expr_range: None,
                        direction: last_direction,
                        default,
                    });
                    if self.peek() == &Token::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RParen)?;
        }
        if self.peek() == &Token::Semi {
            self.advance();
        }

        // Parse non-ANSI port declarations and body.
        // `stmts` diisi dari blok `begin...end` (bila ada) dan/atau statement
        // sisa body task — begin...end TIDAK harus jadi statement terakhir.
        let mut stmts: Vec<Stmt> = Vec::new();
        loop {
            match self.peek() {
                Token::Input | Token::Output | Token::Inout | Token::Ref => {
                    let direction = match self.peek() {
                        Token::Input => {
                            self.advance();
                            PortDirection::Input
                        }
                        Token::Output => {
                            self.advance();
                            PortDirection::Output
                        }
                        Token::Ref => {
                            self.advance();
                            PortDirection::Ref
                        }
                        _ => {
                            self.advance();
                            PortDirection::Inout
                        }
                    };
                    // F34: konsumsi keyword tipe dasar SETELAH direction pada
                    // port non-ANSI (`input int x;`, `input logic [7:0] x;`)
                    // — sama seperti fix parse_function (lihat komentar di
                    // sana). Sebelumnya port masuk func.decls → inline temp
                    // ganda → argumen tak sampai ke body task.
                    let mut port_is_int = false;
                    while matches!(
                        self.peek(),
                        Token::Int
                            | Token::Integer
                            | Token::Reg
                            | Token::Logic
                            | Token::Wire
                            | Token::Bit
                            | Token::Byte
                            | Token::Shortint
                            | Token::Longint
                            | Token::Time
                            | Token::String
                            | Token::Real
                            | Token::RealTime
                            | Token::WReal
                            | Token::Signed
                            | Token::Unsigned
                    ) {
                        if matches!(self.peek(), Token::Int | Token::Integer) {
                            port_is_int = true;
                        }
                        self.advance();
                    }
                    let mut port_range = if self.peek() == &Token::LBrack {
                        let er = self.parse_port_dims()?; // `[msb:lsb]` atau skip unpacked
                        er.as_ref().and_then(|er| {
                            if let (Ok(m), Ok(l)) =
                                (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                            {
                                Some(Range {
                                    msb: m as usize,
                                    lsb: l as usize,
                                })
                            } else {
                                None
                            }
                        })
                    } else if port_is_int {
                        Some(Range { msb: 31, lsb: 0 })
                    } else {
                        None
                    };
                    // Tipe user-defined non-ANSI: `input foo_t a;` atau
                    // `input foo_t [7:0] a;` — `foo_t` bukan keyword dasar,
                    // jadi loop keyword di atas tidak mengkonsumsinya. Konsumsi
                    // tipe (dan packed range) DI SINI agar inner loop hanya
                    // melihat nama port. Catatan: break di inner loop saja
                    // TIDAK cukup — `foo_t` yang tersisa membuat loop luar
                    // `Token::Ident` dengan peek_ahead(1)=LBrack jatuh ke
                    // `_ => break` → seluruh port list berhenti & sisa
                    // deklarasi masuk body → error E1005/E2001.
                    if matches!(self.peek(), Token::Ident(_))
                        && (matches!(self.peek_ahead(1), Token::Ident(_))
                            || (self.peek_ahead(1) == &Token::LBrack
                                && self.peek_packed_range_followed_by_ident()))
                    {
                        self.advance(); // konsumsi tipe `foo_t`
                        if self.peek() == &Token::LBrack {
                            if let Some(er) = self.parse_port_dims()? {
                                // `er` sudah ExprRange (bukan Option) — langsung
                                // pakai msb/lsb, jangan `.as_ref()` (E0599).
                                port_range = if let (Ok(m), Ok(l)) =
                                    (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                                {
                                    Some(Range {
                                        msb: m as usize,
                                        lsb: l as usize,
                                    })
                                } else {
                                    None
                                };
                            }
                        }
                    }
                    while let Token::Ident(pname) = self.peek() {
                        let pn = *pname;
                        self.advance();
                        self.skip_unpacked_dims()?;
                        let default = if self.peek() == &Token::BlockingAssign {
                            self.advance();
                            Some(self.parse_expr(0)?)
                        } else {
                            None
                        };
                        ports.push(FunctionPort {
                            name: pn,
                            range: port_range.clone(),
                            expr_range: None,
                            direction: Some(direction),
                            default,
                        });
                        if self.peek() == &Token::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.skip_semi();
                }
                Token::Wire
                | Token::Reg
                | Token::Logic
                | Token::Int
                | Token::Integer
                | Token::Bit
                | Token::Byte
                | Token::Shortint
                | Token::Longint
                | Token::Time
                | Token::String
                | Token::Real
                | Token::RealTime
                | Token::WReal
                | Token::Mailbox
                | Token::Semaphore => {
                    decls.push(self.parse_decl()?);
                }
                Token::Ident(_) => {
                    // Tipe user-defined: `my_item it;` (ident diikuti ident/::)
                    // — sama seperti parse_function. Sebelumnya task body
                    // menganggapnya statement → decls task kosong → `it = new()`
                    // tak punya tipe class (F17).
                    // KECUALI `pkg::func(...)` — ini statement call, bukan
                    // deklarasi (lihat komentar panjang di parse_function).
                    // F35: pola `foo_t [7:0] name;` — tipe user-defined dgn
                    // packed range (sama seperti parse_function).
                    match self.peek_ahead(1) {
                        Token::Ident(_) | Token::Scope => {
                            let is_scoped_call = matches!(self.peek_ahead(1), Token::Scope)
                                && matches!(self.peek_ahead(3), Token::LParen);
                            if is_scoped_call {
                                break;
                            }
                            decls.push(self.parse_decl()?);
                        }
                        Token::LBrack if self.peek_packed_range_followed_by_ident() => {
                            decls.push(self.parse_decl()?);
                        }
                        _ => break,
                    }
                }
                Token::Auto | Token::Static => {
                    // static/automatic variable declaration in task body
                    let is_static = matches!(self.peek(), Token::Static);
                    self.advance();
                    if let Ok(mut decl) = self.parse_decl() {
                        if is_static {
                            for var in &mut decl.names {
                                var.is_static = true;
                            }
                        }
                        decls.push(decl);
                    } else {
                        return Err(self.err_with_code(DiagCode::ExpectedToken, "expected declaration after automatic/static"));
                    }
                }
                Token::Begin => {
                    // `begin...end` TIDAK harus statement terakhir: statement
                    // lain boleh mengikuti sebelum `endtask`. Simpan blok sebagai
                    // `stmts` awal lalu break ke loop statement kedua.
                    stmts = self.parse_stmt_block()?;
                    break;
                }
                Token::EndTask => {
                    self.advance(); // consume 'endtask'
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    return Ok(TaskDecl {
                        name,
                        ports,
                        decls,
                        stmts: vec![],
                        virtual_flag,
                        is_static,
                    });
                }
                _ => break,
            }
        }
        loop {
            if matches!(
                self.peek(),
                Token::EndTask
                    | Token::End
                    | Token::EndClass
                    | Token::EndInterface
                    | Token::EndPackage
                    | Token::Eof
            ) {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        match self.peek() {
            Token::EndTask => {
                self.advance();
            }
            _ => {
                return Err(self.err_with_code(DiagCode::ExpectedToken, "expected endtask"));
            }
        }
        if self.peek() == &Token::Colon {
            self.advance();
            if matches!(self.peek(), Token::Ident(_)) {
                self.advance();
            }
        }
        Ok(TaskDecl {
            name,
            ports,
            decls,
            stmts,
            virtual_flag,
            is_static,
        })
    }

    pub(crate) fn parse_generate_block(&mut self) -> Result<GenerateBlock, SimError> {
        self.advance(); // consume 'generate'
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Token::EndGenerate => {
                    self.advance();
                    return Ok(GenerateBlock { items });
                }
                Token::Eof => {
                    return Err(self.err_with_code(DiagCode::ExpectedToken, "unexpected EOF in generate block"));
                }
                _ => {
                    let item = self.parse_generate_item()?;
                    items.push(item);
                }
            }
        }
    }

    pub(crate) fn parse_generate_item(&mut self) -> Result<GenerateItem, SimError> {
        match self.peek() {
            Token::If => {
                self.advance();
                self.expect(Token::LParen)?;
                let cond = self.parse_expr(0)?;
                if std::env::var("DBG_GEN_IF").is_ok() {
                    let (df, dl) = self.resolve_source_file(self.peek_line());
                    eprintln!(
                        "DBG-GEN-IF: parse generate-if {}:{} line {}: {:?}",
                        df,
                        dl,
                        self.peek_line(),
                        format!("{:?}", cond)
                    );
                }
                self.expect(Token::RParen)?;
                let (true_label, true_items) = self.parse_generate_block_body()?;
                let false_items = if self.peek() == &Token::Else {
                    self.advance();
                    self.parse_generate_block_body()?.1
                } else {
                    Vec::new()
                };
                Ok(GenerateItem::If {
                    cond,
                    true_items,
                    false_items,
                    label: true_label,
                })
            }
            Token::For => {
                self.advance();
                self.expect(Token::LParen)?;
                // Skip optional 'genvar' keyword
                if self.peek() == &Token::GenVar {
                    self.advance();
                }
                // Skip optional tipe var: `for (int i = ...)`, `for (int unsigned
                // i = ...)`, `for (logic [3:0] i = ...)` — pola umum di reg_top
                // OpenTitan. Sebelumnya `int unsigned slice_idx` gagal dengan
                // "expected genvar name" → modul besar terpotong.
                while matches!(
                    self.peek(),
                    Token::Int
                        | Token::Integer
                        | Token::Bit
                        | Token::Logic
                        | Token::Reg
                        | Token::Byte
                        | Token::Shortint
                        | Token::Longint
                        | Token::Time
                        | Token::Signed
                        | Token::Unsigned
                ) {
                    self.advance();
                }
                // Range tipe di generate for: `for (logic [7:0] i = ...)`.
                // Urutan token: `[` msb `:` lsb `]` — parse msb dulu, baru
                // harapkan Colon (sebelumnya expect(Colon) langsung setelah
                // LBrack → selalu gagal karena token berikutnya adalah msb).
                if self.peek() == &Token::LBrack {
                    self.advance();
                    let _ = self.parse_expr(0)?;
                    self.expect(Token::Colon)?;
                    let _ = self.parse_expr(0)?;
                    self.expect(Token::RBrack)?;
                }
                let var_tok = self.peek().clone();
                let var = match &var_tok {
                    Token::Ident(n) => {
                        self.advance();
                        *n
                    }
                    _ => return Err(self.err_with_code(DiagCode::ExpectedToken, "expected genvar name")),
                };
                // Parse init: i = <expr>
                let _init = if self.peek() != &Token::Semi {
                    self.expect(Token::BlockingAssign)?;
                    let init_expr = self.parse_expr(0)?;
                    // Multi-var init generate-for (IEEE 1800 §12.7.1):
                    // `for (int i = 0, StateEnumT t = t.first(); ...)` —
                    // komma mengawali assignment var berikutnya. Parser
                    // mengharapkan `;` di sini (sebelumnya "expected Semi,
                    // found Comma" → seluruh module gagal; prim_sparse_fsm_flop).
                    while self.peek() == &Token::Comma {
                        self.advance();
                        // Tipe variabel optional di init kedua:
                        // `, int j = 0`, `, StateEnumT t = ...`, `, int unsigned k = ...`
                        while matches!(
                            self.peek(),
                            Token::Int
                                | Token::Integer
                                | Token::Bit
                                | Token::Logic
                                | Token::Reg
                                | Token::Byte
                                | Token::Shortint
                                | Token::Longint
                                | Token::Time
                                | Token::Signed
                                | Token::Unsigned
                        ) {
                            self.advance();
                        }
                        if self.peek() == &Token::LBrack {
                            self.advance();
                            let _ = self.parse_expr(0)?;
                            self.expect(Token::Colon)?;
                            let _ = self.parse_expr(0)?;
                            self.expect(Token::RBrack)?;
                        }
                        // Nama tipe user-defined: `, StateEnumT t = ...` —
                        // Ident diikuti Ident = nama tipe; skip.
                        if matches!(self.peek(), Token::Ident(_))
                            && matches!(self.peek_ahead(1), Token::Ident(_))
                        {
                            self.advance();
                        }
                        self.expect_ident()?;
                        self.expect(Token::BlockingAssign)?;
                        self.parse_expr(0)?;
                    }
                    self.expect(Token::Semi)?;
                    Some(Stmt::BlockingAssign {
                        lhs: Expr::Ident {
                            name: var,
                            line: 0,
                            col: 0,
                        },
                        rhs: init_expr,
                        delay: None,
                    })
                } else {
                    self.advance();
                    None
                };
                // Parse condition
                let cond = if self.peek() != &Token::Semi {
                    let c = Some(self.parse_expr(0)?);
                    self.expect(Token::Semi)?;
                    c
                } else {
                    self.advance();
                    None
                };
                // Parse step
                let step = if self.peek() != &Token::RParen {
                    let first = self.parse_stmt()?;
                    // Multi-step: `i += 1, t = t.next()` (generate-for §12.7.1) —
                    // konsumsi statement lanjutan dipisah koma.
                    while self.peek() == &Token::Comma {
                        self.advance();
                        let _ = self.parse_stmt()?;
                    }
                    Some(first)
                } else {
                    None
                };
                self.expect(Token::RParen)?;
                let (body_label, body_items) = self.parse_generate_block_body()?;
                if std::env::var("DBG_GEN").is_ok() {
                    eprintln!(
                        "[DBG-PARSER-GENFOR] body_items count={}, items: {:?}",
                        body_items.len(),
                        body_items
                            .iter()
                            .map(std::mem::discriminant)
                            .collect::<Vec<_>>()
                    );
                }
                Ok(GenerateItem::For {
                    var,
                    init: _init,
                    cond,
                    step,
                    body_items,
                    label: body_label,
                })
            }
            Token::GenVar => {
                self.skip_until_semi_or_end()?;
                // genvar declaration - skip, handled by For loop above
                self.parse_generate_item()
            }
            Token::Case | Token::CaseX | Token::CaseZ => {
                let case_type = match self.peek() {
                    Token::Case => GenerateCaseType::Normal,
                    Token::CaseX => GenerateCaseType::CaseX,
                    Token::CaseZ => GenerateCaseType::CaseZ,
                    _ => unreachable!(),
                };
                self.advance();
                self.expect(Token::LParen)?;
                let expr = self.parse_expr(0)?;
                self.expect(Token::RParen)?;

                let mut items = Vec::new();
                let mut default = None;

                loop {
                    if self.peek() == &Token::Endcase || self.peek() == &Token::Eof {
                        break;
                    }

                    if self.peek() == &Token::Default {
                        self.advance();
                        self.expect(Token::Colon)?;
                        default = Some(self.parse_generate_block_body()?.1);
                    } else {
                        let mut labels = Vec::new();
                        loop {
                            let label = self.parse_expr(0)?;
                            labels.push(label);
                            if self.peek() == &Token::Comma {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        self.expect(Token::Colon)?;
                        let body = self.parse_generate_block_body()?.1;
                        items.push(CaseGenerateItem { labels, body });
                    }
                }

                self.expect(Token::Endcase)?;
                Ok(GenerateItem::Case {
                    case_type,
                    expr,
                    items,
                    default,
                })
            }
            _ => {
                let item = self.parse_module_item()?;
                match item {
                    Some(mi) => Ok(GenerateItem::Items(vec![mi])),
                    None => Err(self.err_with_code(DiagCode::ExpectedToken, "expected generate item")),
                }
            }
        }
    }

    pub(crate) fn parse_generate_block_body(
        &mut self,
    ) -> Result<(Option<Symbol>, Vec<ModuleItem>), SimError> {
        if std::env::var("DBG_GEN").is_ok() {
            eprintln!(
                "[DBG-PARSER-GENBODY-START] peek={:?}, pos={}",
                self.peek(),
                self.pos.get()
            );
        }
        if self.peek() == &Token::Begin {
            self.advance();
            // Optional begin label: `begin [: name]` — tangkap untuk scope generate.
            let mut label = None;
            if self.peek() == &Token::Colon {
                self.advance();
                if let Token::Ident(n) = self.peek().clone() {
                    self.advance();
                    label = Some(n);
                }
            } else if let Token::Ident(n) = self.peek().clone() {
                // bentuk `begin name : ...` (tanpa colon) — jarang, abaikan label
                self.advance();
                if self.peek() == &Token::Colon {
                    self.advance();
                }
                label = Some(n);
            }
            let mut items = Vec::new();
            if std::env::var("DBG_GEN").is_ok() {
                let (file, _) = mivon_core::diagnostics::resolve_source_location(
                    &self.source_lines,
                    &self.source_file,
                    self.peek_line(),
                );
                eprintln!(
                    "[DBG-PARSER-GENBODY-AFTER-LABEL] peek={:?}, pos={}, file={}",
                    self.peek(),
                    self.pos.get(),
                    file
                );
            }
            loop {
                if std::env::var("DBG_GEN").is_ok() {
                    let (file, _) = mivon_core::diagnostics::resolve_source_location(
                        &self.source_lines,
                        &self.source_file,
                        self.peek_line(),
                    );
                    eprintln!("[DBG-PARSER-GENBODY-LOOP] peek={:?}, pos={}, items_count={}, file={}, items_ptr={:p}", self.peek(), self.pos.get(), items.len(), file, &items);
                }
                if matches!(self.peek(), Token::End | Token::Eof) {
                    self.advance();
                    // Handle optional : name after end
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    break;
                }
                let pos_before = self.pos.get();
                match self.parse_module_item()? {
                    Some(mi) => {
                        if std::env::var("DBG_GEN").is_ok() {
                            let (file, _) = mivon_core::diagnostics::resolve_source_location(
                                &self.source_lines,
                                &self.source_file,
                                self.peek_line(),
                            );
                            eprintln!(
                                "[DBG-PARSER-GENBODY] parsed item: {:?}, depth={}, file={}",
                                std::mem::discriminant(&mi),
                                self.recursion_depth,
                                file
                            );
                        }
                        items.push(mi);
                    }
                    None => {
                        // Hanya skip apabila posisi TIDAK bergerak (item tidak
                        // di-handle oleh parser). Sequence / Statement / Dollar
                        // sudah konsumu content; skip akan menghancurkan item
                        // berikutnya dalam generate block.
                        if self.pos.get() == pos_before {
                            self.skip_until_semi_or_end()?;
                        }
                    }
                }
            }
            if std::env::var("DBG_GEN").is_ok() {
                eprintln!(
                    "[DBG-PARSER-GENBODY-RETURN] items count={}, items: {:?}",
                    items.len(),
                    items.iter().map(std::mem::discriminant).collect::<Vec<_>>()
                );
            }
            Ok((label, items))
        } else {
            match self.parse_module_item()? {
                Some(mi) => Ok((None, vec![mi])),
                None => Ok((None, Vec::new())),
            }
        }
    }
}
