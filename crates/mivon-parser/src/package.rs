//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan parser.rs (SRP Refactoring).
//! Tanggung jawab: Parsing package declaration (package ... endpackage).
//!
//! Fungsi:
//!   - parse_package_decl() — parsing deklarasi package
//!
//! ──────────────────────────────────────────────────────────────────────────────

use super::Parser;
use crate::lexer::*;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

impl Parser {
    pub(crate) fn parse_package_decl(&mut self) -> Result<PackageDecl, SimError> {
        self.advance(); // consume 'package'
        let name = self.expect_ident()?;
        self.skip_semi();
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Token::EndPackage => {
                    self.advance();
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    break;
                }
                Token::Eof => {
                    return Err(
                        self.err_with_code(DiagCode::ExpectedToken, "unexpected EOF in package")
                    )
                }
                _ => {
                    // Macro non-expand di package body: bare `Ident(`
                    // (mis. `ASSERT_STATIC_IN_PACKAGE(...)` prim_assert).
                    // Ident( di package-level TIDAK pernah valid — skip.
                    if matches!(self.peek(), Token::Ident(_))
                        && self.peek_ahead(1) == &Token::LParen
                    {
                        self.advance();
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
                        self.skip_semi();
                        continue;
                    }
                    match self.peek() {
                        Token::Param | Token::Parameter | Token::LocalParam => {
                            let is_localparam = self.peek() == &Token::LocalParam;
                            self.advance();

                            // Handle 'parameter type X = type_expr'
                            if self.peek() == &Token::Type {
                                self.advance();
                                let pname = self.expect_ident()?;
                                let type_default = if self.peek() == &Token::BlockingAssign {
                                    self.advance();
                                    Some(self.parse_type_expr()?)
                                } else {
                                    None
                                };
                                self.skip_semi();
                                items.push(PackageItem::Param(ParamDecl {
                                    name: pname,
                                    dtype: None,
                                    range: None,
                                    default: None,
                                    is_localparam,
                                    is_type_param: true,
                                    type_default,
                                }));
                                continue;
                            }

                            // Parse optional built-in type keyword
                            let mut dtype = None;
                            match self.peek() {
                                Token::Integer => {
                                    self.advance();
                                    dtype = Some(DataType::Integer);
                                }
                                Token::Int => {
                                    self.advance();
                                    dtype = Some(DataType::Int);
                                }
                                Token::Reg => {
                                    self.advance();
                                    dtype = Some(DataType::Logic);
                                }
                                Token::Logic => {
                                    self.advance();
                                    dtype = Some(DataType::Logic);
                                }
                                Token::Bit => {
                                    self.advance();
                                    dtype = Some(DataType::Bit);
                                }
                                Token::Byte => {
                                    self.advance();
                                    dtype = Some(DataType::Byte);
                                }
                                Token::Shortint => {
                                    self.advance();
                                    dtype = Some(DataType::Shortint);
                                }
                                Token::Longint => {
                                    self.advance();
                                    dtype = Some(DataType::Longint);
                                }
                                Token::Time => {
                                    self.advance();
                                    dtype = Some(DataType::Time);
                                }
                                _ => {}
                            }

                            // Handle signed/unsigned
                            if self.peek() == &Token::Signed {
                                self.advance();
                                let inner = dtype.take().unwrap_or(DataType::Int);
                                dtype = Some(DataType::Signed(Box::new(inner)));
                            }
                            if self.peek() == &Token::Unsigned {
                                self.advance();
                            }

                            // Handle user-defined type (ident followed by ident or [)
                            let mut type_ident = None;
                            if dtype.is_none() {
                                if let Token::Ident(s) = self.peek() {
                                    let ahead = self.peek_ahead(1).clone();
                                    if matches!(
                                        ahead,
                                        Token::Ident(_)
                                            | Token::LBrack
                                            | Token::Scope
                                            | Token::Signed
                                            | Token::Unsigned
                                    ) {
                                        let s_owned = *s;
                                        self.advance();
                                        type_ident = Some(s_owned);
                                        // Scoped type: pkg::type
                                        if self.peek() == &Token::Scope {
                                            self.advance();
                                            let type_name = self.expect_ident()?;
                                            type_ident = Some(Symbol::intern(&format!(
                                                "{}::{}",
                                                s_owned, type_name
                                            )));
                                        }
                                    }
                                }
                            }

                            // Parse optional range [msb:lsb]
                            let mut range = None;
                            if self.peek() == &Token::LBrack {
                                self.advance();
                                let msb = self.parse_expr(0)?;
                                self.expect(Token::Colon)?;
                                let lsb = self.parse_expr(0)?;
                                self.expect(Token::RBrack)?;
                                range = Some((msb, lsb));
                                // Skip additional packed dimensions: [a:b][c:d]
                                while self.peek() == &Token::LBrack {
                                    self.advance();
                                    self.parse_expr(0)?;
                                    self.expect(Token::Colon)?;
                                    self.parse_expr(0)?;
                                    self.expect(Token::RBrack)?;
                                }
                            }

                            // Parse parameter name(s)
                            loop {
                                let pk = self.peek().clone();
                                let pname = match &pk {
                                    Token::Ident(s) => {
                                        self.advance();
                                        *s
                                    }
                                    _ => break,
                                };
                                // Skip unpacked array dimension(s) after name:
                                // name [N], name [msb:lsb], name [] (dynamic),
                                // name [$] (queue). Multi-dimensi diperbolehkan.
                                while self.peek() == &Token::LBrack {
                                    self.advance();
                                    if self.peek() == &Token::RBrack {
                                        // `[]` kosong — dynamic array
                                        self.advance();
                                    } else if self.peek() == &Token::Dollar {
                                        // `[$]` / `[$:N]` — queue
                                        self.advance();
                                        if self.peek() == &Token::Colon {
                                            self.advance();
                                            let _ = self.parse_expr(0);
                                        }
                                        self.expect(Token::RBrack)?;
                                    } else {
                                        self.parse_expr(0)?;
                                        if self.peek() == &Token::Colon {
                                            self.advance();
                                            self.parse_expr(0)?;
                                        }
                                        self.expect(Token::RBrack)?;
                                    }
                                }
                                let default = if self.peek() == &Token::BlockingAssign {
                                    self.advance();
                                    Some(self.parse_expr(0)?)
                                } else {
                                    None
                                };
                                let resolved_dtype = if let Some(t) = &type_ident {
                                    Some(DataType::UserDefined(*t))
                                } else {
                                    dtype.clone()
                                };
                                items.push(PackageItem::Param(ParamDecl {
                                    name: pname,
                                    dtype: resolved_dtype,
                                    range: range.clone(),
                                    default,
                                    is_localparam,
                                    is_type_param: false,
                                    type_default: None,
                                }));
                                if self.peek() == &Token::Comma {
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                            self.skip_semi();
                        }
                        Token::Function => {
                            items.push(PackageItem::Function(self.parse_function(false)?));
                        }
                        Token::Task => {
                            items.push(PackageItem::Task(self.parse_task(false)?));
                        }
                        Token::Covergroup => {
                            // Covergroup di package level (pola DV umum).
                            // Parsed untuk robustness lalu di-buang (PackageItem
                            // tidak punya varian covergroup; hanya register nama).
                            let cg = self.parse_covergroup()?;
                            self.class_names.insert(cg.name);
                        }
                        Token::Class => {
                            items.push(PackageItem::Class(self.parse_class()?));
                        }
                        Token::Virtual if self.peek_ahead(1) == &Token::Class => {
                            // `virtual class Name #(...) ...;` di dalam package
                            self.advance(); // consume 'virtual' agar parse_class
                                            // melihat token 'class'
                            items.push(PackageItem::Class(self.parse_class()?));
                        }
                        Token::Typedef => {
                            // Check for 'typedef class' (forward declaration)
                            if matches!(self.peek_ahead(1), Token::Class | Token::Virtual) {
                                self.advance(); // consume 'typedef'
                                while self.peek() != &Token::Semi && self.peek() != &Token::Eof {
                                    self.advance();
                                }
                                self.skip_semi();
                            } else {
                                let td = self.parse_typedef()?;
                                self.typedef_names.insert(td.name);
                                self.package_tdefs.entry(name).or_default().push(td.name);
                                items.push(PackageItem::Typedef(td));
                            }
                        }
                        Token::Import => {
                            self.advance();
                            // DPI-C import di package: `import "DPI-C" context
                            // function void name(...)` / `task`. Parser membaca
                            // deklarasi lalu mengabaikan isinya (simulator tidak
                            // menjalankan DPI body), simpan sebagai DpiImport.
                            if self.peek() == &Token::StringLit(Symbol::intern("DPI-C"))
                                || self.peek() == &Token::StringLit(Symbol::intern("DPI"))
                            {
                                let dpi = self.parse_dpi_import()?;
                                items.push(PackageItem::DpiImport(dpi));
                                continue;
                            }
                            let mut imported = Vec::new();
                            loop {
                                let pkg = self.expect_ident()?;
                                self.expect(Token::Scope)?;
                                let item = if self.peek() == &Token::Star {
                                    self.advance();
                                    Symbol::intern("*")
                                } else {
                                    self.expect_ident()?
                                };
                                // Register imported typedef names
                                if let Some(tdefs) = self.package_tdefs.get(&pkg) {
                                    if item == "*" {
                                        for name in tdefs {
                                            self.typedef_names.insert(*name);
                                        }
                                    } else if tdefs.contains(&item) {
                                        self.typedef_names.insert(item);
                                    }
                                }
                                imported.push((pkg, item));
                                if self.peek() == &Token::Comma {
                                    self.advance();
                                    continue;
                                }
                                break;
                            }
                            self.skip_semi();
                            for (pkg, item) in imported {
                                items.push(PackageItem::Import { package: pkg, item });
                            }
                        }
                        Token::Export => {
                            self.advance();
                            // export bisa beberapa item dipisah koma:
                            // `export pkg::a, pkg::b;` (LRM 1800 §26.4).
                            loop {
                                let pkg = self.expect_ident()?;
                                self.expect(Token::Scope)?;
                                let item = if self.peek() == &Token::Star {
                                    self.advance();
                                    Symbol::intern("*")
                                } else {
                                    self.expect_ident()?
                                };
                                items.push(PackageItem::Export { package: pkg, item });
                                if self.peek() == &Token::Comma {
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                            self.skip_semi();
                        }
                        Token::Constraint => {
                            // Eksternal constraint di dalam package (include file
                            // DV): `constraint C::name { ... }` — body sudah
                            // dikumpulkan saat class di-parse; hanya skip utk
                            // tidak desync (sama dgn arm top-level). Sebelumnya
                            // jatuh ke parse_decl → "expected wire/reg/...".
                            self.advance(); // 'constraint'
                            while matches!(
                                self.peek(),
                                Token::Ident(_) | Token::Scope | Token::Hash
                            ) {
                                self.advance();
                            }
                            match self.peek() {
                                Token::LBrace => {
                                    // Konsumsi `{` DULU — parse_constraint_items
                                    // mengharapkan peek di ITEM PERTAMA (loop
                                    // berterminasi di RBrace). Tanpa advance,
                                    // iterasi pertama melihat LBrace →
                                    // parse_expr({...) error → parser desync
                                    // (constraint external vseq DV).
                                    self.advance();
                                    let _ = self.parse_constraint_items();
                                    if self.peek() == &Token::RBrace {
                                        self.advance();
                                    }
                                }
                                _ => {
                                    let _ = self.skip_until_semi_or_end();
                                }
                            }
                        }
                        Token::Semi => {
                            // Stray `;` di package body — umum setelah makro yang
                            // me-expand `class ... endclass`/`function ... endfunction`
                            // (pola `DEFINE_*_INSTR(...);` riscv-dv): sisa `;` dari
                            // baris pemanggil makro tertinggal. Sebelumnya jatuh ke
                            // parse_decl → "expected wire/reg/..." error nyata padahal
                            // konstruk sudah ter-parse benar.
                            self.advance();
                        }
                        _ => {
                            let decl = self.parse_decl()?;
                            items.push(PackageItem::Decl(decl));
                        }
                    }
                }
            }
        }
        Ok(PackageDecl { name, items })
    }
}
