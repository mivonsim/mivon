//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan parser.rs (SRP Refactoring).
//! Tanggung jawab: Parsing class declaration (class ... endclass).
//!
//! Fungsi:
//!   - parse_class() — parsing deklarasi class dengan type params, extends, members
//!
//! ──────────────────────────────────────────────────────────────────────────────

use super::Parser;
use crate::lexer::*;
use mivon_ast::*;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

impl Parser {
    /// Fast skip for first pass: collect class name + fast-skip body to endclass.
    /// Does NOT parse members — dramatically faster for class discovery pass.
    /// Parse isi blok `constraint name { ... }` (berhenti di `}` tanpa
    /// memakannya). Item: `solve a before b`, `if (c) {..} else {..}` (F12,
    /// constraint kondisional), atau ekspresi (termasuk `inside`/`dist`).
    /// Ekspresi yang gagal di-skip ke `;` (recovery — perilaku lama).
    pub(crate) fn parse_constraint_items(&mut self) -> Result<Vec<ConstraintItem>, SimError> {
        let mut body = Vec::new();
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            // `solve var before var, var` (di-lex: Ident("solve"), Ident("before"))
            if let Token::Ident(ref s) = self.peek() {
                if s == "solve" {
                    self.advance();
                    let mut vars = Vec::new();
                    let first_var = self.expect_ident()?;
                    vars.push(first_var);
                    if let Token::Ident(ref s2) = self.peek() {
                        if s2 == "before" {
                            self.advance();
                            loop {
                                let v = self.expect_ident()?;
                                vars.push(v);
                                if self.peek() == &Token::Comma {
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    self.skip_semi();
                    body.push(ConstraintItem::SolveBefore { vars });
                    continue;
                }
            }
            // `if (cond) { items } else if (cond) { items } ... else { items }`
            // (constraint kondisional F12 + else-if chain — pola
            // kmac_test_vectors_kmac_vseq: `if..else if..else if..`).
            if self.peek() == &Token::If {
                let citem = self.parse_constraint_if()?;
                body.push(citem);
                continue;
            }
            // `foreach (arr[i]) { constraints }` di dalam dengan-block /
            // constraint body (pola umum UVM randomize-with). ConstraintItem
            // TIDAK punya variant foreach — parse body (recursive) & BUANG
            // (parsing resilience; body tidak dipakai engine).
            if self.peek() == &Token::Foreach {
                self.advance();
                if self.peek() == &Token::LParen {
                    // Foreach header `(arr[i])` — balanced.
                    let mut depth = 0i32;
                    loop {
                        match self.peek() {
                            Token::Eof => break,
                            Token::LParen => {
                                depth += 1;
                                self.advance();
                            }
                            Token::RParen => {
                                depth -= 1;
                                self.advance();
                                if depth <= 0 {
                                    break;
                                }
                            }
                            _ => self.advance(),
                        }
                    }
                }
                if self.peek() == &Token::LBrace {
                    self.advance();
                    let _ = self.parse_constraint_items();
                    if self.peek() == &Token::RBrace {
                        self.advance();
                    }
                } else {
                    let _ = self.skip_until_semi_or_end();
                }
                continue;
            }
            // `unique {expr; ...}` is a constraint qualifier used by
            // riscv-dv randomize-with blocks.  The contents are not needed
            // by Mivon's runtime, but must be consumed without treating the
            // semicolons as malformed expression syntax.
            if self.peek() == &Token::Unique {
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
                } else {
                    self.skip_semi();
                }
                continue;
            }
            // `soft expr;` (LANG-31) — constraint soft (best-effort): boleh
            // dilanggar bila bertentangan dengan hard constraint.
            if self.peek() == &Token::Soft {
                self.advance();
                match self.parse_expr(0) {
                    Ok(expr) => {
                        self.skip_semi();
                        body.push(ConstraintItem::Soft(expr));
                    }
                    Err(_) => {
                        // Recovery: skip ke ';' atau '}'
                        loop {
                            match self.peek() {
                                Token::Semi => {
                                    self.advance();
                                    break;
                                }
                                Token::RBrace | Token::Eof => break,
                                _ => {
                                    self.advance();
                                }
                            }
                        }
                    }
                }
                continue;
            }
            // Ekspresi constraint (relasional/equality/inside/dist)
            // parse_expr might fail on complex constraint expressions;
            // if so, skip to ';' to recover
            match self.parse_expr(0) {
                Ok(expr) => {
                    self.skip_semi();
                    body.push(ConstraintItem::Expr(expr));
                }
                Err(_) => {
                    // Error in constraint expression — skip to ';' or '}'
                    loop {
                        match self.peek() {
                            Token::Semi => {
                                self.advance();
                                break;
                            }
                            Token::RBrace | Token::Eof => break,
                            _ => {
                                self.advance();
                            }
                        }
                    }
                }
            }
        }
        Ok(body)
    }

    /// Parse satu konstruk `if (cond) { then } [else if (c2) { } | else { }]`
    /// dalam constraint body — recursive utk chain `else if` (pola
    /// kmac_test_vectors_*: lima level if/else if di randomize-with).
    fn parse_constraint_if(&mut self) -> Result<ConstraintItem, SimError> {
        self.advance(); // consume 'if'
        self.expect(Token::LParen)?;
        let cond = self.parse_expr(0)?;
        self.expect(Token::RParen)?;
        let mut then = Vec::new();
        let mut els = Vec::new();
        if self.peek() == &Token::LBrace {
            self.advance();
            then = self.parse_constraint_items()?;
            self.expect(Token::RBrace)?;
        } else {
            // Tanpa braces: parse satu ekspresi constraint sebagai branch-then.
            match self.parse_expr(0) {
                Ok(e) => {
                    self.skip_semi();
                    then.push(ConstraintItem::Expr(e));
                }
                Err(_) => loop {
                    match self.peek() {
                        Token::Semi => {
                            self.advance();
                            break;
                        }
                        Token::RBrace | Token::Eof => break,
                        _ => {
                            self.advance();
                        }
                    }
                },
            }
        }
        if self.peek() == &Token::Else {
            self.advance();
            if self.peek() == &Token::If {
                // `else if (cond2) { ... }` — chain recursive.
                els.push(self.parse_constraint_if()?);
            } else if self.peek() == &Token::LBrace {
                self.advance();
                els = self.parse_constraint_items()?;
                self.expect(Token::RBrace)?;
            } else {
                match self.parse_expr(0) {
                    Ok(e) => {
                        self.skip_semi();
                        els.push(ConstraintItem::Expr(e));
                    }
                    Err(_) => loop {
                        match self.peek() {
                            Token::Semi => {
                                self.advance();
                                break;
                            }
                            Token::RBrace | Token::Eof => break,
                            _ => {
                                self.advance();
                            }
                        }
                    },
                }
            }
        }
        Ok(ConstraintItem::If { cond, then, els })
    }

    pub(crate) fn parse_class_fast(&mut self) -> Result<(), SimError> {
        self.advance(); // consume 'class'
                        // Skip optional #(type T = ...) parameter list
        if self.peek() == &Token::Hash {
            self.advance();
            if self.peek() == &Token::LParen {
                let _ = self.skip_balanced_paren_light();
            }
        }
        // Collect class name
        if let Token::Ident(name) = self.peek() {
            self.class_names.insert(*name);
            self.advance();
        }
        // Skip extends clause
        if self.peek() == &Token::Extends {
            self.advance();
            if matches!(self.peek(), Token::Ident(_)) {
                self.advance(); // base class name
            }
            // Skip optional #(.PARAM(...)) after base class
            if self.peek() == &Token::Hash {
                self.advance();
                if self.peek() == &Token::LParen {
                    let _ = self.skip_balanced_paren_light();
                }
            }
        }
        // Skip implements clause (SV-2005)
        if matches!(self.peek(), Token::Ident(s) if s.as_str() == "implements") {
            self.advance();
            loop {
                if matches!(self.peek(), Token::Ident(_)) {
                    self.advance();
                }
                if self.peek() == &Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.skip_semi();
        // Fast-skip class body until endclass
        loop {
            match self.peek() {
                Token::EndClass | Token::Eof => {
                    if self.peek() == &Token::EndClass {
                        self.advance();
                    }
                    // Skip optional ': name' after endclass
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    break;
                }
                Token::Class => {
                    // Nested class — skip body
                    self.skip_class_body();
                }
                _ => {
                    if self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
                        self.skip_attribute();
                    } else {
                        self.advance();
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn parse_class(&mut self) -> Result<ClassDecl, SimError> {
        self.advance(); // consume 'class'
                        // Bentuk non-standar: `class #(type T = int) Name;` (param list SEBELUM nama).
        let mut type_params = Vec::new();
        if self.peek() == &Token::Hash {
            type_params.extend(self.parse_class_param_list()?);
        }
        let name = self.expect_ident()?;
        // Bentuk LRM (IEEE 1800-2017 §8.3): `class Name #(param_list) extends ...;`
        // (param list SETELAH nama class).
        if self.peek() == &Token::Hash {
            type_params.extend(self.parse_class_param_list()?);
        }
        let extends = if self.peek() == &Token::Extends {
            self.advance();
            let base_name = self.expect_ident()?;
            // Base class package-qualified: `extends pkg::cls;`
            let base_name = if self.peek() == &Token::Scope {
                self.advance();
                let inner = self.expect_ident()?;
                Symbol::intern(&format!("{}::{}", base_name, inner))
            } else {
                base_name
            };
            // Handle parameterized base class: extends Base #(.PARAM(value), ...)
            if self.peek() == &Token::Hash {
                self.advance();
                if self.peek() == &Token::LParen {
                    self.skip_balanced_paren()?;
                }
            }
            Some(base_name)
        } else {
            None
        };
        self.expect(Token::Semi)?;
        self.type_param_names = type_params.iter().map(|tp| tp.name).collect();
        let mut members = Vec::new();
        loop {
            match self.peek() {
                Token::EndClass => {
                    self.advance();
                    // Handle optional 'endclass : name'
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    break;
                }
                Token::Function => match self.parse_function(false) {
                    Ok(f) => members.push(ClassMember::Function(f)),
                    Err(_) => {
                        let _ = self.skip_until_semi_or_end();
                    }
                },
                Token::Ident(s) if s == "extern" => {
                    // Extern prototype — consume until semicolon
                    self.advance(); // extern
                                    // Handle 'extern local', 'extern protected', 'extern virtual', etc.
                    loop {
                        match self.peek() {
                            Token::Ident(n)
                                if n == "local" || n == "protected" || n == "virtual" =>
                            {
                                self.advance();
                            }
                            _ => break,
                        }
                    }
                    // Handle extern constraint: `extern constraint name;`
                    if self.peek() == &Token::Constraint {
                        self.advance(); // consume constraint
                        let _ = self.expect_ident(); // consume name
                        self.skip_semi();
                        continue;
                    }
                    if !matches!(self.peek(), Token::Function | Token::Task) {
                        continue;
                    }
                    self.advance(); // consume function/task
                    let mut depth = 0i32;
                    loop {
                        match self.peek() {
                            Token::Semi if depth <= 0 => {
                                self.advance();
                                break;
                            }
                            Token::LParen => {
                                depth += 1;
                                self.advance();
                            }
                            Token::RParen => {
                                depth -= 1;
                                self.advance();
                            }
                            Token::EndClass | Token::Eof => break,
                            _ => {
                                self.advance();
                            }
                        }
                    }
                }
                Token::Virtual => {
                    self.advance();
                    match self.peek() {
                        Token::Function => match self.parse_function(true) {
                            Ok(f) => members.push(ClassMember::Function(f)),
                            Err(_) => {
                                let _ = self.skip_until_semi_or_end();
                            }
                        },
                        Token::Task => match self.parse_task(true) {
                            Ok(t) => members.push(ClassMember::Task(t)),
                            Err(_) => {
                                let _ = self.skip_until_semi_or_end();
                            }
                        },
                        _ => match self.parse_decl() {
                            Ok(mut decl) => {
                                for n in &mut decl.names {
                                    n.is_rand = false;
                                }
                                members.push(ClassMember::Decl(decl));
                            }
                            Err(_) => {
                                let _ = self.skip_until_semi_or_end();
                            }
                        },
                    }
                }
                Token::Task => match self.parse_task(false) {
                    Ok(t) => members.push(ClassMember::Task(t)),
                    Err(_) => {
                        let _ = self.skip_until_semi_or_end();
                    }
                },
                Token::Input
                | Token::Output
                | Token::Inout
                | Token::Reg
                | Token::Logic
                | Token::Wire
                | Token::Int
                | Token::Integer
                | Token::Signed
                | Token::Bit
                | Token::Byte
                | Token::Shortint
                | Token::Longint
                | Token::Time
                | Token::String
                | Token::Mailbox
                | Token::Semaphore
                | Token::Real
                | Token::RealTime
                | Token::Enum
                | Token::Struct
                | Token::Union
                | Token::Wand
                | Token::Wor
                | Token::Tri
                | Token::Tri0
                | Token::Tri1
                | Token::TriAnd
                | Token::TriOr
                | Token::Supply0
                | Token::Supply1 => {
                    // Error deklarasi field TIDAK lagi ditelan diam-diam:
                    // swallowing membuat field yang gagal di-parse hilang
                    // dari design tanpa diagnostic (silent miscompilation —
                    // contoh `logic [7:0] [4] fa;` yang dimensi unpacked-nya
                    // salah tempat, LRM 1800 §7.3).
                    let mut decl = self.parse_decl()?;
                    for n in &mut decl.names {
                        n.is_rand = false;
                    }
                    members.push(ClassMember::Decl(decl));
                }
                Token::Rand | Token::RandC => {
                    self.advance();
                    let mut decl = self.parse_decl()?;
                    for n in &mut decl.names {
                        n.is_rand = true;
                    }
                    members.push(ClassMember::Decl(decl));
                }
                Token::Ident(name) if self.type_param_names.contains(name) => {
                    let tp_name = *name;
                    self.advance();
                    let decl_expr_range = if self.peek() == &Token::LBrack {
                        self.parse_range()?
                    } else {
                        None
                    };
                    let mut extra_packed: Vec<(ExprRange, Option<Range>)> = Vec::new();
                    while self.peek() == &Token::LBrack && self.peek_ahead(1) == &Token::Colon {
                        if let Some(er) = self.parse_range()? {
                            extra_packed.push((er, None));
                        }
                    }
                    let names = self.parse_decl_names(decl_expr_range, extra_packed)?;
                    self.skip_semi();
                    members.push(ClassMember::Decl(mivon_ast::types::Decl {
                        dtype: DataType::UserDefined(tp_name),
                        kind: mivon_ast::types::DeclKind::Logic,
                        names,
                    }));
                }
                Token::Ident(s) if s == "pure" && self.peek_ahead(1) == &Token::Virtual => {
                    self.advance(); // pure
                    loop {
                        match self.peek() {
                            Token::Semi => {
                                self.advance();
                                break;
                            }
                            Token::EndClass | Token::Eof => break,
                            _ => {
                                self.advance();
                            }
                        }
                    }
                }
                Token::Ident(name) if self.peek_ahead(1) == &Token::LParen => {
                    // Macro/fungsi CALL di body class yang TIDAK diikuti `;`
                    // (hasil expand macro undefined seperti
                    // `` `uvm_object_param_utils(rom_ctrl_prim_mem_rom_mem#(MemDepth)) ``).
                    // `name(...)` di level class member TIDAK pernah deklarasi/
                    // instance valid — itu call (UVM factory, registrasi, dll).
                    // Konsumsi call balanced sampai `)` lalu `;` (jika ada);
                    // jangan sampai skip melewati `endclass` karena call tidak
                    // punya `;` (via skip_until_semi_or_end → bahaya EOF).
                    if self.class_names.contains(name) || self.typedef_names.contains(name) {
                        // Tipe user-defined → bisa jadi deklarasi class field
                        // `Type obj;` — parse_decl menanganinya.
                        match self.parse_decl() {
                            Ok(mut decl) => {
                                for n in &mut decl.names {
                                    n.is_rand = false;
                                }
                                members.push(ClassMember::Decl(decl));
                            }
                            Err(_) => {
                                let _ = self.skip_until_semi_or_end();
                            }
                        }
                        continue;
                    }
                    self.advance(); // nama macro-call
                    self.skip_balanced_call();
                    self.skip_semi();
                    continue;
                }
                Token::Ident(_) => {
                    // F18: field class bertipe user-defined (`my_env env;`).
                    // Sebelumnya hanya type-param Ident yang diparse sebagai
                    // field — tipe user-defined lain di-skip diam-diam oleh
                    // fallback `_ => advance()`, sehingga class fields kosong
                    // dan `env = new("env", this)` di build_phase tidak bisa
                    // resolve class (lihat resolve_new_class_hint).
                    match self.parse_decl() {
                        Ok(mut decl) => {
                            for n in &mut decl.names {
                                n.is_rand = false;
                            }
                            members.push(ClassMember::Decl(decl));
                        }
                        Err(_) => {
                            let _ = self.skip_until_semi_or_end();
                        }
                    }
                }
                Token::Constraint => {
                    self.advance();
                    let cname = self.expect_ident()?;
                    // Deklarasi constraint forward: `constraint name;` — body
                    // didefinisikan di luar class (`constraint C::name { ... }`).
                    // Biasanya ditulis `extern constraint name;` tapi UVM/DV juga
                    // memakai `constraint name;` tanpa extern.
                    if self.peek() == &Token::Semi {
                        self.advance();
                        members.push(ClassMember::Constraint {
                            name: cname,
                            body: Vec::new(),
                            is_static: false,
                        });
                    } else {
                        self.expect(Token::LBrace)?;
                        let body = self.parse_constraint_items()?;
                        self.expect(Token::RBrace)?;
                        members.push(ClassMember::Constraint {
                            name: cname,
                            body,
                            is_static: false,
                        });
                    }
                }
                Token::Let => {
                    // LANG-40: `let` di dalam class.
                    match self.parse_let_decl() {
                        Ok(ld) => members.push(ClassMember::Let(ld)),
                        Err(_) => {
                            let _ = self.skip_until_semi_or_end();
                        }
                    }
                }
                Token::Static => {
                    // LANG-32: `static constraint name { ... }` — block constraint
                    // dibagi antar semua instance class (IEEE 1800-2017 §18.5.10).
                    self.advance();
                    if self.peek() == &Token::Constraint {
                        self.advance();
                        let cname = self.expect_ident()?;
                        self.expect(Token::LBrace)?;
                        let body = self.parse_constraint_items()?;
                        self.expect(Token::RBrace)?;
                        members.push(ClassMember::Constraint {
                            name: cname,
                            body,
                            is_static: true,
                        });
                    }
                    // Bukan static constraint (static var/function/task) — token
                    // Static sudah dikonsumsi; member diparse di iterasi berikutnya.
                }
                Token::Class => {
                    // Nested class — skip entire body to matching endclass
                    self.skip_class_body();
                }
                _ => {
                    if self.peek() == &Token::Eof {
                        // Hit EOF without finding endclass — abort to prevent infinite loop
                        break;
                    }
                    self.advance();
                }
            }
        }
        self.type_param_names.clear();
        Ok(ClassDecl {
            name,
            extends,
            type_params,
            members,
        })
    }

    /// Parse parameter port list class: `#(type T = int, parameter int AW = 7)`.
    /// Mendukung item `type NAME [= type]` (LRM) maupun `parameter [type] NAME
    /// [= value]` (dipakai mis. pulp_riscv_dbg `class req_t #(parameter int...)`).
    pub(crate) fn parse_class_param_list(&mut self) -> Result<Vec<TypeParam>, SimError> {
        self.advance(); // '#'
        self.expect(Token::LParen)?;
        let mut type_params = Vec::new();
        loop {
            if matches!(self.peek(), Token::RParen | Token::Eof) {
                break;
            }
            let is_param = matches!(self.peek(), Token::Param | Token::Parameter);
            // `type NAME [= type_expr]` — port tipe.
            if self.peek() == &Token::Type || (is_param && self.peek_ahead(1) == &Token::Type) {
                if is_param {
                    self.advance(); // parameter
                }
                self.advance(); // type
                let tp_name = self.expect_ident()?;
                let default_type = if self.peek() == &Token::BlockingAssign {
                    self.advance();
                    Some(self.parse_type_expr()?)
                } else {
                    None
                };
                type_params.push(TypeParam {
                    name: tp_name,
                    default_type,
                });
            } else {
                // `parameter [type] NAME [= value]` atau `int NAME = value`.
                if is_param {
                    self.advance(); // parameter
                }
                // Skip tipe (keyword atau user-defined / pkg::type).
                while matches!(
                    self.peek(),
                    Token::Int
                        | Token::Integer
                        | Token::Logic
                        | Token::Bit
                        | Token::Byte
                        | Token::Shortint
                        | Token::Longint
                        | Token::Time
                        | Token::Real
                        | Token::RealTime
                        | Token::Reg
                        | Token::String
                        | Token::Signed
                        | Token::Unsigned
                ) {
                    self.advance();
                }
                if matches!(self.peek(), Token::Ident(_))
                    && matches!(
                        self.peek_ahead(1),
                        Token::Ident(_) | Token::Scope | Token::LBrack
                    )
                {
                    // User-defined type — konsumsi ident lalu optional `::`/range.
                    self.advance(); // type name
                    while self.peek() == &Token::Scope {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    while self.peek() == &Token::LBrack {
                        let _ = self.skip_balanced_paren();
                    }
                }
                let tp_name = match self.peek() {
                    Token::Ident(n) => {
                        let name = *n;
                        self.advance();
                        name
                    }
                    _ => break,
                };
                let default_type = if self.peek() == &Token::BlockingAssign {
                    self.advance(); // '='
                    let saved = self.pos.get();
                    match self.parse_expr(0) {
                        Ok(_) => None,
                        Err(_) => {
                            // Nilai kompleks ({..}, '0, dsb.) — lewati seimbang.
                            self.pos.set(saved);
                            let mut depth = 0i32;
                            loop {
                                match self.peek() {
                                    Token::Comma if depth <= 0 => break,
                                    Token::RParen if depth <= 0 => break,
                                    Token::LParen | Token::LBrace | Token::LBrack => {
                                        depth += 1;
                                        self.advance();
                                    }
                                    Token::RParen | Token::RBrace | Token::RBrack => {
                                        depth -= 1;
                                        self.advance();
                                    }
                                    Token::Eof => break,
                                    _ => {
                                        self.advance();
                                    }
                                }
                            }
                            None
                        }
                    }
                } else {
                    None
                };
                type_params.push(TypeParam {
                    name: tp_name,
                    default_type,
                });
            }
            if self.peek() == &Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(Token::RParen)?;
        Ok(type_params)
    }
}
