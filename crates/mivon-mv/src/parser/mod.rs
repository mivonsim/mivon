//! Mivon HDL (.mv) — Parser.
//! Recursive descent: tokens → AST (src/mv/ast.rs).
//! 1 file = 1 tanggung jawab: hanya parsing, tanpa codegen.
//!
//! Struktur (berkembang — mivon-mv makin besar):
//! - `mod.rs`   — entry `parse()`, struct `Parser` + token helpers, level file
//! - `expr.rs`  — ekspresi (Pratt) + tipe (parse_type/base)
//! - `stmt.rs`  — statement (all: control flow, assign, case, fork, assert, @sv)
//! - `module.rs`— module/program/param/port/seq/inst/interface/modport/generate
//! - `class.rs` — class/func/task/arglist/constraint/dist
//! - `defs.rs`  — typedef (alias/struct/union/enum) + package

use crate::ast::*;
use crate::lexer::{tokenize, Tok};
use crate::MvError;

pub mod class;
pub mod defs;
pub mod expr;
pub mod module;
pub mod stmt;

#[cfg(test)]
mod tests;

/// Parse source `.mv` → `MvFile`.
pub fn parse(src: &str) -> Result<MvFile, MvError> {
    let toks = tokenize(src)?;
    // Precompute offset byte awal tiap baris — dipakai raw-slice
    // `assert property (...)` yang harus dipertahankan tekstual (operator
    // SVA seperti `|->`/`##` bukan token `.mv`).
    let mut line_starts = vec![0usize];
    for (i, ch) in src.char_indices() {
        if ch == '\n' {
            line_starts.push(i + 1);
        }
    }
    let mut p = Parser {
        toks,
        pos: 0,
        src: src.to_string(),
        line_starts,
    };
    p.parse_file()
}

/// Parser recursive-descent `.mv`. Fields `pub(crate)` agar blok `impl`
/// di sub-modul (`expr`/`stmt`/`module`/`class`/`defs`) bisa mengakses.
pub(crate) struct Parser {
    pub(crate) toks: Vec<(Tok, usize, usize)>,
    pub(crate) pos: usize,
    /// Sumber asli (untuk raw-slice `assert property`).
    pub(crate) src: String,
    /// Offset byte awal tiap baris (indeks = line - 1).
    pub(crate) line_starts: Vec<usize>,
}

impl Parser {
    // ── Token helpers ──
    pub(crate) fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].0
    }
    pub(crate) fn peek_at(&self, n: usize) -> &Tok {
        let idx = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[idx].0
    }
    pub(crate) fn pos_line(&self) -> (usize, usize) {
        let (_, l, c) = self.toks[self.pos.min(self.toks.len() - 1)];
        (l, c)
    }
    pub(crate) fn advance(&mut self) {
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
    }
    pub(crate) fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.advance();
            true
        } else {
            false
        }
    }
    pub(crate) fn expect(&mut self, t: &Tok) -> Result<(), MvError> {
        if self.peek() == t {
            self.advance();
            Ok(())
        } else {
            let (l, c) = self.pos_line();
            Err(MvError::new(
                l,
                c,
                format!("diharapkan {:?}, ditemukan {:?}", t, self.peek()),
            ))
        }
    }
    pub(crate) fn expect_ident(&mut self) -> Result<String, MvError> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.advance();
                Ok(s)
            }
            _ => {
                let (l, c) = self.pos_line();
                Err(MvError::new(
                    l,
                    c,
                    format!("diharapkan identifier, ditemukan {:?}", self.peek()),
                ))
            }
        }
    }
    pub(crate) fn is_ident(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Ident(x) if x == s)
    }

    /// Lookahead F33: apakah `Ident [ ... ]` (boleh multi-dimensi, termasuk
    /// `[$]`) diikuti `'(` → cast ber-dimensi (`logic[7:0]'(x)` /
    /// `logic[7:0][$]'(x)` / `Word16[N]'(x)`). Digunakan supaya cast target
    /// boleh punya range / dimensi unpacked / queue (full support LRM §6.24).
    ///
    /// `self.pos` menunjuk ke `Ident`. Mengembalikan `true` bila token tepat
    /// setelah kurung siku penutup TERLUAR (grup terakhir) adalah
    /// `Tok::Quote`. Grup bracket berantai (`T[N][M]`) ditelusuri sampai
    /// grup terakhir.
    pub(crate) fn ident_followed_by_cast(&self) -> bool {
        if self.peek_at(1) != &Tok::LBrack {
            return false;
        }
        let mut i = self.pos + 1; // posisi '[' pembuka grup pertama
        loop {
            if self.toks.get(i).map(|t| t.0 != Tok::LBrack).unwrap_or(true) {
                return false;
            }
            // telusuri sampai ']' penutup grup ini (tangani nested).
            let mut depth: i32 = 0;
            while i < self.toks.len() {
                match &self.toks[i].0 {
                    Tok::LBrack => depth += 1,
                    Tok::RBrack => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            // sesudah ']' terluar grup ini:
            match self.toks.get(i + 1).map(|t| &t.0) {
                Some(Tok::LBrack) => {
                    i += 1; // grup dimensi lanjutan → lanjut telusuri
                    continue;
                }
                Some(Tok::Quote) => return true,
                _ => return false,
            }
        }
    }
    pub(crate) fn expect_ident_kw(&mut self, s: &str) -> Result<(), MvError> {
        if self.is_ident(s) {
            self.advance();
            Ok(())
        } else {
            let (l, c) = self.pos_line();
            Err(MvError::new(l, c, format!("diharapkan '{}'", s)))
        }
    }

    // ── Raw slice (untuk `assert property`) ──
    pub(crate) fn byte_offset(&self, line: usize, col: usize) -> usize {
        let ls = *self.line_starts.get(line.wrapping_sub(1)).unwrap_or(&0);
        ls + col.saturating_sub(1)
    }

    /// Potong teks asli dari (sl, sc) ke (el, ec) — 1-based line/col, end eksklusif.
    ///
    /// Catatan (edge case): `line_starts` memakai offset BYTE (`char_indices`),
    /// sedangkan `col` lexer dihitung per CHAR — untuk baris yang memuat
    /// karakter multi-byte (non-ASCII) SEBELUM titik potong, offset bisa
    /// meleset. Body `assert property` praktis selalu ASCII (operator SVA),
    /// jadi ini diterima; jangan dipakai untuk slicing sumber bebas unicode.
    pub(crate) fn raw_slice(&self, sl: usize, sc: usize, el: usize, ec: usize) -> String {
        let s = self.byte_offset(sl, sc);
        let e = self.byte_offset(el, ec).min(self.src.len());
        if s >= e {
            String::new()
        } else {
            self.src[s..e].to_string()
        }
    }

    // ── Step opsional utk `for` (generate & behavioral) ──
    /// Step opsional `for`: `for i in 0..N step 2 { ... }` — `step` di-lex
    /// sebagai `Ident("step")` (bukan keyword), diikuti ekspresi nilai.
    pub(crate) fn parse_optional_step(&mut self) -> Result<Option<Expr>, MvError> {
        if self.is_ident("step") {
            self.advance();
            Ok(Some(self.parse_expr()?))
        } else {
            Ok(None)
        }
    }

    // ── File ──
    pub(crate) fn parse_file(&mut self) -> Result<MvFile, MvError> {
        let mut f = MvFile::default();
        loop {
            match self.peek().clone() {
                Tok::Eof => break,
                Tok::Type => {
                    f.typedefs.push(self.parse_typedef_alias()?);
                }
                Tok::Packed | Tok::Struct | Tok::Enum | Tok::Union => {
                    f.typedefs.push(self.parse_typedef()?);
                }
                Tok::Package => {
                    f.packages.push(self.parse_package()?);
                }
                Tok::Interface => {
                    f.interfaces.push(self.parse_interface()?);
                }
                Tok::Module => {
                    f.modules.push(self.parse_module()?);
                }
                Tok::Func => {
                    f.funcs.push(self.parse_func()?);
                }
                Tok::Task => {
                    f.tasks.push(self.parse_task()?);
                }
                Tok::Ident(s) if s == "program" => {
                    // `program` bukan keyword token — di-lex sebagai Ident.
                    f.programs.push(self.parse_program()?);
                }
                Tok::Ident(s) if s == "class" => {
                    // `class` bukan keyword token — di-lex sebagai Ident.
                    f.classes.push(self.parse_class()?);
                }
                Tok::Ident(_) => {
                    // `type NAME = ...` boleh juga tanpa keyword `type`? Tidak —
                    // alias wajib `type`. Ident di level file = error.
                    let (l, c) = self.pos_line();
                    return Err(MvError::new(
                        l,
                        c,
                        format!("konstruk tidak dikenal di level file: {:?}", self.peek()),
                    ));
                }
                _ => {
                    let (l, c) = self.pos_line();
                    return Err(MvError::new(
                        l,
                        c,
                        format!("konstruk tidak dikenal: {:?}", self.peek()),
                    ));
                }
            }
        }
        Ok(f)
    }
}
