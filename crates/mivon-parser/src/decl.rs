// Parser submodule: declaration parsing
// Tanggung jawab: parse_decl, parse_decl_names, parse_enum_members, parse_struct_body,
// parse_typedef, parse_scoped_type_name, parse_type_expr, parse_param_list, parse_range

use super::Parser;
use crate::lexer::*;
use mivon_ast::types::const_eval_simple;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

impl Parser {
    pub(crate) fn parse_scoped_type_name(&mut self) -> Option<DataType> {
        // Check if the next tokens are Ident(::Ident)? — a user-defined type name
        // that should be treated as the type of a declaration (e.g., wire pkg::type varname)
        if let Token::Ident(s) = self.peek() {
            let s = *s;
            let ahead = self.peek_ahead(1).clone();
            if ahead == Token::Scope {
                let pkg = s;
                self.advance(); // consume package name
                self.advance(); // consume ::
                if let Token::Ident(t) = self.peek() {
                    let type_name = *t;
                    self.advance();
                    Some(DataType::UserDefined(Symbol::intern(&format!(
                        "{}::{}",
                        pkg, type_name
                    ))))
                } else {
                    None
                }
            } else if matches!(ahead, Token::Ident(_)) {
                self.advance();
                Some(DataType::UserDefined(s))
            } else {
                None
            }
        } else {
            None
        }
    }

    pub(crate) fn parse_decl(&mut self) -> Result<Decl, SimError> {
        let is_const = self.peek() == &Token::Const;
        if is_const {
            self.advance(); // consume 'const'
        }
        // Skip optional 'var' keyword
        if self.peek() == &Token::Var {
            self.advance(); // consume 'var'
        }
        // `virtual <iface_type>[.<modport>] <name> [= init];`
        // Virtual interface variable di dalam task/function body, class body,
        // atau module body. Tipe dipetakan ke UserDefined; semantik simulasi
        // tidak berbeda (VIF tidak diexecute oleh engine).
        if self.peek() == &Token::Virtual {
            self.advance(); // consume 'virtual'
            if let Token::Ident(iface_type) = self.peek().clone() {
                let iface_sym = iface_type;
                self.advance();
                // Skip optional parametric: `virtual force_if#(.P(1),...)`
                if self.peek() == &Token::Hash {
                    let _ = self.parse_param_block();
                }
                // Skip optional modport `.modport_name`
                if self.peek() == &Token::Dot {
                    self.advance();
                    let _ = self.expect_ident();
                }
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::UserDefined(iface_sym),
                    kind: DeclKind::Logic,
                    names,
                });
            }
            return Err(self.err("expected interface type after virtual"));
        }
        let kind = match self.peek() {
            Token::Wire => DeclKind::Wire,
            Token::Wand => DeclKind::Wand,
            Token::Wor => DeclKind::Wor,
            Token::Tri => DeclKind::Tri,
            Token::Tri0 => DeclKind::Tri0,
            Token::Tri1 => DeclKind::Tri1,
            Token::TriAnd => DeclKind::TriAnd,
            Token::TriOr => DeclKind::TriOr,
            Token::Supply0 => DeclKind::Supply0,
            Token::Supply1 => DeclKind::Supply1,
            Token::Reg => DeclKind::Reg,
            Token::Logic => DeclKind::Logic,
            Token::Int => DeclKind::Int,
            Token::Integer => DeclKind::Integer,
            Token::Bit | Token::Byte | Token::Shortint | Token::Longint | Token::Time => {
                let dt = match self.peek() {
                    Token::Bit => DataType::Bit,
                    Token::Byte => DataType::Byte,
                    Token::Shortint => DataType::Shortint,
                    _ => DataType::Longint,
                };
                self.advance();
                let mut dtype = dt;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                // A wildcard unpacked dimension belongs to the declarator:
                // `spi_data_t storage[*]`.  Do not parse `[*]` as a packed
                // type range, because `*` is not an expression bound.
                let decl_expr_range =
                    if self.peek() == &Token::LBrack && self.peek_ahead(1) != &Token::Star {
                        self.parse_range()?
                    } else {
                        None
                    };
                let mut extra_packed: Vec<(ExprRange, Option<Range>)> = Vec::new();
                while self.peek_is_packed_dim() {
                    if let Some(er) = self.parse_range()? {
                        extra_packed.push((er, None));
                    }
                }
                let names = self.parse_decl_names(decl_expr_range, extra_packed)?;
                self.skip_semi();
                return Ok(Decl {
                    dtype,
                    kind: DeclKind::Logic,
                    names,
                });
            }
            Token::Enum => {
                self.advance();
                let base = match self.peek() {
                    Token::Bit
                    | Token::Logic
                    | Token::Int
                    | Token::Integer
                    | Token::Byte
                    | Token::Shortint
                    | Token::Longint
                    | Token::Time => {
                        let dt = match self.peek() {
                            Token::Bit => DataType::Bit,
                            Token::Logic => DataType::Logic,
                            Token::Int => DataType::Int,
                            Token::Integer => DataType::Integer,
                            Token::Byte => DataType::Byte,
                            Token::Shortint => DataType::Shortint,
                            _ => DataType::Longint,
                        };
                        self.advance();
                        let dt = if self.peek() == &Token::Signed {
                            self.advance();
                            DataType::Signed(Box::new(dt))
                        } else {
                            dt
                        };
                        Some(Box::new(dt))
                    }
                    _ => None,
                };
                let decl_expr_range = if base.is_some() && self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let members = self.parse_enum_members()?;
                let mut extra_packed: Vec<(ExprRange, Option<Range>)> = Vec::new();
                while self.peek_is_packed_dim() {
                    if let Some(er) = self.parse_range()? {
                        extra_packed.push((er, None));
                    }
                }
                let names = self.parse_decl_names(decl_expr_range, extra_packed)?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::EnumType { base, members },
                    kind: DeclKind::Logic,
                    names,
                });
            }
            Token::Struct => {
                self.advance();
                if matches!(self.peek(), Token::Ident(s) if s == "packed") {
                    self.advance();
                }
                let members = self.parse_struct_body()?;
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::StructType { members },
                    kind: DeclKind::Logic,
                    names,
                });
            }
            Token::Union => {
                self.advance();
                if matches!(self.peek(), Token::Ident(s) if s == "packed") {
                    self.advance();
                }
                let members = self.parse_struct_body()?;
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::UnionType { members },
                    kind: DeclKind::Logic,
                    names,
                });
            }
            Token::String => {
                self.advance();
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::String,
                    kind: DeclKind::Reg,
                    names,
                });
            }
            Token::Real => {
                self.advance();
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::Real,
                    kind: DeclKind::Reg,
                    names,
                });
            }
            Token::WReal => {
                self.advance();
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::Real,
                    kind: DeclKind::Wire,
                    names,
                });
            }
            Token::RealTime => {
                self.advance();
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::Realtime,
                    kind: DeclKind::Reg,
                    names,
                });
            }
            Token::Mailbox => {
                self.advance();
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::UserDefined(Symbol::intern("__mailbox")),
                    kind: DeclKind::Reg,
                    names,
                });
            }
            Token::Semaphore => {
                self.advance();
                let names = self.parse_decl_names(None, vec![])?;
                self.skip_semi();
                return Ok(Decl {
                    dtype: DataType::UserDefined(Symbol::intern("__semaphore")),
                    kind: DeclKind::Reg,
                    names,
                });
            }
            Token::Ident(_) => {
                let name = self.expect_ident()?;
                let mut dtype = DataType::UserDefined(name);
                // Handle scoped type: pkg::type
                if self.peek() == &Token::Scope {
                    self.advance();
                    let type_name = self.expect_ident()?;
                    dtype = DataType::UserDefined(Symbol::intern(&format!(
                        "{}::{}",
                        match &dtype {
                            DataType::UserDefined(s) => s.as_str(),
                            _ => "",
                        },
                        type_name
                    )));
                }
                let decl_expr_range =
                    if self.peek() == &Token::LBrack && self.peek_ahead(1) != &Token::Star {
                        self.parse_range()?
                    } else {
                        None
                    };
                let mut extra_packed: Vec<(ExprRange, Option<Range>)> = Vec::new();
                while self.peek_is_packed_dim() {
                    if let Some(er) = self.parse_range()? {
                        extra_packed.push((er, None));
                    }
                }
                let names = self.parse_decl_names(decl_expr_range, extra_packed)?;
                self.skip_semi();
                return Ok(Decl {
                    dtype,
                    kind: DeclKind::Logic,
                    names,
                });
            }
            _ => {
                return Err(self.err(
                    "expected wire/reg/logic/int/byte/shortint/longint/enum/struct/union/wand/wor/tri",
                ));
            }
        };
        self.advance();

        let mut dtype = match kind {
            DeclKind::Logic => DataType::Logic,
            DeclKind::Int => DataType::Int,
            DeclKind::Integer => DataType::Integer,
            _ => DataType::Logic,
        };

        if self.peek() == &Token::Signed {
            self.advance();
            dtype = DataType::Signed(Box::new(dtype));
        }
        if self.peek() == &Token::Unsigned {
            self.advance();
            // unsigned = default, no-op
        }

        let decl_expr_range = if self.peek() == &Token::LBrack && self.peek_ahead(1) != &Token::Star
        {
            self.parse_range()?
        } else {
            None
        };

        // Handle scoped type name after wire/reg/logic: wire pkg::type varname
        // Only try when no range precedes (to avoid misinterpreting "wire [7:0] arr")
        // or when we see :: which is unambiguous scoped type
        let scoped_dtype = if matches!(self.peek(), Token::Ident(_))
            && (decl_expr_range.is_none() || self.peek_ahead(1) == &Token::Scope)
        {
            self.parse_scoped_type_name()
        } else {
            None
        };
        let effective_dtype = scoped_dtype.unwrap_or(dtype);

        let mut extra_packed: Vec<(ExprRange, Option<Range>)> = Vec::new();
        while self.peek_is_packed_dim()
            && !(self.peek() == &Token::LBrack && self.peek_ahead(1) == &Token::Star)
        {
            if let Some(er) = self.parse_range()? {
                extra_packed.push((er, None));
            }
        }

        let names = self.parse_decl_names(decl_expr_range, extra_packed)?;
        if self.peek() == &Token::Semi {
            self.advance();
        } else if !matches!(self.peek(), Token::Eof | Token::Endmodule) {
            // `logic c = 1` tanpa ';' (dilanjutkan `logic d;` di baris
            // berikut) ditelan diam-diam oleh skip_semi lama → kode rusak
            // dianggap valid (E-probe e01). Laporkan di lokasi MASALAH
            // (ujung token terakhir deklarasi — bukan token berikutnya,
            // yang menunjuk baris/module salah ke user) + fix-it sisip ';'
            // di titik yang sama (infra push_warning_code_at tanpa konsumsi
            // token: item berikutnya tetap ter-parse normal).
            let (line, col) = self.missing_semi_loc();
            self.push_warning_code_at(
                DiagCode::ExpectedSemi,
                "expected ';' after declaration — missing semicolon".to_string(),
                line,
                col,
            );
        }

        Ok(Decl {
            dtype: effective_dtype,
            kind,
            names,
        })
    }

    pub(crate) fn parse_decl_names(
        &mut self,
        decl_expr_range: Option<ExprRange>,
        extra_packed_dims: Vec<(ExprRange, Option<Range>)>,
    ) -> Result<Vec<DeclVar>, SimError> {
        let mut names = Vec::new();
        loop {
            let name_tok = self.peek().clone();
            match &name_tok {
                Token::Ident(name) => {
                    self.advance();
                    let mut is_dynamic = false;
                    let mut is_queue = false;
                    let mut is_associative = false;
                    let mut assoc_key_type: Option<DataType> = None;
                    // `[]` langsung setelah nama walaupun TANPA decl_expr_range
                    // (mis. `typedef logic state_t[];` — patung DV). Sebelumnya
                    // hanya diproses bila decl_expr_range Some → parse_decl
                    // tersandung di `[` ("expected wire/reg/...").
                    if decl_expr_range.is_none()
                        && self.peek() == &Token::LBrack
                        && self.peek_ahead(1) == &Token::RBrack
                    {
                        self.advance();
                        self.advance();
                        is_dynamic = true;
                    }
                    if decl_expr_range.is_none()
                        && self.peek() == &Token::LBrack
                        && self.peek_ahead(1) == &Token::Star
                        && self.peek_ahead(2) == &Token::RBrack
                    {
                        self.advance();
                        self.advance();
                        self.advance();
                        is_associative = true;
                        assoc_key_type = Some(DataType::Int);
                    }
                    let (var_expr_range, array_range, array_size_expr) = if decl_expr_range
                        .is_some()
                    {
                        let ar = if self.peek() == &Token::LBrack {
                            if self.peek_ahead(1) == &Token::RBrack {
                                self.advance();
                                self.advance();
                                is_dynamic = true;
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Dollar
                                && self.peek_ahead(2) == &Token::RBrack
                            {
                                self.advance();
                                self.advance();
                                self.advance();
                                is_queue = true;
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Int {
                                // int-key associative array
                                self.advance(); // [
                                self.advance(); // int
                                if self.peek() == &Token::Unsigned {
                                    self.advance();
                                }
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Int);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::String {
                                // string-key associative array
                                self.advance(); // [
                                self.advance(); // string
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::String);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Star
                                && self.peek_ahead(2) == &Token::RBrack
                            {
                                self.advance();
                                self.advance();
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Int);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Bit {
                                // bit-key associative array
                                self.advance();
                                self.advance();
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Bit);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Logic {
                                // logic-key associative array
                                self.advance();
                                self.advance();
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Logic);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Byte {
                                // byte-key associative array
                                self.advance();
                                self.advance();
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Byte);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Shortint {
                                // shortint-key associative array
                                self.advance();
                                self.advance();
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Shortint);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Longint {
                                // longint-key associative array
                                self.advance();
                                self.advance();
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Longint);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Star
                                && self.peek_ahead(2) == &Token::RBrack
                            {
                                // wildcard [*] associative array
                                self.advance();
                                self.advance();
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Int);
                                (None, None)
                            } else if self.peek_ahead(1) == &Token::Colon
                                || self.peek_ahead(2) == &Token::Colon
                                || self.peek_bracket_has_range_colon()
                            {
                                // `[msb:lsb]` unpacked — bukan size-expr.
                                let er = self.parse_range()?;
                                let r = er.as_ref().and_then(|er| {
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
                                });
                                // Range tak bisa di-resolve saat parse (bound
                                // pakai enum member / parameter, mis.
                                // `[PmEnLastPos-1:0]`): simpan ukuran
                                // `|msb-lsb|+1` sebagai size-expr agar
                                // elaborator bisa menyelesaikannya jadi
                                // `array_range`. WAJIB simetris — dulu hanya
                                // `(msb-lsb)+1` yang salah untuk rentang
                                // terbalik `[0:N-1]` (picorv32 `cpuregs`,
                                // hasil negatif → sinyal diam-diam flat).
                                let sz_expr = match (er.as_ref(), &r) {
                                    (Some(er), None) => {
                                        let span = |a: &Expr, b: &Expr| Expr::BinaryOp {
                                            op: BinaryOp::Sub,
                                            lhs: Box::new(a.clone()),
                                            rhs: Box::new(b.clone()),
                                        };
                                        let plus_one = |e: Expr| Expr::BinaryOp {
                                            op: BinaryOp::Add,
                                            lhs: Box::new(e),
                                            rhs: Box::new(Expr::Value(Value::Decimal(1))),
                                        };
                                        Some(Expr::TernaryOp {
                                            cond: Box::new(Expr::BinaryOp {
                                                op: BinaryOp::Ge,
                                                lhs: Box::new(er.msb.clone()),
                                                rhs: Box::new(er.lsb.clone()),
                                            }),
                                            true_expr: Box::new(plus_one(span(&er.msb, &er.lsb))),
                                            false_expr: Box::new(plus_one(span(&er.lsb, &er.msb))),
                                        })
                                    }
                                    _ => None,
                                };
                                (r, sz_expr)
                            } else {
                                // `[N]` / `[Width]`: unpacked array size. Resolve
                                // literal sekarang; simpan ekspresi untuk parameter.
                                self.advance(); // [
                                let sz = self.parse_expr(0)?;
                                self.expect(Token::RBrack)?;
                                match const_eval_simple(&sz) {
                                    Ok(n) if n > 0 => (
                                        Some(Range {
                                            msb: (n - 1) as usize,
                                            lsb: 0,
                                        }),
                                        None,
                                    ),
                                    _ => (None, Some(sz)),
                                }
                            }
                        } else {
                            (None, None)
                        };
                        (decl_expr_range.clone(), ar.0, ar.1)
                    } else {
                        if self.peek() == &Token::LBrack {
                            if self.peek_ahead(1) == &Token::RBrack {
                                self.advance();
                                self.advance();
                                is_dynamic = true;
                                (None, None, None)
                            } else if self.peek_ahead(1) == &Token::Dollar
                                && self.peek_ahead(2) == &Token::RBrack
                            {
                                self.advance();
                                self.advance();
                                self.advance();
                                is_queue = true;
                                (None, None, None)
                            } else if self.peek_ahead(1) == &Token::Star
                                && self.peek_ahead(2) == &Token::RBrack
                            {
                                // wildcard [*] associative array (tanpa packed range)
                                self.advance(); // [
                                self.advance(); // *
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Int);
                                (None, None, None)
                            } else if self.peek_ahead(1) == &Token::Int {
                                // int-key associative array (tanpa packed range)
                                self.advance(); // [
                                self.advance(); // int
                                if self.peek() == &Token::Unsigned {
                                    self.advance();
                                }
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::Int);
                                (None, None, None)
                            } else if self.peek_ahead(1) == &Token::String {
                                // string-key associative array (tanpa packed range)
                                self.advance(); // [
                                self.advance(); // string
                                self.expect(Token::RBrack)?;
                                is_associative = true;
                                assoc_key_type = Some(DataType::String);
                                (None, None, None)
                            } else if self.peek_ahead(1) != &Token::Colon
                                && !self.peek_bracket_has_range_colon()
                            {
                                // `[N]` / `[Width]`: unpacked array size.
                                self.advance(); // [
                                let sz = self.parse_expr(0)?;
                                self.expect(Token::RBrack)?;
                                match const_eval_simple(&sz) {
                                    Ok(n) if n > 0 => (
                                        None,
                                        Some(Range {
                                            msb: (n - 1) as usize,
                                            lsb: 0,
                                        }),
                                        None,
                                    ),
                                    _ => (None, None, Some(sz)),
                                }
                            } else {
                                // `[msb:lsb]` SETELAH nama TANPA packed-range =
                                // UNPACKED ARRAY (SV: packed hanya sebelum nama).
                                // Fix Bug-1: `logic f5 [0:N-1]` kini array 0..N-1
                                // (bukan packed 128-bit) → elemen 1-bit index ok.
                                let er = self.parse_range()?;
                                match er {
                                    Some(er) => {
                                        if let (Ok(m), Ok(l)) =
                                            (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                                        {
                                            (
                                                None,
                                                Some(Range {
                                                    msb: m as usize,
                                                    lsb: l as usize,
                                                }),
                                                None,
                                            )
                                        } else {
                                            // Range dgn bound param — size-expr
                                            // |msb-lsb|+1 (ternary utk arah).
                                            let span = |a: &Expr, b: &Expr| Expr::BinaryOp {
                                                op: BinaryOp::Sub,
                                                lhs: Box::new(a.clone()),
                                                rhs: Box::new(b.clone()),
                                            };
                                            let plus_one = |e: Expr| Expr::BinaryOp {
                                                op: BinaryOp::Add,
                                                lhs: Box::new(e),
                                                rhs: Box::new(Expr::Value(Value::Decimal(1))),
                                            };
                                            let sz_expr = Some(Expr::TernaryOp {
                                                cond: Box::new(Expr::BinaryOp {
                                                    op: BinaryOp::Ge,
                                                    lhs: Box::new(er.msb.clone()),
                                                    rhs: Box::new(er.lsb.clone()),
                                                }),
                                                true_expr: Box::new(plus_one(span(
                                                    &er.msb, &er.lsb,
                                                ))),
                                                false_expr: Box::new(plus_one(span(
                                                    &er.lsb, &er.msb,
                                                ))),
                                            });
                                            (None, None, sz_expr)
                                        }
                                    }
                                    None => (None, None, None),
                                }
                            }
                        } else {
                            (None, None, None)
                        }
                    };
                    // Dimensi unpacked LANJUTAN `[..][..]` (mis. `bit a[4][8]`,
                    // `logic [7:0] mat [0:1][0:1]`) — tiap `[` berikutnya
                    // di-parse sebagai range `[msb:lsb]` / size `[N]` dan
                    // disimpan ke `extra_unpacked_dims` (F39). Sebelumnya
                    // di-skip buta → array multi-dim jadi flat single-dim
                    // (bug: `mat[0][0]` = 0, lebar total salah).
                    // Bentuk eksotik (`[string]`, `[$]`, `[*]`, key type
                    // `[addr_data_t]`) tetap di-skip buta (perilaku lama).
                    let mut extra_unpacked_dims: Vec<(Option<Range>, Option<Expr>)> = Vec::new();
                    while self.peek() == &Token::LBrack {
                        let ahead = self.peek_ahead(1);
                        // Hanya fixed dim: `[N]` / `[msb:lsb]` (bukan dynamic/
                        // queue/assoc/key-type, yang tidak valid di dim lanjutan).
                        let fixed = !matches!(
                            ahead,
                            Token::RBrack
                                | Token::Dollar
                                | Token::Star
                                | Token::String
                                | Token::Int
                                | Token::Unsigned
                                | Token::Bit
                                | Token::Logic
                                | Token::Byte
                                | Token::Shortint
                                | Token::Longint
                        );
                        if !fixed {
                            // Blind-skip bracket ini (dan sisanya) — perilaku lama.
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
                            // Sisa bracket berikutnya pun ikut di-skip buta.
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
                            break;
                        }
                        // Fixed dim: coba parse range/size.
                        // CATATAN: `parse_range` mengkonsumsi `[msb:lsb]`
                        // SENDIRI (expect LBrack di dalamnya) — jangan
                        // `advance()` `[` lebih dulu (bug: semua dim lanjutan
                        // jatuh ke Err → blind-skip). Deteksi range lewat
                        // `peek_bracket_has_range_colon` (peek masih `[`).
                        if self.peek_bracket_has_range_colon()
                            || self.peek_ahead(1) == &Token::Colon
                        {
                            match self.parse_range() {
                                Ok(Some(er)) => {
                                    if let (Ok(m), Ok(l)) =
                                        (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                                    {
                                        extra_unpacked_dims.push((
                                            Some(Range {
                                                msb: m as usize,
                                                lsb: l as usize,
                                            }),
                                            None,
                                        ));
                                    } else {
                                        // `|msb-lsb|+1` sebagai size-expr (pola
                                        // simetris dgn dim pertama) — elaborator
                                        // resolve via const_eval_params.
                                        let span = |a: &Expr, b: &Expr| Expr::BinaryOp {
                                            op: BinaryOp::Sub,
                                            lhs: Box::new(a.clone()),
                                            rhs: Box::new(b.clone()),
                                        };
                                        let plus_one = |e: Expr| Expr::BinaryOp {
                                            op: BinaryOp::Add,
                                            lhs: Box::new(e),
                                            rhs: Box::new(Expr::Value(Value::Decimal(1))),
                                        };
                                        let sz_expr = Expr::TernaryOp {
                                            cond: Box::new(Expr::BinaryOp {
                                                op: BinaryOp::Ge,
                                                lhs: Box::new(er.msb.clone()),
                                                rhs: Box::new(er.lsb.clone()),
                                            }),
                                            true_expr: Box::new(plus_one(span(&er.msb, &er.lsb))),
                                            false_expr: Box::new(plus_one(span(&er.lsb, &er.msb))),
                                        };
                                        extra_unpacked_dims.push((None, Some(sz_expr)));
                                    }
                                }
                                Ok(None) | Err(_) => {
                                    // Blind-skip bracket ini.
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
                        } else {
                            // `[N]` / `[Width]` — size-expr.
                            // Const-eval dulu; gagal → (None, Some(sz)).
                            self.advance(); // '['
                            match self.parse_expr(0) {
                                Ok(sz) => {
                                    let _ = self.expect(Token::RBrack);
                                    match const_eval_simple(&sz) {
                                        Ok(n) if n > 0 => {
                                            extra_unpacked_dims.push((
                                                Some(Range {
                                                    msb: (n - 1) as usize,
                                                    lsb: 0,
                                                }),
                                                None,
                                            ));
                                        }
                                        _ => {
                                            extra_unpacked_dims.push((None, Some(sz)));
                                        }
                                    }
                                }
                                Err(_) => {
                                    // Blind-skip bracket ini (isi bukan ekspresi).
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
                        }
                    }
                    let var_range = var_expr_range.as_ref().and_then(|er| {
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
                    });
                    let init_expr = if self.peek() == &Token::BlockingAssign {
                        self.advance();
                        Some(self.parse_expr(0)?)
                    } else {
                        None
                    };
                    names.push(DeclVar {
                        name: *name,
                        range: var_range,
                        expr_range: var_expr_range,
                        array_range,
                        array_size_expr,
                        extra_unpacked_dims,
                        extra_packed_dims: extra_packed_dims.clone(),
                        is_dynamic,
                        is_queue,
                        is_associative,
                        assoc_key_type,
                        is_rand: false,
                        is_const: false,
                        is_static: false,
                        expr: init_expr,
                    });
                }
                _ => break,
            }

            if self.peek() == &Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(names)
    }

    pub(crate) fn parse_enum_members(&mut self) -> Result<Vec<(Symbol, Option<Expr>)>, SimError> {
        self.expect(Token::LBrace)?;
        let mut members = Vec::new();
        loop {
            match self.peek() {
                Token::Ident(name) => {
                    let name = *name;
                    self.advance();
                    let range: Option<(i64, Option<i64>)> = if self.peek() == &Token::LBrack {
                        self.advance();
                        let lo = self.parse_enum_range_literal()?;
                        let hi = if self.peek() == &Token::Colon {
                            self.advance();
                            Some(self.parse_enum_range_literal()?)
                        } else {
                            None
                        };
                        self.expect(Token::RBrack)?;
                        Some((lo, hi))
                    } else {
                        None
                    };
                    let val = if matches!(self.peek(), Token::Eq | Token::BlockingAssign) {
                        self.advance();
                        Some(self.parse_expr(0)?)
                    } else {
                        None
                    };
                    match range {
                        None => members.push((name, val)),
                        Some((lo, hi)) => {
                            let indices: Vec<i64> = match hi {
                                // name[N] -> name0 .. nameN-1
                                None => (0..lo).collect(),
                                // name[N:M] -> nameN .. nameM (inclusive, direction follows N->M)
                                Some(m) => {
                                    if lo <= m {
                                        (lo..=m).collect()
                                    } else {
                                        (m..=lo).rev().collect()
                                    }
                                }
                            };
                            for (k, idx) in indices.into_iter().enumerate() {
                                let member_val = if k == 0 { val.clone() } else { None };
                                let member_name =
                                    Symbol::intern(&format!("{}{}", name.as_str(), idx));
                                members.push((member_name, member_val));
                            }
                        }
                    }
                }
                _ => return Err(self.err("expected identifier in enum")),
            }
            if self.peek() == &Token::Comma {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(Token::RBrace)?;
        Ok(members)
    }

    /// Parse integer literal untuk enum range (hanya literal konstan per LRM).
    fn parse_enum_range_literal(&mut self) -> Result<i64, SimError> {
        match self.peek().clone() {
            Token::Number { value, base, .. } => {
                let s = value.as_str().to_string();
                self.advance();
                let n = match base {
                    None => s.parse::<i64>(),
                    Some(10) => s.parse::<i64>(),
                    Some(b) => i64::from_str_radix(&s, b as u32),
                }
                .map_err(|_| self.err("invalid integer literal in enum range"))?;
                Ok(n)
            }
            _ => Err(self.err("expected integer literal in enum range")),
        }
    }

    pub(crate) fn parse_struct_body(&mut self) -> Result<Vec<StructMember>, SimError> {
        self.push_depth()?;
        let result = self.parse_struct_body_impl();
        self.pop_depth();
        result
    }

    fn parse_struct_body_impl(&mut self) -> Result<Vec<StructMember>, SimError> {
        self.expect(Token::LBrace)?;
        let mut members = Vec::new();
        loop {
            if self.peek() == &Token::RBrace {
                self.advance();
                return Ok(members);
            }
            // `rand`/`randc` modifier sebelum tipe field (pola DV umum:
            // `rand bit [7:0] len;` di struct env pkg). Rand tidak bermakna
            // untuk struct non-class; skip.
            if matches!(self.peek(), Token::Rand | Token::RandC) {
                self.advance();
            }
            // `static`/`local` modifier (jarang di struct) — skip aman.
            if matches!(self.peek(), Token::Static) {
                self.advance();
            }
            let member_type = match self.peek() {
                Token::Logic => {
                    self.advance();
                    DataType::Logic
                }
                Token::Int => {
                    self.advance();
                    DataType::Int
                }
                Token::Integer => {
                    self.advance();
                    DataType::Integer
                }
                Token::Bit => {
                    self.advance();
                    DataType::Bit
                }
                Token::Byte => {
                    self.advance();
                    DataType::Byte
                }
                Token::Shortint => {
                    self.advance();
                    DataType::Shortint
                }
                Token::Longint => {
                    self.advance();
                    DataType::Longint
                }
                Token::Time => {
                    self.advance();
                    DataType::Time
                }
                Token::Reg => {
                    self.advance();
                    DataType::Logic
                }
                Token::String => {
                    self.advance();
                    DataType::String
                }
                Token::Signed => {
                    self.advance();
                    let inner = match self.peek() {
                        Token::Bit => {
                            self.advance();
                            DataType::Bit
                        }
                        Token::Logic => {
                            self.advance();
                            DataType::Logic
                        }
                        Token::Int => {
                            self.advance();
                            DataType::Int
                        }
                        Token::Integer => {
                            self.advance();
                            DataType::Integer
                        }
                        Token::Byte => {
                            self.advance();
                            DataType::Byte
                        }
                        Token::Shortint => {
                            self.advance();
                            DataType::Shortint
                        }
                        Token::Longint => {
                            self.advance();
                            DataType::Longint
                        }
                        Token::Time => {
                            self.advance();
                            DataType::Time
                        }
                        _ => DataType::Logic,
                    };
                    DataType::Signed(Box::new(inner))
                }
                Token::Struct => {
                    self.advance();
                    if matches!(self.peek(), Token::Ident(s) if s == "packed") {
                        self.advance();
                    }
                    DataType::StructType {
                        members: self.parse_struct_body()?,
                    }
                }
                Token::Ident(name) => {
                    let name = *name;
                    self.advance();
                    // Handle scoped type: pkg::type
                    if self.peek() == &Token::Scope {
                        self.advance();
                        let type_name = self.expect_ident()?;
                        DataType::UserDefined(Symbol::intern(&format!("{}::{}", name, type_name)))
                    } else {
                        DataType::UserDefined(name)
                    }
                }
                _ => return Err(self.err("expected type in struct/union member")),
            };
            // Modifier signed/unsigned SETELAH tipe dasar: `int unsigned id;`
            // (umum di struct OpenTitan). unsigned = no-op; signed dibungkus.
            let member_type = if self.peek() == &Token::Signed {
                self.advance();
                DataType::Signed(Box::new(member_type))
            } else if self.peek() == &Token::Unsigned {
                self.advance();
                member_type
            } else {
                member_type
            };
            let (range, expr_range) = if self.peek() == &Token::LBrack {
                let er = self.parse_range()?;
                let resolved = er.as_ref().and_then(|er| {
                    if let (Ok(m), Ok(l)) = (const_eval_simple(&er.msb), const_eval_simple(&er.lsb))
                    {
                        Some(Range {
                            msb: m as usize,
                            lsb: l as usize,
                        })
                    } else {
                        None
                    }
                });
                (resolved, er)
            } else {
                (None, None)
            };
            self.skip_extra_packed_dims()?;
            let name = self.expect_ident()?;
            // Unpacked array dims setelah nama: `bit [3:0] [31:0] plain_text[4]`
            // (struktur data OpenTitan). Bisa beberapa dims: `name[4][8]`,
            // dynamic `name[]`, queue `name[$]`.
            while self.peek() == &Token::LBrack {
                if self.peek_is_packed_dim() {
                    let _ = self.parse_range()?;
                } else if self.peek_ahead(1) == &Token::RBrack {
                    // `[]` — dynamic array member (tipe argumen DV dgn
                    // data dinamis; pola `arg_t arg[]` di struct).
                    self.advance();
                    self.advance();
                } else if self.peek_ahead(1) == &Token::Dollar
                    && self.peek_ahead(2) == &Token::RBrack
                {
                    // `[$]` — queue member.
                    self.advance();
                    self.advance();
                    self.advance();
                } else {
                    self.advance(); // '['
                    let _ = self.parse_expr(0)?;
                    self.expect(Token::RBrack)?;
                }
            }
            self.skip_semi();
            members.push(StructMember {
                name,
                dtype: Box::new(member_type),
                range,
                expr_range,
            });
        }
    }

    /// LANG-40: `let name[(params)] = expr;` (IEEE 1800-2017 §11.12.2).
    /// Mengonsumsi token `let` lalu memparse nama, parameter opsional, `=`,
    /// ekspresi body, dan semicolon.
    pub(crate) fn parse_let_decl(&mut self) -> Result<LetDecl, SimError> {
        self.advance(); // consume 'let'
        let lname = self.expect_ident()?;
        let mut params = Vec::new();
        if self.peek() == &Token::LParen {
            self.advance();
            if self.peek() != &Token::RParen {
                loop {
                    params.push(self.expect_ident()?);
                    if self.peek() == &Token::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RParen)?;
        }
        self.expect(Token::BlockingAssign)?;
        let lexpr = self.parse_expr(0)?;
        self.skip_semi();
        Ok(LetDecl {
            name: lname,
            params,
            expr: lexpr,
        })
    }

    pub(crate) fn parse_typedef(&mut self) -> Result<TypedefDecl, SimError> {
        self.advance(); // consume typedef
        let (name, dtype, range, extra_packed_dims) = match self.peek() {
            // Forward class declaration: `typedef class foo;` (LRM 1800 §6.18).
            Token::Class => {
                self.advance();
                let name = self.expect_ident()?;
                self.skip_semi();
                (name, DataType::UserDefined(name), None, Vec::new())
            }
            Token::Enum => {
                self.advance();
                let base = match self.peek() {
                    Token::Bit
                    | Token::Logic
                    | Token::Int
                    | Token::Integer
                    | Token::Byte
                    | Token::Shortint
                    | Token::Longint
                    | Token::Time => {
                        let dt = match self.peek() {
                            Token::Bit => DataType::Bit,
                            Token::Logic => DataType::Logic,
                            Token::Int => DataType::Int,
                            Token::Integer => DataType::Integer,
                            Token::Byte => DataType::Byte,
                            Token::Shortint => DataType::Shortint,
                            _ => DataType::Longint,
                        };
                        self.advance();
                        let dt = if self.peek() == &Token::Signed {
                            self.advance();
                            DataType::Signed(Box::new(dt))
                        } else {
                            dt
                        };
                        if self.peek() == &Token::Unsigned {
                            self.advance();
                        }
                        Some(Box::new(dt))
                    }
                    // User-defined base type: typedef enum lc_state_t {...} or
                    // typedef enum pkg::type {...}
                    Token::Ident(name) => {
                        let name = *name;
                        self.advance();
                        let dtype = if self.peek() == &Token::Scope {
                            self.advance();
                            let type_name = self.expect_ident()?;
                            DataType::UserDefined(Symbol::intern(&format!(
                                "{}::{}",
                                name, type_name
                            )))
                        } else {
                            DataType::UserDefined(name)
                        };
                        Some(Box::new(dtype))
                    }
                    _ => None,
                };
                if base.is_some() && self.peek() == &Token::LBrack {
                    let er = self.parse_range()?;
                    let members = self.parse_enum_members()?;
                    if let Token::Ident(name) = self.peek() {
                        let name = *name;
                        self.advance();
                        (name, DataType::EnumType { base, members }, er, Vec::new())
                    } else {
                        return Err(self.err("expected name after typedef enum"));
                    }
                } else {
                    let members = self.parse_enum_members()?;
                    if let Token::Ident(name) = self.peek() {
                        let name = *name;
                        self.advance();
                        (name, DataType::EnumType { base, members }, None, Vec::new())
                    } else {
                        return Err(self.err("expected name after typedef enum"));
                    }
                }
            }
            Token::Bit => {
                self.advance();
                let mut dtype = DataType::Bit;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef bit"));
                }
            }
            Token::Byte => {
                self.advance();
                let mut dtype = DataType::Byte;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef byte"));
                }
            }
            Token::Shortint => {
                self.advance();
                let mut dtype = DataType::Shortint;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef shortint"));
                }
            }
            Token::Longint => {
                self.advance();
                let mut dtype = DataType::Longint;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef longint"));
                }
            }
            Token::Time => {
                self.advance();
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, DataType::Time, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef time"));
                }
            }
            Token::Int => {
                self.advance();
                let mut dtype = DataType::Int;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef int"));
                }
            }
            Token::Integer => {
                self.advance();
                let mut dtype = DataType::Integer;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef integer"));
                }
            }
            Token::Logic => {
                self.advance();
                let mut dtype = DataType::Logic;
                if self.peek() == &Token::Signed {
                    self.advance();
                    dtype = DataType::Signed(Box::new(dtype));
                }
                if self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef logic"));
                }
            }
            Token::Reg => {
                self.advance();
                let dtype = DataType::Logic;
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef reg"));
                }
            }
            Token::Struct => {
                self.advance();
                if matches!(self.peek(), Token::Ident(s) if s == "packed") {
                    self.advance();
                }
                let members = self.parse_struct_body()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, DataType::StructType { members }, None, Vec::new())
                } else {
                    return Err(self.err("expected name after typedef struct"));
                }
            }
            Token::Union => {
                self.advance();
                if matches!(self.peek(), Token::Ident(s) if s == "packed") {
                    self.advance();
                }
                let members = self.parse_struct_body()?;
                if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    (name, DataType::UnionType { members }, None, Vec::new())
                } else {
                    return Err(self.err("expected name after typedef union"));
                }
            }
            // User-defined base type: typedef some_type_t name; or
            // typedef some_type_t [range] name; or typedef pkg::type name;
            Token::Ident(_) => {
                let type_name = self.expect_ident()?;
                let mut dtype = DataType::UserDefined(type_name);
                if self.peek() == &Token::Scope {
                    self.advance();
                    let t = self.expect_ident()?;
                    dtype = DataType::UserDefined(Symbol::intern(&format!(
                        "{}::{}",
                        match &dtype {
                            DataType::UserDefined(s) => s.as_str(),
                            _ => "",
                        },
                        t
                    )));
                }
                // Class parameterization: typedef some_class #(.P(...), ...) name;
                // Parameter values diabaikan untuk DataType::UserDefined.
                if self.peek() == &Token::Hash {
                    self.advance();
                    if self.peek() == &Token::LParen {
                        self.skip_balanced_paren_light()?;
                    }
                }
                let range = if self.peek() == &Token::LBrack {
                    self.parse_range()?
                } else {
                    None
                };
                let extra_packed_dims = self.parse_extra_packed_dims()?;
                if matches!(self.peek(), Token::Semi) {
                    // `typedef name;` — alias ke NAMA SENDIRI (LRM 1800 §6.18:
                    // "identical alias"). Muncul di package yang di-include
                    // dua kali via guard berbeda (otbn_model_agent_pkg:
                    // `typedef otbn_model_item;` dengan tipe belum tersedia
                    // saat parse file tunggal). Registrasi no-op — JANGAN
                    // error; typedef lengkap sesungguhnya yang menang.
                    (type_name, dtype, None, Vec::new())
                } else if let Token::Ident(name) = self.peek() {
                    let name = *name;
                    self.advance();
                    // Queue typedef: `name [$]` / `name [$:N]` (unbounded /
                    // bounded queue). mivon tidak memodelkan queue sebagai
                    // tipe, tapi JANGAN error — pola umum DV OpenTitan
                    // (`typedef spi_data_t spi_queue_t [$];` di spid_common).
                    while self.peek() == &Token::LBrack
                        && matches!(self.peek_ahead(1), Token::Dollar)
                    {
                        self.advance(); // [
                        self.advance(); // $
                        if self.peek() == &Token::Colon {
                            self.advance();
                            let _ = self.parse_expr(0);
                        }
                        self.expect(Token::RBrack)?;
                    }
                    (name, dtype, range, extra_packed_dims)
                } else {
                    return Err(self.err("expected name after typedef type"));
                }
            }
            _ => return Err(self.err("expected type after typedef")),
        };
        // Unpacked dims SETELAH nama typedef (`[]`, `[$]`, `[type]`, `[N]`)
        // — konsumsi untuk semua branch (logic/bit/int/enum/struct/user-defined).
        self.skip_typedef_unpacked_dims()?;
        self.skip_semi();
        Ok(TypedefDecl {
            name,
            dtype,
            range,
            extra_packed_dims,
        })
    }

    pub(crate) fn parse_type_expr(&mut self) -> Result<DataType, SimError> {
        // `virtual <iface_type>` — tipe virtual interface (param class UVM,
        // mis. `uvm_config_db#(virtual alert_esc_if)::get(...)`). Marker
        // `virtual` dibuang; tipe interface diterjemahkan sbg UserDefined.
        if self.peek() == &Token::Virtual {
            self.advance();
            if let Token::Ident(name) = self.peek() {
                let name = *name;
                self.advance();
                // Type virtual interface parametrik: `virtual force_if#(.P(1),...)`
                // (param class UVM, mis. `uvm_config_db#(virtual force_if#(...))`).
                // Parametrik dibuang — tipe dipetakan ke UserDefined.
                if self.peek() == &Token::Hash {
                    let _ = self.parse_param_block()?;
                }
                return Ok(DataType::UserDefined(name));
            }
            return Err(self.err("expected interface type after virtual"));
        }
        let dt = match self.peek() {
            Token::Struct => {
                self.advance();
                if matches!(self.peek(), Token::Ident(s) if s == "packed") {
                    self.advance();
                }
                DataType::StructType {
                    members: self.parse_struct_body()?,
                }
            }
            Token::Union => {
                self.advance();
                if matches!(self.peek(), Token::Ident(s) if s == "packed") {
                    self.advance();
                }
                DataType::UnionType {
                    members: self.parse_struct_body()?,
                }
            }
            Token::Bit => {
                self.advance();
                DataType::Bit
            }
            Token::Logic => {
                self.advance();
                DataType::Logic
            }
            Token::Int => {
                self.advance();
                DataType::Int
            }
            Token::Integer => {
                self.advance();
                DataType::Integer
            }
            Token::Byte => {
                self.advance();
                DataType::Byte
            }
            Token::Shortint => {
                self.advance();
                DataType::Shortint
            }
            Token::Longint => {
                self.advance();
                DataType::Longint
            }
            Token::Time => {
                self.advance();
                DataType::Time
            }
            Token::Reg => {
                self.advance();
                DataType::Logic
            }
            Token::Real => {
                self.advance();
                DataType::Real
            }
            Token::RealTime => {
                self.advance();
                DataType::Realtime
            }
            Token::String => {
                self.advance();
                DataType::String
            }
            Token::Ident(_) => {
                // PARSER-10: user-defined type, possibly scoped (pkg::type_name)
                let name = self.expect_ident()?;
                if self.peek() == &Token::Scope {
                    self.advance(); // ::
                    let _inner = self.expect_ident()?;
                    DataType::UserDefined(_inner)
                } else {
                    // Type parametrik `force_if#(.P(1),...)` — param dibuang,
                    // tipe dipetakan ke UserDefined (mis. `#(virtual x_if#(...))`).
                    if self.peek() == &Token::Hash {
                        let _ = self.parse_param_block()?;
                    }
                    DataType::UserDefined(name)
                }
            }
            _ => return Err(self.err("expected type")),
        };
        // `.T ( logic [7:0] )` — arg type-param dgn packed range
        // (axi_cdc_dst.sv cva6: `logic [$bits(aw_chan_t)-1:0]`). DataType
        // tidak punya variant range — parse & BUANG (sama spt discard
        // parametrik `#(...)` di atas). Parse-accept; lebar jadian urusan
        // elaborator (WR0102 bila salah). Stopgap korpus, bukan semantik penuh.
        if self.peek() == &Token::LBrack {
            let _ = self.parse_range()?;
            while self.peek() == &Token::LBrack {
                let _ = self.parse_range()?;
            }
        }
        if self.peek() == &Token::Signed {
            self.advance();
            Ok(DataType::Signed(Box::new(dt)))
        } else if self.peek() == &Token::Unsigned {
            // `int unsigned` — unsigned = default, no-op.
            self.advance();
            Ok(dt)
        } else {
            Ok(dt)
        }
    }

    pub(crate) fn parse_param_list(&mut self, params: &mut Vec<ParamDecl>) -> Result<(), SimError> {
        let mut is_localparam = false;
        loop {
            match self.peek() {
                Token::Param | Token::Parameter => {
                    is_localparam = false;
                    self.advance();
                }
                Token::LocalParam => {
                    is_localparam = true;
                    self.advance();
                }
                _ => {}
            }

            // Skip optional type keyword (integer, int, reg, logic, bit, string, ...).
            // `signed`/`unsigned` di sini adalah MODIFIER (dikonsumsi di bawah,
            // bukan bagian list) agar `parameter signed int W` tidak salah-
            // parse: dulu `signed` dikonsumsi sbg "tipe" lalu `int` dianggap
            // NAMA → E1002 'expected RParen, found SW' (probe p18).
            let mut lead_sign: Option<bool> = None;
            if matches!(self.peek(), Token::Signed | Token::Unsigned) {
                lead_sign = Some(self.peek() == &Token::Signed);
                self.advance();
            }
            let mut type_ident = None;
            match self.peek() {
                Token::Integer
                | Token::Int
                | Token::Reg
                | Token::Logic
                | Token::Bit
                | Token::String
                | Token::Byte
                | Token::Shortint
                | Token::Longint
                | Token::Real
                | Token::RealTime
                | Token::Time => {
                    self.advance();
                }
                Token::Ident(_)
                    if matches!(
                        self.peek_ahead(1),
                        Token::Ident(_) | Token::LBrack | Token::Scope
                    ) =>
                {
                    // User-defined type: ident followed by name, range, or ::
                    if let Token::Ident(s) = self.peek() {
                        type_ident = Some(*s);
                        self.advance();
                        // Handle scoped type: pkg::type
                        if self.peek() == &Token::Scope {
                            self.advance();
                            let _ = self.expect_ident();
                        }
                    }
                }
                _ => {}
            }

            // Modifier signed/unsigned SETELAH tipe (`int signed`, `logic unsigned`).
            let mut trail_sign: Option<bool> = None;
            if self.peek() == &Token::Signed {
                self.advance();
                trail_sign = Some(true);
            }
            if self.peek() == &Token::Unsigned {
                self.advance();
                trail_sign = Some(false);
            }

            // Parse optional range(s): [msb:lsb] or [msb:lsb][msb:lsb]...
            let mut range = None;
            if self.peek() == &Token::LBrack {
                self.advance();
                let msb = self.parse_expr(0)?;
                self.expect(Token::Colon)?;
                let lsb = self.parse_expr(0)?;
                self.expect(Token::RBrack)?;
                range = Some((msb, lsb));
                // Skip additional packed dimensions [a:b] (used in packed arrays like logic [3:0][1:0])
                while self.peek() == &Token::LBrack {
                    self.advance();
                    self.parse_expr(0)?;
                    self.expect(Token::Colon)?;
                    self.parse_expr(0)?;
                    self.expect(Token::RBrack)?;
                }
            }

            let tok = self.peek().clone();
            match tok {
                Token::Ident(_) | Token::Int | Token::Integer | Token::Type | Token::LBrack => {}
                _ => break,
            }

            let is_type_param = self.peek() == &Token::Type;
            if is_type_param {
                self.advance(); // consume 'type'
            }

            let name_tok = self.peek().clone();
            let name = match &name_tok {
                Token::Ident(s) => {
                    self.advance();
                    s.as_str().to_string()
                }
                Token::Int => {
                    self.advance();
                    "int".to_string()
                }
                Token::Integer => {
                    self.advance();
                    "integer".to_string()
                }
                _ => break,
            };

            // Type param dari header (module m #(parameter type T = int))
            // juga harus terdaftar agar `T x;` di body diparse sebagai deklarasi.
            if is_type_param {
                self.module_type_params.insert(Symbol::intern(&name));
            }

            let mut dtype = None;
            if self.peek() == &Token::Signed {
                self.advance();
                dtype = Some(DataType::Signed(Box::new(DataType::Int)));
            }

            // Skip unpacked array dimension(s) after name:
            // name [N] atau name [msb:lsb] (multi-dimensi diperbolehkan)
            while self.peek() == &Token::LBrack {
                self.advance(); // [
                let _ = self.parse_expr(0);
                if self.peek() == &Token::Colon {
                    self.advance();
                    let _ = self.parse_expr(0);
                }
                self.expect(Token::RBrack)?;
            }

            // Type default untuk `parameter type T = <type>` (mis. `T = int`,
            // `T = logic`). Dipakai elaborator untuk menghitung lebar deklarasi
            // `T x;` (int→32, logic→1, byte→8, dst) — sebelumnya selalu None
            // sehingga `T x;` jatuh ke fallback lebar 1.
            let mut type_default: Option<DataType> = None;
            let default = if self.peek() == &Token::BlockingAssign {
                self.advance();
                if is_type_param {
                    // Parse default type expression: logic [7:0], bit, int, etc.
                    type_default = Some(self.parse_type_expr()?);
                    // F32 fix: SIMPAN range default type param (`T = logic [7:0]`)
                    // ke `range` (field ParamDecl yang sama utk `parameter [7:0] W`).
                    // Sebelumnya range di-parse lalu DIBUANG → `T` selalu 1-bit
                    // (type_default = DataType::Logic tanpa range). Elaborator
                    // menghitung lebar type param dari `param.range`.
                    if self.peek() == &Token::LBrack {
                        self.advance();
                        let msb = self.parse_expr(0)?;
                        self.expect(Token::Colon)?;
                        let lsb = self.parse_expr(0)?;
                        self.expect(Token::RBrack)?;
                        range = Some((msb, lsb));
                    }
                    // For MVP, store dummy expression; width resolved in elaborator
                    Some(Expr::Value(Value::Decimal(0)))
                } else {
                    Some(self.parse_expr(0)?)
                }
            } else {
                None
            };

            // Use type_ident as UserDefined dtype if set
            let resolved_dtype = type_ident
                .as_ref()
                .map(|t| DataType::UserDefined(*t))
                .or(dtype);
            // Terapkan modifier signed/unsigned (lead/trail) — `parameter
            // signed int W` → Signed(Int) agar elaborator memperlakukannya
            // signed (probe p18).
            let resolved_dtype = match lead_sign.or(trail_sign) {
                Some(sig) => Some(crate::apply_sign_mod(resolved_dtype, sig)),
                None => resolved_dtype,
            };

            params.push(ParamDecl {
                name: Symbol::intern(&name),
                dtype: resolved_dtype,
                range,
                default,
                is_localparam,
                is_type_param,
                type_default,
            });

            if self.peek() == &Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn parse_range(&mut self) -> Result<Option<ExprRange>, SimError> {
        self.expect(Token::LBrack)?;
        let msb = self.parse_expr(0)?;
        if self.peek() != &Token::Colon {
            // `[N]` — size-only, bukan `[msb:lsb]`. Normalkan ke `[N-1:0]`.
            self.expect(Token::RBrack)?;
            return Ok(Some(ExprRange {
                msb: Expr::BinaryOp {
                    op: BinaryOp::Sub,
                    lhs: Box::new(msb),
                    rhs: Box::new(Expr::Value(Value::Decimal(1))),
                },
                lsb: Expr::Value(Value::Decimal(0)),
            }));
        }
        self.expect(Token::Colon)?;
        let lsb = self.parse_expr(0)?;
        self.expect(Token::RBrack)?;
        Ok(Some(ExprRange { msb, lsb }))
    }

    /// Skip packed dimensions tambahan `[msb:lsb][msb:lsb]...` setelah range pertama.
    /// Dipakai di typedef agar `typedef logic [W-1:0][N-1:0] name;` tidak gagal parse.
    pub(crate) fn skip_extra_packed_dims(&mut self) -> Result<(), SimError> {
        let _ = self.parse_extra_packed_dims()?;
        Ok(())
    }

    /// Consume UNPACKED dimensions setelah nama typedef — `[]` (dynamic),
    /// `[$]` / `[$:N]` (queue), `[type_t]` / `[type_t:type_t]` (associative),
    /// `[N]` (fixed unpacked). Bentuk `typedef logic state_t[];`,
    /// `typedef logic [7:0] fq_t[$];`, `typedef data_t mem_t[addr_t];`
    /// (umum di DV OpenTitan). Isi bracket dilewati balance-aware terhadap
    /// bracket bersarang dan kurung ekspresi.
    pub(crate) fn skip_typedef_unpacked_dims(&mut self) -> Result<(), SimError> {
        while self.peek() == &Token::LBrack {
            let mut depth: i64 = 0;
            loop {
                match self.peek() {
                    Token::Eof => {
                        return Err(self.err("unexpected EOF in typedef unpacked dimension"))
                    }
                    Token::LBrack => {
                        depth += 1;
                        self.advance();
                    }
                    Token::RBrack => {
                        depth -= 1;
                        self.advance();
                        if depth <= 0 {
                            break;
                        }
                    }
                    _ => {
                        self.advance();
                    }
                }
            }
        }
        Ok(())
    }

    /// Parse packed dimensions tambahan `[msb:lsb][msb:lsb]...` setelah range
    /// pertama dan kumpulkan sebagai `Vec<ExprRange>` — dipakai di `parse_typedef`
    /// agar width typedef multidimensi (`[4:0][4:0][W-1:0]`) bisa dihitung penuh.
    pub(crate) fn parse_extra_packed_dims(&mut self) -> Result<Vec<ExprRange>, SimError> {
        let mut dims = Vec::new();
        while self.peek() == &Token::LBrack {
            if self.peek_is_packed_dim() {
                if let Some(r) = self.parse_range()? {
                    dims.push(r);
                }
            } else {
                // Unpacked single dimension `[N]` (struct member / typedef):
                // `bit [3:0] [31:0] plain_text[4]` — konsumsi dan buang.
                self.advance(); // '['
                let _ = self.parse_expr(0)?;
                self.expect(Token::RBrack)?;
            }
        }
        Ok(dims)
    }

    /// True jika token saat ini adalah packed dimension `[msb:lsb]` (bukan
    /// unpacked `[N]`, dynamic `[$]`, atau associative `[int]`).
    pub(crate) fn peek_is_packed_dim(&self) -> bool {
        if self.peek() != &Token::LBrack {
            return false;
        }
        let mut depth = 0usize;
        let mut i = 0usize;
        loop {
            match self.peek_ahead(i) {
                Token::LBrack => depth += 1,
                Token::RBrack => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return false;
                    }
                }
                Token::Colon if depth == 1 => return true,
                Token::Eof => return false,
                _ => {}
            }
            i += 1;
        }
    }

    /// True jika token saat ini adalah `[` yang berisi colon `:` pada
    /// kedalaman 1 (di luar kurung / bracket bersarang) — menandakan unpacked
    /// RANGE `[msb:lsb]` (mis. `[Width-1:0]`, `[Last:0]`, `[3:0]`), bukan
    /// size-expr `[N]`/`[Width]`, dynamic `[$]`, atau associative `[int]`.
    /// Scan token maju tanpa konsumsi (teknik sama dengan `peek_is_packed_dim`).
    ///
    /// Dipakai untuk memutuskan range-vs-size saat bound memakai ident
    /// (parameter / enum member) sehingga colon-nya tidak terlihat di posisi
    /// 1/2 — mis. `p1::lc_tx_t [PmEnLastPos-1:0] pinmux_hw_debug_en;` di rv_dm.
    pub(crate) fn peek_bracket_has_range_colon(&self) -> bool {
        if self.peek() != &Token::LBrack {
            return false;
        }
        let mut depth = 0usize;
        let mut i = 0usize;
        loop {
            match self.peek_ahead(i) {
                Token::LBrack | Token::LParen | Token::LBrace => depth += 1,
                Token::RBrack | Token::RParen | Token::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && i > 0 {
                        return false;
                    }
                }
                Token::Colon if depth == 1 => return true,
                Token::Eof => return false,
                _ => {}
            }
            i += 1;
        }
    }

    /// Deteksi pola tipe user-defined dgn packed range diikuti nama port:
    /// `foo_t [7:0] name` (peek saat ini = `foo_t`, peek_ahead(1) = `[`).
    /// Bracket berisi colon (range `[7:0]`) dan SETELAH `]` ada ident
    /// (nama port). Ini membedakan dari unpacked dim (`mat_a [8]` — tidak
    /// ada colon, setelah `]` adalah koma/`)`) sehingga ident pertama tidak
    /// salah dimakan sebagai nama port oleh inner loop.
    pub(crate) fn peek_packed_range_followed_by_ident(&self) -> bool {
        let mut depth = 0usize;
        let mut i = 1usize;
        let mut saw_colon = false;
        loop {
            match self.peek_ahead(i) {
                Token::LBrack | Token::LParen | Token::LBrace => depth += 1,
                Token::RBrack | Token::RParen | Token::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return saw_colon && matches!(self.peek_ahead(i + 1), Token::Ident(_));
                    }
                }
                Token::Colon if depth == 1 => saw_colon = true,
                Token::Eof => return false,
                _ => {}
            }
            i += 1;
        }
    }
}
