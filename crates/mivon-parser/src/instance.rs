// Parser submodule: module/interface/instance parsing
// Tanggung jawab: parse_module, parse_interface, parse_interface_fast, parse_program_fast,
// skip_balanced_paren, parse_modport, parse_port_list, parse_instance, parse_gate_primitive

use super::Parser;
use crate::lexer::*;
use crate::util::*;
use mivon_ast::types::const_eval_simple;
use mivon_ast::*;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

impl Parser {
    pub(crate) fn parse_module(&mut self) -> Result<Module, SimError> {
        // F84 (LRM §19.8): rekam `timescale yang berlaku untuk module ini —
        // directive terakhir pada/sebelum baris `module`. `advance()` di bawah
        // sudah konsumsi keyword, jadi baris diambil SEBELUM itu.
        self.timescale_at_line = self
            .timescale_segments
            .iter()
            .rev()
            .find(|(from, _)| *from <= self.peek_line())
            .map(|(_, ts)| ts.clone());
        self.advance(); // consume 'module', 'interface', or 'program'
        self.typedef_names.clear();
        // Re-seed typedef GLOBAL (lintas file) yang di-clear di atas — tanpa
        // ini nama typedef dari file lain hilang di scope module.
        self.typedef_names
            .extend(self.global_typedef_names.iter().copied());
        // Type params module tidak boleh bocor antar-module.
        self.module_type_params.clear();

        // Skip (* ... *) attributes before module name
        while self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
            self.skip_attribute();
        }

        let name_tok = self.peek().clone();
        let name = match &name_tok {
            Token::Ident(s) => {
                self.advance();
                *s
            }
            Token::Dollar => {
                // Nama module ala Yosys/synthesis: `module $_DLATCH_P_ (...)`
                // (vendor lowrisc_ibex latch_map.v). `$` + ident.
                self.advance();
                let ident = self.expect_ident()?;
                let mut s = String::from("$");
                s.push_str(ident.as_str());
                Symbol::intern(&s)
            }
            _ => return Err(self.err("expected module name")),
        };
        let mut ports = Vec::new();
        let mut params = Vec::new();
        let mut decls = Vec::new();
        let mut items = Vec::new();

        // Handle import statements between module name and #( / (
        // Bisa berisi beberapa item: `import pkg::A, pkg::B;` (OpenTitan
        // spid_readbuffer dll.) — satu import, banyak pasangan pkg::item.
        while self.peek() == &Token::Import {
            self.advance();
            loop {
                let pkg = self.expect_ident()?;
                self.expect(Token::Scope)?;
                let item = if self.peek() == &Token::Star {
                    self.advance();
                    Symbol::intern("*")
                } else {
                    self.expect_ident()?
                };
                // Register imported typedef supaya deklarasi berikut bisa pakai.
                // Nama EKSPLISIT (`import pkg::t`) = calon jenis port LANGSUNG
                // (IEEE 1800 §23.2.1) tanpa tunggu package_tdefs — package bisa
                // di file eksternal (kmac_pkg), parser per-file tak punya tabelnya.
                if item == "*" {
                    if let Some(tdefs) = self.package_tdefs.get(&pkg) {
                        for name in tdefs {
                            self.typedef_names.insert(*name);
                        }
                    }
                } else {
                    self.typedef_names.insert(item);
                }
                items.push(ModuleItem::Import { package: pkg, item });
                if self.peek() == &Token::Comma {
                    self.advance();
                    continue;
                }
                break;
            }
            self.skip_semi();
        }

        if self.peek() == &Token::Hash {
            self.advance();
            self.expect(Token::LParen)?;
            self.parse_param_list(&mut params)?;
            self.expect(Token::RParen)?;
        }

        if self.peek() == &Token::LParen {
            self.advance();
            if self.peek() != &Token::RParen {
                self.parse_port_list(&mut ports)?;
            }
            self.expect(Token::RParen)?;
        }
        if self.peek() == &Token::Semi {
            self.advance();
        } else if !matches!(self.peek(), Token::Eof) {
            // Header modul tidak diakhiri `;` — sintaks salah. Sebelumnya
            // ditelan diam-diam: `module top {` dianggap module valid (token
            // `{` jatuh ke fallback `_ => Ok(None)` yang tidak emit diag).
            // Kini peringatkan di lokasi token penyebab + fix-it sisip `;`
            // (infra push_warning_at). Recovery: token ditangani body loop.
            let line = self.peek_line();
            let col = self.peek_col();
            self.push_warning_at(
                format!("expected ';' after module header, found `{}`", self.peek()),
                line,
                col,
            );
        }
        self.skip_semi();

        let mut _last_pos = self.pos.get();
        let mut _stuck = 0u32;
        let _mod_start = std::time::Instant::now();
        let mut _mod_tokens = 0u64;
        loop {
            // Progress tracking
            if _mod_tokens > 0 && _mod_tokens.is_multiple_of(1000000) {
                eprintln!(
                    "[DBG-MODULE-BODY] {} items parsed, token {}/{}, elapsed {:?}",
                    _mod_tokens,
                    self.pos.get(),
                    self.tokens.len(),
                    _mod_start.elapsed()
                );
            }
            // Stuck detection: if pos hasn't changed for too many iterations, abort
            if self.pos.get() == _last_pos {
                _stuck += 1;
                if _stuck > 1_000_000 {
                    let line = self.peek_line();
                    let col = self.peek_col();
                    let tok_str = format!("{}", self.peek());
                    let summary = if tok_str.len() > 40 {
                        format!("{}...", &tok_str[..40])
                    } else {
                        tok_str
                    };
                    self.push_warning_at(
                        format!("parser stuck in module body at token: {}", summary),
                        line,
                        col,
                    );
                    return Err(self.err("parser stuck (no progress) in module body"));
                }
            } else {
                _stuck = 0;
                _last_pos = self.pos.get();
            }
            match self.peek() {
                Token::Endmodule | Token::EndInterface | Token::EndProgram | Token::Eof => break,
                Token::Input | Token::Output | Token::Inout => {
                    // Port deklarasi non-ANSI di body (lihat
                    // `parse_body_port_decls`). Direktur parse ke ports dan
                    // di-merge dgn header.
                    _mod_tokens += 1;
                    if let Err(e) = self.parse_body_port_decls(&mut ports) {
                        self.errors.push(e.to_diagnostic());
                        self.skip_until_semi_or_end()?;
                    }
                }
                _ => {
                    let before = self.pos.get();
                    let result = self.parse_module_item();
                    match result {
                        Ok(Some(item)) => {
                            _mod_tokens += 1;
                            if let ModuleItem::Covergroup(ref cg) = item {
                                self.class_names.insert(cg.name);
                            }
                            match item {
                                ModuleItem::Decl(d) => decls.push(d),
                                ModuleItem::Param(p) => params.push(p),
                                other => items.push(other),
                            }
                        }
                        Ok(None) => {
                            _mod_tokens += 1;
                            // If position didn't advance, skip the token to avoid infinite loop
                            if self.pos.get() == before {
                                self.advance();
                            }
                        }
                        Err(e) => {
                            _mod_tokens += 1;
                            let diag = e.to_diagnostic();
                            self.errors.push(diag);
                            self.skip_until_semi_or_end()?;
                        }
                    }
                }
            }
        }

        match self.peek() {
            Token::EndProgram => {
                self.advance();
            }
            Token::EndInterface => {
                self.advance();
            }
            _ => {
                self.expect(Token::Endmodule)?;
            }
        }
        if self.peek() == &Token::Colon {
            self.advance();
            if matches!(self.peek(), Token::Ident(_)) {
                self.advance();
            }
        }

        Ok(Module {
            name,
            ports,
            params,
            decls,
            items,
            timescale: self.timescale_at_line.take(),
        })
    }

    /// Fast skip: quickly advance past module body without parsing items.
    /// Used in first pass (class discovery) where we only need class names,
    /// not full module structure. Dramatically faster than parse_module().
    pub(crate) fn parse_module_fast(&mut self) -> Result<(), SimError> {
        self.advance(); // consume 'module'
        match self.peek() {
            Token::Ident(_) => {
                self.advance();
            }
            Token::Dollar => {
                // Yosys module name `$...` — fast path (discovery pass).
                self.advance();
                if !matches!(self.peek(), Token::Ident(_)) {
                    return Err(self.err("expected module name"));
                }
                self.advance();
            }
            _ => return Err(self.err("expected module name")),
        }
        // Skip #(params) if any
        if self.peek() == &Token::Hash {
            self.advance();
            if self.peek() == &Token::LParen {
                self.skip_balanced_paren()?;
            }
        }
        // Skip (ports) if any
        if self.peek() == &Token::LParen {
            self.skip_balanced_paren()?;
        }
        self.skip_semi();
        // Fast-skip module body until endmodule
        loop {
            match self.peek() {
                Token::Endmodule | Token::EndInterface | Token::EndProgram | Token::Eof => {
                    if self.peek() != &Token::Eof {
                        self.advance();
                    }
                    // Consume optional 'endmodule : name' suffix
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    break;
                }
                Token::Class => {
                    // Collect class name from inside module
                    let start = self.pos.get();
                    self.advance(); // consume 'class'
                    if self.peek() == &Token::Hash {
                        self.advance();
                        if self.peek() == &Token::LParen {
                            let _ = self.skip_balanced_paren_light();
                        }
                    }
                    if let Token::Ident(name) = self.peek() {
                        self.class_names.insert(*name);
                    }
                    self.pos.set(start);
                    // Skip class body
                    // Re-use existing skip_class_body
                    self.skip_class_body();
                }
                _ => {
                    // Check for (* attribute annotations
                    if self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
                        self.skip_attribute();
                    } else {
                        self.advance();
                    }
                }
            }
        }
        // Skip optional ': name' after endmodule
        if self.peek() == &Token::Colon {
            self.advance();
            if matches!(self.peek(), Token::Ident(_)) {
                self.advance();
            }
        }
        Ok(())
    }

    pub(crate) fn parse_interface_fast(&mut self) -> Result<(), SimError> {
        self.advance(); // consume 'interface'
                        // Skip name
        match self.peek() {
            Token::Ident(_) | Token::Hash => { /* name optional if #(params) follows */ }
            _ => return Err(self.err("expected interface name")),
        }
        if matches!(self.peek(), Token::Ident(_)) {
            self.advance(); // consume name
        }
        // Skip #(params) if any
        if self.peek() == &Token::Hash {
            self.advance();
            if self.peek() == &Token::LParen {
                self.skip_balanced_paren_light()?;
            }
        }
        // Skip (ports) if any
        if self.peek() == &Token::LParen {
            self.skip_balanced_paren_light()?;
        }
        self.skip_semi();
        // Fast-skip interface body until endinterface
        loop {
            match self.peek() {
                Token::EndInterface | Token::Eof => {
                    if self.peek() != &Token::Eof {
                        self.advance();
                    }
                    break;
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
        // Skip optional ': name' after endinterface
        if self.peek() == &Token::Colon {
            self.advance();
            if matches!(self.peek(), Token::Ident(_)) {
                self.advance();
            }
        }
        Ok(())
    }

    pub(crate) fn parse_program_fast(&mut self) -> Result<(), SimError> {
        self.advance(); // consume 'program'
        if let Token::Ident(_) = self.peek() {
            self.advance();
        }
        if self.peek() == &Token::Hash {
            self.advance(); // #
            if self.peek() == &Token::LParen {
                self.skip_balanced_paren()?;
            }
        }
        if self.peek() == &Token::LParen {
            self.skip_balanced_paren()?;
        }
        self.skip_semi();
        loop {
            match self.peek() {
                Token::EndProgram | Token::Eof => {
                    self.advance();
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    break;
                }
                _ => {
                    self.advance();
                }
            }
        }
        Ok(())
    }

    pub(crate) fn skip_balanced_paren(&mut self) -> Result<(), SimError> {
        let mut depth = 0;
        loop {
            match self.peek() {
                Token::LParen => {
                    depth += 1;
                    self.advance();
                }
                Token::RParen => {
                    depth -= 1;
                    self.advance();
                    if depth == 0 {
                        break;
                    }
                }
                Token::Eof => return Err(self.err("unexpected EOF in balanced paren")),
                _ => {
                    self.advance();
                }
            }
        }
        Ok(())
    }

    pub(crate) fn parse_interface(&mut self) -> Result<Interface, SimError> {
        // F84 (LRM §19.8): interface punya satuan delay sendiri, sama seperti
        // module — direkam sebelum keyword-nya di-konsumsi.
        let ts_line = self
            .timescale_segments
            .iter()
            .rev()
            .find(|(from, _)| *from <= self.peek_line())
            .map(|(_, ts)| ts.clone());
        self.advance(); // consume 'interface'
        self.typedef_names.clear();
        // Re-seed typedef GLOBAL (lintas file) yang di-clear di atas.
        self.typedef_names
            .extend(self.global_typedef_names.iter().copied());
        // Hygiene defensif: simetris dgn parse_module, cegah kejutan bila
        // interface kelak mulai memakai type param.
        self.module_type_params.clear();

        // Skip (* ... *) attributes before interface name
        while self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
            self.skip_attribute();
        }

        let name = match self.peek() {
            Token::Ident(s) => {
                let n = *s;
                self.advance();
                n
            }
            _ => return Err(self.err("expected interface name")),
        };
        let mut ports = Vec::new();
        let mut params = Vec::new();
        let mut decls = Vec::new();
        let mut items = Vec::new();
        let mut modports = Vec::new();

        // Parse header import clause: `interface foo import pkg::*; (...)`
        // Package import di header interface perlu di-collect ke items agar
        // params dari package ter-import tersedia di generate expansion context.
        while self.peek() == &Token::Import {
            self.advance(); // consume 'import'
            while let Token::Ident(s) = self.peek() {
                let pkg = *s;
                self.advance();
                if self.peek() == &Token::Scope {
                    self.advance(); // consume '::'
                    let item_name = match self.peek() {
                        Token::Star => {
                            self.advance();
                            Symbol::intern("*")
                        }
                        Token::Ident(s) => {
                            let n = *s;
                            self.advance();
                            n
                        }
                        _ => Symbol::intern("*"),
                    };
                    items.push(ModuleItem::Import {
                        package: pkg,
                        item: item_name,
                    });
                    // IEEE 1800 §23.2.1 header import: registrasi typedef agar
                    // port ANATYPE ter-resolve — `inout wire app_req_t req`,
                    // app_req_t dari kmac_pkg (kmac_app_if.sv). Sama spt
                    // parse_module/import-body.
                    // IEEE 1800 §23.2.1 header import: registrasi typedef agar
                    // port ANATYPE ter-resolve — `inout wire app_req_t req`,
                    // app_req_t dari kmac_pkg (kmac_app_if.sv). Nama eksplisit
                    // LANGSUNG jadi calon jenis (package eksternal tak punya
                    // tabel); wildcard via package_tdefs.
                    if item_name == "*" {
                        if let Some(tdefs) = self.package_tdefs.get(&pkg) {
                            for n in tdefs {
                                self.typedef_names.insert(*n);
                            }
                        }
                    } else {
                        self.typedef_names.insert(item_name);
                    }
                }
                if self.peek() == &Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            self.skip_semi();
        }

        // Parse #(parameter ...) list (like module syntax)
        if self.peek() == &Token::Hash {
            self.advance();
            self.expect(Token::LParen)?;
            self.parse_param_list(&mut params)?;
            self.expect(Token::RParen)?;
        }

        // Parse (port list)
        if self.peek() == &Token::LParen {
            self.advance();
            if self.peek() != &Token::RParen {
                self.parse_port_list(&mut ports)?;
            }
            self.expect(Token::RParen)?;
        }
        self.skip_semi();

        let mut _last_pos = self.pos.get();
        let mut _stuck = 0u32;

        loop {
            if self.pos.get() == _last_pos {
                _stuck += 1;
                if _stuck > 1_000_000 {
                    let line = self.peek_line();
                    let col = self.peek_col();
                    let tok_str = format!("{}", self.peek());
                    let summary = if tok_str.len() > 40 {
                        format!("{}...", &tok_str[..40])
                    } else {
                        tok_str
                    };
                    self.push_warning_at(
                        format!("parser stuck in interface body at token: {}", summary),
                        line,
                        col,
                    );
                    return Err(self.err("parser stuck (no progress) in interface body"));
                }
            } else {
                _stuck = 0;
                _last_pos = self.pos.get();
            }
            match self.peek() {
                Token::EndInterface | Token::Eof => {
                    break;
                }
                _ => match self.peek() {
                    Token::ModPort => {
                        modports.push(self.parse_modport()?);
                    }
                    Token::LParen if self.peek_ahead(1) == &Token::Star => {
                        self.skip_attribute();
                    }
                    Token::Input | Token::Output | Token::Inout => {
                        // Port deklarasi non-ANSI di body interface — simetris
                        // dengan parse_module (lihat parse_body_port_decls).
                        if let Err(e) = self.parse_body_port_decls(&mut ports) {
                            self.errors.push(e.to_diagnostic());
                            self.skip_until_semi_or_end()?;
                        }
                    }
                    _ => {
                        let before = self.pos.get();
                        match self.parse_module_item() {
                            Ok(Some(item)) => {
                                if let ModuleItem::Covergroup(ref cg) = item {
                                    self.class_names.insert(cg.name);
                                }
                                match item {
                                    ModuleItem::Decl(d) => decls.push(d),
                                    ModuleItem::Param(p) => params.push(p),
                                    other => items.push(other),
                                }
                            }
                            Ok(None) => {
                                // If position didn't advance, skip the token to avoid infinite loop
                                if self.pos.get() == before {
                                    self.advance();
                                }
                            }
                            Err(e) => {
                                self.errors.push(e.to_diagnostic());
                                self.skip_until_semi_or_end()?;
                            }
                        }
                    }
                },
            }
        }
        match self.peek() {
            Token::EndInterface => {
                self.advance();
            }
            _ => {
                return Err(self.err("expected endinterface"));
            }
        }
        if self.peek() == &Token::Colon {
            self.advance();
            if matches!(self.peek(), Token::Ident(_)) {
                self.advance();
            }
        }

        Ok(Interface {
            name,
            params,
            ports,
            decls,
            items,
            modports,
            timescale: ts_line,
        })
    }

    pub(crate) fn parse_modport(&mut self) -> Result<Modport, SimError> {
        self.advance(); // consume 'modport'
        let name = match self.peek() {
            Token::Ident(s) => {
                let n = *s;
                self.advance();
                n
            }
            _ => return Err(self.err("expected modport name")),
        };
        self.expect(Token::LParen)?;
        let mut items = Vec::new();
        loop {
            // Modport clocking reference: `modport host_mp(clocking cb, ...)`
            // (LRM 1800 §25.5.3) — nama clocking dipakai sbg interface item.
            if self.peek() == &Token::Clocking {
                self.advance();
                let cb_name = self.expect_ident()?;
                items.push(ModportItem {
                    name: cb_name,
                    direction: PortDirection::Input,
                });
                match self.peek() {
                    Token::Comma => {
                        self.advance();
                        continue;
                    }
                    Token::RParen => {
                        self.advance();
                        break;
                    }
                    _ => continue,
                }
            }
            let dir = match self.peek() {
                Token::Input => {
                    self.advance();
                    PortDirection::Input
                }
                Token::Output => {
                    self.advance();
                    PortDirection::Output
                }
                Token::Inout => {
                    self.advance();
                    PortDirection::Inout
                }
                _ => return Err(self.err("expected direction in modport")),
            };
            // Collect all signals under this direction, comma-separated
            loop {
                let sig_name = match self.peek() {
                    Token::Ident(s) => {
                        let n = *s;
                        self.advance();
                        n
                    }
                    _ => return Err(self.err("expected signal name in modport")),
                };
                items.push(ModportItem {
                    name: sig_name,
                    direction: dir,
                });
                match self.peek() {
                    Token::Comma => {
                        self.advance();
                        // Check if next token is a direction (then break inner loop)
                        match self.peek() {
                            Token::Input | Token::Output | Token::Inout => break,
                            _ => continue,
                        }
                    }
                    _ => break,
                }
            }
            match self.peek() {
                Token::RParen => {
                    self.advance();
                    break;
                }
                _ => continue,
            }
        }
        self.skip_semi();
        Ok(Modport { name, items })
    }

    /// Port deklarasi gaya NON-ANSI di body module/interface:
    /// `input [7:0] a;` / `output logic y;` / `inout wire [3:0] z;`.
    /// Sebelumnya token `input/output/inout` tidak punya arm di
    /// `parse_module_item_body` → token di-skip satu per satu → range
    /// `[7:0]` DIBUANG dan port jatuh ke lebar default 1-bit (mis. `b` di
    /// netlist alu_opt jadi 1 bit → hasil sim salah). Sintaks body identik
    /// dengan header (`parse_port_list`), lalu hasilnya di-MERGE ke daftar
    /// ports: body MELENGKAPI/MENIMPA port yang sudah dideklarasi di header
    /// non-ANSI (`module m(a, b, y); input [7:0] a; ...`).
    fn parse_body_port_decls(&mut self, ports: &mut Vec<Port>) -> Result<(), SimError> {
        let mut body_ports = Vec::new();
        self.parse_port_list(&mut body_ports)?;
        self.skip_semi();
        for np in body_ports {
            match ports.iter_mut().find(|p| p.name == np.name) {
                Some(existing) => {
                    existing.direction = np.direction;
                    // Body yang punya range/dtype MENIMPA header yang kosong
                    // (header non-ANSI cuma nama). Jangan menimpa dengan None.
                    if np.range.is_some() || np.expr_range.is_some() || np.array_range.is_some() {
                        existing.range = np.range;
                        existing.expr_range = np.expr_range;
                        existing.array_range = np.array_range;
                        existing.extra_packed_dims = np.extra_packed_dims;
                    }
                    if np.dtype_name.is_some() {
                        existing.dtype_name = np.dtype_name;
                    }
                }
                None => ports.push(np),
            }
        }
        Ok(())
    }

    pub(crate) fn parse_port_list(&mut self, ports: &mut Vec<Port>) -> Result<(), SimError> {
        loop {
            if self.peek() == &Token::RParen || self.peek() == &Token::Eof {
                break;
            }

            let tok = self.peek().clone();
            match tok {
                // `.name` / `.name(expr)` adalah sintaks KONEKSI INSTANCE,
                // bukan deklarasi di module port list. `module top (.x);`
                // TIDAK valid (IEEE 1800 §23.2.1) — error.
                Token::Dot => {
                    return Err(self.err(
                        "unexpected `.` di port list module — koneksi named-port \
                         hanya valid di instance (`.clk_i`), bukan deklarasi port",
                    ));
                }
                Token::Comma => {
                    self.advance(); // skip stray comma
                }
                _ => {
                    let dir = match self.peek() {
                        Token::Input => {
                            self.advance();
                            PortDirection::Input
                        }
                        Token::Output => {
                            self.advance();
                            PortDirection::Output
                        }
                        Token::Inout => {
                            self.advance();
                            PortDirection::Inout
                        }
                        _ => PortDirection::Input,
                    };

                    let consumed_keyword_type = if matches!(
                        self.peek(),
                        Token::Wire
                            | Token::Reg
                            | Token::Logic
                            | Token::Bit
                            | Token::Byte
                            | Token::Shortint
                            | Token::Longint
                            | Token::Time
                            | Token::Int
                            | Token::Integer
                    ) {
                        self.advance();
                        true
                    } else {
                        false
                    };

                    // Check for type parameter reference (identifier before port name or range)
                    let mut dtype_name = None;
                    if !consumed_keyword_type {
                        if let Token::Ident(_s) = self.peek() {
                            let ah1 = self.peek_ahead(1).clone();
                            if ah1 == Token::Scope {
                                let pkg = self.expect_ident()?;
                                self.expect(Token::Scope)?;
                                let typ = self.expect_ident()?;
                                dtype_name = Some(format!("{}::{}", pkg, typ));
                            } else if ah1 == Token::Dot {
                                // Port bertipe interface + modport: `axi_lite.dut bus`
                                // → dtype_name = "axi_lite.dut". Elaborator men-strip
                                // bagian modport saat mencocokkan nama interface.
                                let iface = self.expect_ident()?;
                                self.expect(Token::Dot)?;
                                let mp = self.expect_ident()?;
                                dtype_name = Some(format!("{}.{}", iface, mp));
                            } else if matches!(ah1, Token::Ident(_) | Token::LBrack) {
                                let name = self.expect_ident()?;
                                dtype_name = Some(name.as_str().to_string());
                            }
                        }
                    } // if !consumed_keyword_type

                    // IEEE 1800 §23.2.2.3: net type ATAU keyword type + user
                    // typedef — `input wire app_req_t req` (app_req_t = typedef
                    // dari import body/header). Saat keyword type dikonsumsi,
                    // ident BERIKUT yang TERDAFTAR di typedef_names = TIPE,
                    // bukan nama port. Tanpa ini `app_req_t` salah-parse jadi
                    // nama port → `req` berikutnya = E1002 (kmac_app_if).
                    if dtype_name.is_none() && consumed_keyword_type {
                        if let Token::Ident(s) = self.peek() {
                            if self.typedef_names.contains(&Symbol::intern(s.as_str())) {
                                let name = self.expect_ident()?;
                                dtype_name = Some(name.as_str().to_string());
                            }
                        }
                    }

                    if self.peek() == &Token::Signed {
                        self.advance();
                    }
                    // `int unsigned`, `byte unsigned` dll — unsigned = default,
                    // konsumsi agar tidak ter-parse sebagai port name.
                    if self.peek() == &Token::Unsigned {
                        self.advance();
                    }

                    let expr_range = if self.peek() == &Token::LBrack {
                        self.parse_range()?
                    } else {
                        None
                    };
                    // Parse additional packed dimensions before port name: [a:b][c:d]
                    let mut extra_packed_dims = Vec::new();
                    while self.peek() == &Token::LBrack {
                        if let Some(er) = self.parse_range()? {
                            extra_packed_dims.push(er);
                        }
                    }
                    let range = expr_range.as_ref().and_then(|er| {
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

                    // Port bertipe typedef/interface (dtype_name non-None) dan
                    // BUKAN type parameter: dimensi `[a:b]` SEBELUM nama port
                    // adalah dimensi UNPACKED array, bukan packed range vector.
                    // Contoh OpenTitan: `input cmd_info_t [NumTotalCmdInfo-1:0]
                    // cmd_info_i` — elemen array = seluruh typedef (struct
                    // 23-bit), bukan bit-vector selebar dimensi. Tanpa ini
                    // parser menaruh `[3:0]` sebagai packed range → port 4-bit
                    // + elem select `cmd_info_i[i]` = bit tunggal (WR0102
                    // rhs=1). Type parameter DIKECUALIKAN — `T [7:0]` di mana
                    // T = logic/bit adalah packed range vector (test
                    // `parameter type T = logic`), bukan array.
                    let (range, expr_range, extra_packed_dims, array_range_pre) = if dtype_name
                        .is_some()
                        && !self
                            .module_type_params
                            .contains(&Symbol::intern(dtype_name.as_deref().unwrap_or("")))
                    {
                        let ar = expr_range.as_ref().and_then(|er| {
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
                        (None, None, Vec::new(), ar)
                    } else {
                        (range, expr_range, extra_packed_dims, None)
                    };

                    loop {
                        let name_tok = self.peek().clone();
                        match &name_tok {
                            Token::Ident(name) => {
                                self.advance();
                                // Parse unpacked array dimension(s) after port name:
                                //   data_i [N]          — ukuran tunggal (→ [N-1:0])
                                //   data_i [msb:lsb]    — rentang eksplisit
                                // (multi-dimensi diperbolehkan, dimensi ekstra di-skip)
                                let mut array_range = None;
                                let mut extra_unpacked_dims: Vec<(Option<Range>, Option<Expr>)> =
                                    Vec::new();
                                if self.peek() == &Token::LBrack {
                                    self.advance(); // [
                                    if self.peek() != &Token::RBrack {
                                        // Parse ekspresi pertama, lalu putuskan:
                                        //   [N]        — ukuran tunggal (→ [N-1:0])
                                        //   [msb:lsb]  — rentang eksplisit
                                        let first = self.parse_expr(0)?;
                                        if self.peek() == &Token::Colon {
                                            self.advance();
                                            let second = self.parse_expr(0)?;
                                            self.expect(Token::RBrack)?;
                                            if let (Ok(m), Ok(l)) = (
                                                const_eval_simple(&first),
                                                const_eval_simple(&second),
                                            ) {
                                                array_range = Some(Range {
                                                    msb: m as usize,
                                                    lsb: l as usize,
                                                });
                                            }
                                        } else {
                                            self.expect(Token::RBrack)?;
                                            if let Ok(n) = const_eval_simple(&first) {
                                                if n > 0 {
                                                    array_range = Some(Range {
                                                        msb: (n - 1) as usize,
                                                        lsb: 0,
                                                    });
                                                }
                                            }
                                        }
                                    } else {
                                        self.advance(); // ]
                                    }
                                    // Dimensi unpacked LANJUTAN `[..][..]`
                                    // (F39): parse range/size → simpan ke
                                    // extra_unpacked_dims (sebelumnya di-skip).
                                    while self.peek() == &Token::LBrack {
                                        let exo = matches!(
                                            self.peek_ahead(1),
                                            Token::RBrack
                                                | Token::Dollar
                                                | Token::String
                                                | Token::Int
                                                | Token::Unsigned
                                                | Token::Star
                                                | Token::Bit
                                                | Token::Logic
                                                | Token::Byte
                                                | Token::Shortint
                                                | Token::Longint
                                        );
                                        if exo {
                                            // Exotik — skip buta (perilaku lama).
                                            self.skip_extra_unpacked_dims();
                                            break;
                                        }
                                        if self.peek_bracket_has_range_colon() {
                                            // `parse_range` mengkonsumsi
                                            // `[msb:lsb]` sendiri — jangan
                                            // advance `[` lebih dulu.
                                            match self.parse_range() {
                                                Ok(Some(er)) => {
                                                    if let (Ok(m), Ok(l)) = (
                                                        const_eval_simple(&er.msb),
                                                        const_eval_simple(&er.lsb),
                                                    ) {
                                                        extra_unpacked_dims.push((
                                                            Some(Range {
                                                                msb: m as usize,
                                                                lsb: l as usize,
                                                            }),
                                                            None,
                                                        ));
                                                    } else {
                                                        extra_unpacked_dims.push((None, None));
                                                    }
                                                }
                                                Ok(None) | Err(_) => {
                                                    self.skip_extra_unpacked_dims();
                                                    break;
                                                }
                                            }
                                        } else {
                                            self.advance(); // [
                                            let sz = self.parse_expr(0)?;
                                            self.expect(Token::RBrack)?;
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
                                    }
                                }
                                // Initializer port ANSI: `output reg [7:0] b
                                // = 8'h2A;` — SV legal (default value saat
                                // sim). Sebelumnya `=` tidak di-parse → token
                                // tersisa → `expected )` → module gagal parse
                                // (E3001 module not found).
                                let init_expr = if self.peek() == &Token::BlockingAssign {
                                    self.advance();
                                    Some(self.parse_expr(0)?)
                                } else {
                                    None
                                };
                                ports.push(Port {
                                    name: *name,
                                    direction: dir,
                                    range: range.clone(),
                                    expr_range: expr_range.clone(),
                                    dtype_name: dtype_name.as_ref().map(|s| Symbol::intern(s)),
                                    array_range: array_range_pre.clone().or(array_range),
                                    extra_unpacked_dims,
                                    extra_packed_dims: extra_packed_dims.clone(),
                                    init_expr,
                                });
                            }
                            _ => break,
                        }

                        if self.peek() == &Token::Comma {
                            let ahead = self.peek_ahead(1).clone();
                            // Port baru dimulai dengan direction (input/output/
                            // inout), tipe data (logic/wire/reg/bit/byte/int/…
                            // — port ANSI dengan direction implicit, umum di
                            // OpenTitan: `logic clk_edn_i,`), atau ident yang
                            // diikuti `::` (pkg::type). Tanpa guard ini, comma
                            // setelah port `logic x` di-consume lalu token
                            // `logic` berikutnya membuat parse_port_list berhenti
                            // → `expected RParen, found logic` (E1002 palsu).
                            let is_new_port = matches!(
                                ahead,
                                Token::Input
                                    | Token::Output
                                    | Token::Inout
                                    | Token::Wire
                                    | Token::Reg
                                    | Token::Logic
                                    | Token::Bit
                                    | Token::Byte
                                    | Token::Shortint
                                    | Token::Longint
                                    | Token::Time
                                    | Token::Int
                                    | Token::Integer
                            ) || (matches!(&ahead, Token::Ident(_))
                                && matches!(self.peek_ahead(2), Token::Scope))
                                // `Ident . Ident` = port interface+modport
                                // (`AXI_BUS.Slave in, AXI_BUS.Master out` —
                                // axi_cut.sv cva6): comma = batas port BARU,
                                // bukan lanjutan nama. Tanpa ini port kedua
                                // salah-parse → `expected RParen, found Dot`.
                                || (matches!(&ahead, Token::Ident(_))
                                    && matches!(self.peek_ahead(2), Token::Dot))
                                || (matches!(&ahead, Token::Ident(_))
                                    && matches!(self.peek_ahead(2), Token::Ident(_)));
                            // `Ident Ident` setelah comma = port baru bertipe
                            // user-defined: `input clk_i, flash_ctrl_err_t
                            // op_err_o` (flash_ctrl_err_t = type dari package).
                            // Tanpa ini `flash_ctrl_err_t` dikira nama port
                            // lanjutan deklarasi `clk_i` → `op_err_o` error
                            // `expected RParen` → module hilang (E3001).
                            // Kasus lanjutan `input a, b, c;` (Ident + Comma /
                            // Semi) tetap dikira lanjutan — benar.
                            if !is_new_port {
                                self.advance();
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }

            if self.peek() == &Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Parse blok parameter instance `#(...)` (SEBELUM nama instance).
    /// Caller memastikan `self.peek() == Hash` — helper mengonsumsi `#(`
    /// sampai `)`. Menghasilkan dua map: parameter nilai (`Symbol → Expr`)
    /// dan parameter tipe (`Symbol → TypeParamAssign`: tipe + range packed
    /// literal bila ada). Item posisional diberi
    /// key `__param0`, `__param1`, … (konvensi lama elaborator).
    /// F31: dipakai di DUA posisi (`mod #(.P(1)) u (...)` dan
    /// `mod u #(.P(1)) (...)`).
    pub(crate) fn parse_param_block(
        &mut self,
    ) -> Result<
        (
            std::collections::HashMap<Symbol, mivon_ast::Expr>,
            std::collections::HashMap<Symbol, mivon_ast::TypeParamAssign>,
        ),
        SimError,
    > {
        let mut param_assigns: std::collections::HashMap<Symbol, mivon_ast::Expr> =
            std::collections::HashMap::new();
        let mut type_param_assigns: std::collections::HashMap<
            Symbol,
            mivon_ast::TypeParamAssign,
        > = std::collections::HashMap::new();

        self.advance(); // consume '#'
        self.expect(Token::LParen)?;
        if self.peek() != &Token::RParen {
            loop {
                // Trailing comma: `#(.A(1), .B(2),)` (kmac_reduced_tb) —
                // setelah koma langsung `)`. Toleransi agar tidak error
                // "expected expression, found RParen".
                if self.peek() == &Token::RParen {
                    break;
                }
                if self.peek() == &Token::Dot {
                    self.advance();
                    let pname_tok = self.peek().clone();
                    let pname: Symbol = match &pname_tok {
                        Token::Ident(s) => {
                            self.advance();
                            *s
                        }
                        _ => return Err(self.err("expected parameter name")),
                    };
                    self.expect(Token::LParen)?;
                    if self.is_type_token() {
                        // Range packed literal (`logic[15:0]`) dipertahankan
                        // (bukan dibuang `parse_type_expr`) agar lebar
                        // override ter-resolve di elaborasi.
                        let (dt, range) = self.parse_type_expr_with_range()?;
                        self.expect(Token::RParen)?;
                        type_param_assigns.insert(
                            pname,
                            mivon_ast::TypeParamAssign { dtype: dt, range },
                        );
                    } else {
                        let val = self.parse_expr(0)?;
                        self.expect(Token::RParen)?;
                        param_assigns.insert(pname, val);
                    }
                } else {
                    let val = self.parse_expr(0)?;
                    param_assigns.insert(
                        Symbol::intern(&format!("__param{}", param_assigns.len())),
                        val,
                    );
                }

                if self.peek() == &Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        Ok((param_assigns, type_param_assigns))
    }

    pub(crate) fn parse_instance(&mut self) -> Result<ModuleInstance, SimError> {
        // Atribut `(* ... *)` langsung sebelum instance (bila pemanggil belum
        // mengkonsumsinya — jalur module-item mem-parse sendiri lalu
        // menyuntikkan; jalur lain jatuh ke sini).
        let mut attrs: Vec<AttrEntry> = Vec::new();
        if self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
            attrs = self.parse_attribute_entries();
        }
        let name_tok = self.peek().clone();
        // Catat posisi token module name untuk diagnostic (baris/kolom source).
        let inst_line = self.peek_line();
        let inst_col = self.peek_col();
        let module_name = match &name_tok {
            Token::Ident(s) => {
                self.advance();
                *s
            }
            _ => return Err(self.err("expected module name")),
        };

        // F31 fix: blok parameter `#(...)` legal di DUA posisi di SV:
        // (1) SEBELUM nama instance — `mod #(.P(1)) u(...)` (Verilog-2001),
        // (2) SETELAH nama instance — `mod u #(.P(1)) (...)` (juga umum).
        // Sebelumnya hanya posisi (1) yang dikenali: `mod u #(...)` gagal
        // mem-parse param + port (instance jadi kosong → elaborator
        // memperlakukan sbg default → lebar port salah / signal tak pernah
        // di-assign). Helper dipanggil di kedua posisi.
        let (mut param_assigns, mut type_param_assigns) = if self.peek() == &Token::Hash {
            self.parse_param_block()?
        } else {
            (
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
            )
        };

        let inst_tok = self.peek().clone();
        let instance_name = match &inst_tok {
            Token::Ident(s) => {
                self.advance();
                *s
            }
            _ => return Err(self.err("expected instance name")),
        };

        // Parse optional array range [msb:lsb] for arrayed instances
        let range = if self.peek() == &Token::LBrack {
            let saved = self.pos.get();
            match self.parse_range() {
                Ok(Some(er)) => Some(er),
                _ => {
                    // `mod inst[N] (...)` / interface array `ifce vif[P](...)` —
                    // ukuran tunggal tanpa ':' → setara `[N-1:0]`.
                    self.pos.set(saved);
                    self.advance(); // '['
                    let size = self.parse_expr(0)?;
                    self.expect(Token::RBrack)?;
                    Some(ExprRange {
                        msb: Expr::BinaryOp {
                            op: BinaryOp::Sub,
                            lhs: Box::new(size),
                            rhs: Box::new(Expr::Value(Value::Decimal(1))),
                        },
                        lsb: Expr::Value(Value::Decimal(0)),
                    })
                }
            }
        } else {
            None
        };

        // F31: posisi (2) — `mod u #(.P(1)) (...)`; extend (bukan timpa)
        // agar kombinasi aneh `mod #(.A(1)) u #(.B(2)) (...)` tetap jalan.
        if self.peek() == &Token::Hash {
            let (pa, ta) = self.parse_param_block()?;
            param_assigns.extend(pa);
            type_param_assigns.extend(ta);
        }

        let mut port_conns = Vec::new();
        if self.peek() == &Token::LParen {
            self.advance();
            if self.peek() != &Token::RParen {
                loop {
                    // `continue` dari `.*` kembali ke sini — periksa RParen agar
                    // `mod u(.*);` tidak jatuh ke parse_expr positional.
                    if self.peek() == &Token::RParen {
                        break;
                    }
                    // IEEE 1800 §22.11: `(* async *) .p(...)` — atribut sebelum
                    // koneksi (cdc_fifo_gray.sv PULP). Tanpa skip → "expected
                    // expression, found Star".
                    if self.peek() == &Token::LParen && self.peek_ahead(1) == &Token::Star {
                        self.skip_attribute();
                        continue;
                    }
                    if self.peek() == &Token::Dot {
                        self.advance();

                        if self.peek() == &Token::Star {
                            self.advance();
                            // `.*` (implicit port connection, IEEE 1800 §23.2.2.3)
                            // Dulu di-skip total → port_conns KOSONG → SEMUA
                            // port tak terhubung (input mengambang X, output
                            // tak ter-drive) — fuzzer: `sub dut (.*)` → y=q=X.
                            // Parser tak tahu daftar port target → kirim
                            // SENTINEL Named{port:"*"} → elab mengekspansi
                            // ke tiap port target_module (lihat
                            // elaborator/mod.rs arm Named port=="*").
                            port_conns.push(PortConnection::Named {
                                port: Symbol::intern("*"),
                                expr: Expr::Ident {
                                    name: Symbol::intern("*"),
                                    line: 0,
                                    col: 0,
                                },
                            });
                            continue;
                        }

                        let port_tok = self.peek().clone();
                        let port_name = match &port_tok {
                            Token::Ident(s) => {
                                self.advance();
                                *s
                            }
                            _ => return Err(self.err("expected port name")),
                        };

                        if self.peek() == &Token::LParen {
                            self.advance();
                            if self.peek() != &Token::RParen {
                                let expr = self.parse_expr(0)?;
                                self.expect(Token::RParen)?;
                                port_conns.push(PortConnection::Named {
                                    port: port_name,
                                    expr,
                                });
                            } else {
                                // `.port()` kosong = UNCONNECTED (bukan 0!).
                                self.expect(Token::RParen)?;
                                port_conns.push(PortConnection::Unconnected { port: port_name });
                            }
                        } else {
                            port_conns.push(PortConnection::Named {
                                port: port_name,
                                expr: Expr::Ident {
                                    name: port_name,
                                    line: 0,
                                    col: 0,
                                },
                            });
                        }
                    } else {
                        let expr = self.parse_expr(0)?;
                        port_conns.push(PortConnection::Positional(expr));
                    }

                    if self.peek() == &Token::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RParen)?;
        } else if self.peek() == &Token::Dot {
            // SV juga mengizinkan koneksi port TANPA kurung & koma:
            //   `mod u .a(x) .b(y);`   (space-separated named connections)
            // Sebelumnya bentuk ini TIDAK pernah di-parse — instance jadi tanpa
            // koneksi port → port instance tidak ter-resolve → proses always_ff
            // di module sel tak pernah ter-trigger (FF tidak berdetak, output
            // menggantung z). Ditemukan fase 3 (netlist) saat men-simulasi
            // netlist.v yang di-emit dgn gaya space-separated.
            // Loop di-guard `while Dot` agar `.*` wildcard tidak menelan token
            // setelahnya (mis. `;` penutup) saat `continue`.
            while self.peek() == &Token::Dot {
                self.advance(); // '.'
                if self.peek() == &Token::Star {
                    self.advance(); // '.*' wildcard — skip, while re-checks Dot
                    continue;
                }
                let port_name = match self.peek() {
                    Token::Ident(s) => {
                        let n = *s;
                        self.advance();
                        n
                    }
                    _ => {
                        return Err(self.err("expected port name after '.' in instance connection"))
                    }
                };
                let expr = if self.peek() == &Token::LParen {
                    self.advance();
                    if self.peek() != &Token::RParen {
                        let e = self.parse_expr(0)?;
                        self.expect(Token::RParen)?;
                        e
                    } else {
                        // `.port()` kosong = UNCONNECTED (bukan 0!).
                        self.expect(Token::RParen)?;
                        port_conns.push(PortConnection::Unconnected { port: port_name });
                        continue;
                    }
                } else {
                    // `.port` tanpa `(expr)` = koneksi ke signal senama.
                    Expr::Ident {
                        name: port_name,
                        line: 0,
                        col: 0,
                    }
                };
                port_conns.push(PortConnection::Named {
                    port: port_name,
                    expr,
                });
            }
        }

        if self.peek() != &Token::Semi {
            // If we have trailing tokens (e.g., = new() from misidentified class type), skip them
            self.skip_until_semi_or_end()?;
        } else {
            self.advance();
        }

        Ok(ModuleInstance {
            module_name,
            instance_name,
            range,
            param_assigns,
            type_param_assigns,
            port_conns,
            line: inst_line,
            col: inst_col,
            attrs,
        })
    }

    pub(crate) fn parse_gate_primitive(&mut self) -> Result<GatePrimitive, SimError> {
        let gate_type = match self.peek() {
            Token::And => {
                self.advance();
                GateType::And
            }
            Token::Or => {
                self.advance();
                GateType::Or
            }
            Token::Nand => {
                self.advance();
                GateType::Nand
            }
            Token::Nor => {
                self.advance();
                GateType::Nor
            }
            Token::Xor => {
                self.advance();
                GateType::Xor
            }
            Token::Xnor => {
                self.advance();
                GateType::Xnor
            }
            Token::Buf => {
                self.advance();
                GateType::Buf
            }
            Token::NotGate => {
                self.advance();
                GateType::Not
            }
            _ => return Err(self.err("expected gate type")),
        };

        // Parse optional drive strength: (strength1, strength0)
        let mut drive_strength = None;
        if self.peek() == &Token::LParen && matches!(self.peek_ahead(1), Token::Ident(_)) {
            // Check if this looks like drive strength, not port list
            let saved = self.pos.get();
            self.advance(); // consume (
            if let Token::Ident(s1) = self.peek().clone() {
                if is_strength_keyword(s1.as_str()) {
                    self.advance();
                    if self.peek() == &Token::Comma {
                        self.advance();
                        if let Token::Ident(s2) = self.peek().clone() {
                            if is_strength_keyword(s2.as_str()) {
                                self.advance();
                                if self.peek() == &Token::RParen {
                                    self.advance();
                                    drive_strength = Some((
                                        s1.as_str().to_lowercase(),
                                        s2.as_str().to_lowercase(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            if drive_strength.is_none() {
                self.pos.set(saved); // Not drive strength, restore position
            }
        }

        // Parse optional delay: #(rise, fall, turnoff) or #delay
        let mut delay = None;
        if self.peek() == &Token::Hash {
            self.advance();
            if self.peek() == &Token::LParen {
                self.advance();
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
                delay = Some(mivon_ast::types::Delay {
                    rise,
                    fall,
                    turnoff,
                });
            } else {
                // Single delay value
                let d = Some(self.parse_expr(0)?);
                delay = Some(mivon_ast::types::Delay {
                    rise: d.clone(),
                    fall: d,
                    turnoff: None,
                });
            }
        }

        let instance_name = if self.peek() == &Token::LParen {
            None
        } else {
            let name = match self.peek().clone() {
                Token::Ident(s) => {
                    self.advance();
                    Some(s)
                }
                _ => return Err(self.err("expected gate instance name")),
            };
            name
        };
        self.expect(Token::LParen)?;
        let mut ports = Vec::new();
        if self.peek() != &Token::RParen {
            loop {
                let expr = self.parse_expr(0)?;
                ports.push(expr);
                if self.peek() == &Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        self.skip_semi();
        Ok(GatePrimitive {
            gate_type,
            instance_name,
            ports,
            drive_strength,
            delay,
        })
    }
}
