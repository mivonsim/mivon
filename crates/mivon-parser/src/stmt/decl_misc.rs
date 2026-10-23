//! Deklarasi khusus statement-level: clocking event, wait_order, covergroup,
//! dan DPI-C import/export (termasuk skip range [N] dan tipe DPI).

use super::*;

impl Parser {
    pub(crate) fn parse_clocking_event(&mut self) -> Result<CovergroupClocking, SimError> {
        self.expect(Token::At)?;
        self.expect(Token::LParen)?;
        // Edge disimpan penuh (dulu di-skip → implicit sampling `@event`
        // tak mungkin). Tanpa edge → posedge (default praktis; `@(clk)`
        // level-ambigu, di-sample sbg posedge).
        let mut posedge = true;
        if self.peek() == &Token::PosEdge {
            self.advance();
        } else if self.peek() == &Token::NegEdge {
            self.advance();
            posedge = false;
        }
        let signal = self.parse_expr(0)?;
        self.expect(Token::RParen)?;
        Ok(CovergroupClocking {
            posedge,
            expr: signal,
        })
    }

    pub(crate) fn parse_wait_order(&mut self) -> Result<Stmt, SimError> {
        // Posisi keyword `wait_order` (1-based) — diagnostic elaborator.
        let wline = self.peek_line();
        let wcol = self.peek_col();
        self.advance();
        self.expect(Token::LParen)?;
        let mut events = Vec::new();
        if self.peek() != &Token::RParen {
            loop {
                events.push(self.expect_ident()?);
                if self.peek() == &Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        let fail_stmt = if self.peek() == &Token::Else {
            self.advance();
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };
        self.skip_semi();
        Ok(Stmt::WaitOrder {
            events,
            fail_stmt,
            line: wline,
            col: wcol,
        })
    }

    pub(crate) fn parse_covergroup(&mut self) -> Result<CovergroupDecl, SimError> {
        self.advance();
        let name = self.expect_ident()?;
        // Formal arguments `covergroup cg (input logic v, ...) @(...) BELUM
        // didukung. Dulu daftar arg DIABISKAN diam-diam: `v` tak pernah
        // terdefinisi → tiap coverpoint gagal jauh di elab dgn E2001
        // membingungkan (`signal 'v' not found` di baris coverpoint), dan
        // `@(...)` ikut tak terbaca (event hilang → implicit sampling mati).
        // Fail-fast di titik deklarasi dgn saran. Dukungan penuh butuh
        // IrExpr::CovergroupArg + binding `sample(a,b,c)` — desain terpisah.
        // Formal arguments `covergroup cg (int unsigned v, ...)` — IEEE 1800
        // §19.5. Fail-fast LAMA dihapus: error di HEADER membuat seluruh body
        // ikut ter-skip, lalu konstruk sesudahnya (`bins x = {[0:$]}`,
        // `bins s[] = fn()`) diparse jalur module-item yang salah → cascade
        // diagnostic SESAT ("expected system call name after $", "expected
        // instance name" — 7 dari 23 error OpenTitan berasal dari sini).
        // Daftar formal kini diparse & disimpan ke CovergroupDecl.formals;
        // binding nilai saat `sample(...)` tanggung jawab elaborasi.
        let mut formals: Vec<Symbol> = Vec::new();
        if self.peek() == &Token::LParen {
            self.advance(); // '('
            if self.peek() != &Token::RParen {
                loop {
                    // Konsumsi tipe + nama sampai ',' / ')' di depth0.
                    // Nama formal = token TERAKHIR (bentuk `type name`);
                    // tipe bisa keyword (`int unsigned`), typedef Ident,
                    // scoped `pkg::T`, atau packed dim `[3:0]`.
                    let mut depth = 0i32;
                    let mut last_ident: Option<Symbol> = None;
                    loop {
                        match self.peek() {
                            Token::Comma | Token::RParen if depth == 0 => break,
                            Token::LBrack | Token::LParen | Token::LBrace => {
                                depth += 1;
                                self.advance();
                            }
                            Token::RBrack | Token::RBrace => {
                                depth = depth.saturating_sub(1);
                                self.advance();
                            }
                            Token::RParen => {
                                depth -= 1;
                                self.advance();
                            }
                            Token::Ident(s) => {
                                last_ident = Some(*s);
                                self.advance();
                            }
                            Token::Eof => {
                                return Err(self.err("unterminated covergroup formal argument list"))
                            }
                            _ => self.advance(),
                        }
                    }
                    match last_ident {
                        Some(n) => formals.push(n),
                        None => {
                            return Err(self
                                .err("expected formal argument name in covergroup argument list"))
                        }
                    }
                    if self.peek() == &Token::Comma {
                        self.advance();
                        continue;
                    }
                    break;
                }
            }
            self.expect(Token::RParen)?;
        }
        let clocking_event = if self.peek() == &Token::At {
            Some(self.parse_clocking_event()?)
        } else {
            None
        };
        if let Token::Ident(s) = self.peek() {
            if *s == Symbol::intern("with") {
                self.advance();
                let mut depth = 0;
                loop {
                    match self.peek() {
                        Token::Semi if depth == 0 => {
                            self.advance();
                            break;
                        }
                        Token::LParen => {
                            depth += 1;
                            self.advance();
                        }
                        Token::RParen if depth > 0 => {
                            depth -= 1;
                            self.advance();
                        }
                        Token::Eof => break,
                        _ => {
                            self.advance();
                        }
                    }
                }
            } else {
                self.skip_semi();
            }
        } else {
            self.skip_semi();
        }
        let mut coverpoints = Vec::new();
        let mut crosses = Vec::new();
        // VERIF-28: `type_option.weight = N` / `type_option.per_instance = 1`
        // (juga varian `option.*`) — dibaca saat body loop; default weight 1,
        // per_instance false.
        let mut cg_weight: Option<u64> = None;
        let mut cg_per_instance: bool = false;
        loop {
            match self.peek() {
                Token::EndGroup | Token::Eof => {
                    self.advance();
                    if self.peek() == &Token::Colon {
                        self.advance();
                        if matches!(self.peek(), Token::Ident(_)) {
                            self.advance();
                        }
                    }
                    break;
                }
                Token::Ident(_) => {
                    let ident = self.expect_ident()?;
                    // VERIF-28: `type_option.weight = N` / `type_option.per_instance =
                    // 0|1` / `type_option.merge_instances = 0|1` — opsi tipe
                    // covergroup. Sebelumnya di-skip sampai ';' (opsi diabaikan).
                    if ident == Symbol::intern("type_option") && self.peek() == &Token::Dot {
                        self.advance(); // '.'
                        let opt = self.expect_ident()?;
                        self.expect(Token::BlockingAssign)?;
                        if opt == Symbol::intern("weight") {
                            let w = self.parse_expr(0)?;
                            cg_weight = const_eval_simple(&w).ok().map(|v| v as u64);
                        } else if opt == Symbol::intern("per_instance")
                            || opt == Symbol::intern("merge_instances")
                        {
                            let v = self.parse_expr(0)?;
                            let v = const_eval_simple(&v).ok().map(|x| x as u64);
                            if opt == Symbol::intern("per_instance") {
                                // per_instance=1 → per-instance; 0 → merge
                                cg_per_instance = v == Some(1);
                            } else {
                                // merge_instances=1 → merge; 0 → per-instance
                                cg_per_instance = v == Some(0);
                            }
                        } else {
                            let _ = self.parse_expr(0)?;
                        }
                        self.skip_semi();
                        continue;
                    }
                    if self.peek() == &Token::Colon {
                        self.advance();
                        match self.peek() {
                            Token::Coverpoint => {
                                self.advance();
                                // Coverpoint expression boleh berupa nama signal
                                // yang bertabrakan dgn keyword SV (`coverpoint param`,
                                // `coverpoint wait`, dst. — umum di kode DV).
                                // Token keyword tidak diparse sbg Ident oleh
                                // parse_expr → substitute sbg Ident signal.
                                let expr = if matches!(
                                    self.peek(),
                                    Token::Param | Token::Wait | Token::Time | Token::String
                                ) {
                                    let kw = match self.peek() {
                                        Token::Param => "param",
                                        Token::Wait => "wait",
                                        Token::Time => "time",
                                        _ => "string",
                                    };
                                    let dl = self.peek_line();
                                    let dc = self.peek_col();
                                    self.advance();
                                    Expr::Ident {
                                        name: Symbol::intern(kw),
                                        line: dl,
                                        col: dc,
                                    }
                                } else {
                                    self.parse_expr(0)?
                                };
                                let mut bins = Vec::new();
                                if self.peek() == &Token::LBrace {
                                    self.advance();
                                    loop {
                                        match self.peek() {
                                            Token::RBrace => {
                                                self.advance();
                                                break;
                                            }
                                            Token::Bins
                                            | Token::IllegalBins
                                            | Token::IgnoreBins => {
                                                let bin_type = match self.peek() {
                                                    Token::IllegalBins => BinType::Illegal,
                                                    Token::IgnoreBins => BinType::Ignore,
                                                    _ => BinType::Normal,
                                                };
                                                self.advance();
                                                // Wildcard modifier opsional sebelum nama bin
                                                if let Token::Ident(s) = self.peek() {
                                                    if *s == Symbol::intern("wildcard") {
                                                        self.advance();
                                                    }
                                                }
                                                // `none`/`some` are lexed as Token::None/Some_
                                                // keywords but are valid bin names in covergroups.
                                                let bin_name = match self.peek() {
                                                    Token::None => {
                                                        self.advance();
                                                        Symbol::intern("none")
                                                    }
                                                    Token::Some_ => {
                                                        self.advance();
                                                        Symbol::intern("some")
                                                    }
                                                    _ => self.expect_ident()?,
                                                };
                                                // Opsional: `[N]` atau `[]` setelah nama (array bins)
                                                if self.peek() == &Token::LBrack {
                                                    self.advance();
                                                    if self.peek() != &Token::RBrack {
                                                        let _ = self.parse_expr(0);
                                                    }
                                                    let _ = self.expect(Token::RBrack);
                                                }
                                                self.expect(Token::BlockingAssign)?;
                                                let mut range_list: Vec<BinRange> = Vec::new();
                                                let mut transitions: Vec<Vec<Expr>> = Vec::new();
                                                // Cek apakah rhs adalah binsof(...) atau {range_list}
                                                let is_binsof = if let Token::Ident(s) = self.peek()
                                                {
                                                    s.as_str() == "binsof"
                                                        || s.as_str() == "default"
                                                } else {
                                                    false
                                                };
                                                if is_binsof {
                                                    // binsof(coverpoint) intersect {values} — skip sampai ';'
                                                    // Implementasi nyata: konsumsi expression binsof
                                                    let mut depth = 0i32;
                                                    loop {
                                                        match self.peek() {
                                                            Token::Semi if depth == 0 => break,
                                                            Token::LParen | Token::LBrace => {
                                                                depth += 1;
                                                                self.advance();
                                                            }
                                                            Token::RParen | Token::RBrace
                                                                if depth > 0 =>
                                                            {
                                                                depth -= 1;
                                                                self.advance();
                                                            }
                                                            Token::RBrace if depth == 0 => break,
                                                            Token::Eof => break,
                                                            _ => {
                                                                self.advance();
                                                            }
                                                        }
                                                    }
                                                } else if self.peek() == &Token::LParen {
                                                    // Transition bin (VERIF-31). Setiap `(...)` = SATU
                                                    // sekuens transisi; beberapa sekuens dipisah koma
                                                    // di luar paren: `(a => b)`, `(0 => 1), (1 => 0)`.
                                                    // Di dalam sekuens, `=>` memisahkan level dan
                                                    // koma memisahkan nilai/range dalam satu level:
                                                    //   `(a => b => c)`  `(a, b => c, d)`  `([lo:hi] => v)`.
                                                    // Representasi: transitions[i] = satu Vec<Expr>
                                                    // level ter-flatten (panjang 2 = `prev => curr`).
                                                    // Jika paren TANPA `=>` → value-list; tiap nilai
                                                    // masuk range_list (bukan transisi).
                                                    self.advance(); // '('
                                                    loop {
                                                        // Level pertama.
                                                        let mut level1 = Vec::new();
                                                        loop {
                                                            level1
                                                                .push(self.parse_bin_level_expr()?);
                                                            if self.peek() == &Token::Comma {
                                                                self.advance();
                                                            } else {
                                                                break;
                                                            }
                                                        }
                                                        if self.peek() == &Token::FatArrow {
                                                            // Transition: kumpulkan level tersisa.
                                                            let mut seq = level1;
                                                            while self.peek() == &Token::FatArrow {
                                                                self.advance(); // '=>'
                                                                loop {
                                                                    seq.push(
                                                                        self.parse_bin_level_expr(
                                                                        )?,
                                                                    );
                                                                    if self.peek() == &Token::Comma
                                                                    {
                                                                        self.advance();
                                                                    } else {
                                                                        break;
                                                                    }
                                                                }
                                                            }
                                                            self.expect(Token::RParen)?;
                                                            transitions.push(seq);
                                                        } else {
                                                            // Value-list: `(0,1,2)` — nilai tunggal.
                                                            self.expect(Token::RParen)?;
                                                            for v in level1 {
                                                                range_list.push(BinRange {
                                                                    low: v,
                                                                    high: None,
                                                                });
                                                            }
                                                        }
                                                        if self.peek() == &Token::Comma
                                                            && self.peek_ahead(1) == &Token::LParen
                                                        {
                                                            self.advance(); // ','
                                                            self.advance(); // '('
                                                            continue;
                                                        }
                                                        break;
                                                    }
                                                } else if self.peek() == &Token::LBrace {
                                                    self.advance();
                                                    loop {
                                                        if self.peek() == &Token::RBrace {
                                                            self.advance();
                                                            break;
                                                        }
                                                        if self.peek() == &Token::LBrack {
                                                            self.advance();
                                                            let low = self.parse_expr(0)?;
                                                            self.expect(Token::Colon)?;
                                                            let high = self.parse_expr(0)?;
                                                            self.expect(Token::RBrack)?;
                                                            range_list.push(BinRange {
                                                                low,
                                                                high: Some(high),
                                                            });
                                                        } else {
                                                            let low = self.parse_expr(0)?;
                                                            range_list
                                                                .push(BinRange { low, high: None });
                                                        }
                                                        if self.peek() == &Token::Comma {
                                                            let ahead = self.peek_ahead(1).clone();
                                                            let is_new_port = ahead == Token::Input
                                                                || ahead == Token::Output
                                                                || ahead == Token::Inout
                                                                || (matches!(
                                                                    &ahead,
                                                                    Token::Ident(_)
                                                                ) && matches!(
                                                                    self.peek_ahead(2),
                                                                    Token::Scope
                                                                ));
                                                            if !is_new_port {
                                                                self.advance();
                                                            } else {
                                                                break;
                                                            }
                                                        } else {
                                                            // BUG VERIF-30: range-list `{...}` ditutup `}` — loop sebelumnya
                                                            // break TANPA mengonsumsi `}` sehingga skip_semi tidak melihat
                                                            // `;` dan bins loop break di `}` → hanya bin PERTAMA yang
                                                            // ter-parse (bin kedua+ di-skip diam-diam).
                                                            if self.peek() == &Token::RBrace {
                                                                self.advance();
                                                            }
                                                            break;
                                                        }
                                                    }
                                                }
                                                self.skip_semi();
                                                bins.push(BinDef {
                                                    name: bin_name,
                                                    range_list,
                                                    bin_type,
                                                    transitions,
                                                });
                                            }
                                            // `default` bin — skip sampai ';'
                                            Token::Default => {
                                                self.advance();
                                                self.skip_until_semi_or_end()?;
                                            }
                                            _ => break,
                                        }
                                    }
                                }
                                self.skip_semi();
                                coverpoints.push(CoverpointDef {
                                    name: ident,
                                    expr,
                                    bins,
                                });
                            }
                            Token::Cross => {
                                self.advance();
                                let mut cps = Vec::new();
                                loop {
                                    cps.push(self.expect_ident()?);
                                    if self.peek() == &Token::Comma {
                                        self.advance();
                                    } else {
                                        break;
                                    }
                                }
                                self.skip_semi();
                                crosses.push(CrossDef {
                                    name: ident,
                                    coverpoints: cps,
                                });
                            }
                            _ => {
                                // Konstruk SV tidak dikenal setelah ':' — skip sampai ';'
                                // Contoh: `name : assume ...` atau `name : restrict ...`
                                self.skip_until_semi_or_end()?;
                            }
                        }
                    } else {
                        // Identifier tanpa ':' — kemungkinan `option.weight = 0` atau
                        // `type_option.name = "..."`. Skip sampai ';'.
                        self.skip_until_semi_or_end()?;
                    }
                }
                Token::Option_ => {
                    self.advance();
                    // VERIF-28: `option.weight = N` / `option.per_instance = 0|1` —
                    // opsi instance covergroup. Sebelumnya di-skip sampai ';'.
                    if self.peek() == &Token::Dot {
                        self.advance(); // '.'
                        let opt = self.expect_ident()?;
                        self.expect(Token::BlockingAssign)?;
                        if opt == Symbol::intern("weight") {
                            let w = self.parse_expr(0)?;
                            cg_weight = const_eval_simple(&w).ok().map(|v| v as u64);
                        } else if opt == Symbol::intern("per_instance") {
                            let v = self.parse_expr(0)?;
                            let v = const_eval_simple(&v).ok().map(|x| x as u64);
                            cg_per_instance = v == Some(1);
                        } else {
                            let _ = self.parse_expr(0)?;
                        }
                    } else {
                        self.skip_until_semi_or_end()?;
                    }
                    self.skip_semi();
                }
                _ => {
                    // Token tidak dikenal di body covergroup — skip sampai ';' atau '}'
                    // agar parsing tidak berhenti di sini. Contoh: `type_option.weight = 0;`
                    // atau konstruk SV coverage lain yang belum diimplementasikan.
                    self.advance();
                    // Skip jika ada expression atau assignment setelah token ini
                    let mut depth = 0i32;
                    loop {
                        match self.peek() {
                            Token::Semi if depth == 0 => {
                                self.advance();
                                break;
                            }
                            Token::EndGroup | Token::Eof => break,
                            Token::LBrace | Token::LParen => {
                                depth += 1;
                                self.advance();
                            }
                            Token::RBrace if depth > 0 => {
                                depth -= 1;
                                self.advance();
                            }
                            Token::RBrace if depth == 0 => break,
                            Token::RParen if depth > 0 => {
                                depth -= 1;
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
        Ok(CovergroupDecl {
            name,
            clocking_event,
            coverpoints,
            crosses,
            weight: cg_weight,
            per_instance: cg_per_instance,
            formals,
        })
    }

    pub(crate) fn parse_dpi_import(&mut self) -> Result<DpiImport, SimError> {
        self.advance();
        // `import "DPI-C" [context|pure] function/task` (LRM 1800 §35.5).
        while matches!(self.peek(), Token::Ident(s) if s == "context" || s == "pure") {
            self.advance();
        }
        let is_task = if self.peek() == &Token::Task {
            self.advance();
            true
        } else if self.peek() == &Token::Function {
            self.advance();
            false
        } else {
            return Err(self.err("expected 'function' or 'task' after import \"DPI-C\""));
        };
        if matches!(self.peek(), Token::Auto | Token::Static) {
            self.advance();
        }
        let return_type = if is_task {
            None
        } else if self.peek() == &Token::Void {
            self.advance();
            None
        } else if let Some(dt) = self.try_parse_dpi_type() {
            self.skip_dpi_range();
            Some(Box::new(dt))
        } else {
            None
        };
        let name = self.expect_ident()?;
        let mut args = Vec::new();
        if self.peek() != &Token::LParen {
            // DPI heading boleh tanpa port list: `export "DPI-C" function foo;`
            self.skip_semi();
            return Ok(DpiImport {
                name,
                return_type,
                args,
                is_task,
            });
        }
        self.expect(Token::LParen)?;
        if self.peek() != &Token::RParen {
            loop {
                let direction = if self.peek() == &Token::Input {
                    self.advance();
                    PortDirection::Input
                } else if self.peek() == &Token::Output {
                    self.advance();
                    PortDirection::Output
                } else if self.peek() == &Token::Inout {
                    self.advance();
                    PortDirection::Inout
                } else {
                    PortDirection::Input
                };
                let dtype = self.try_parse_dpi_type().unwrap_or(DataType::Logic);
                self.skip_dpi_range();
                // Modifier signed/unsigned sebelum nama: `int unsigned cycle_count`.
                if self.peek() == &Token::Signed || self.peek() == &Token::Unsigned {
                    self.advance();
                }
                let arg_name = self.expect_ident()?;
                // Dynamic array di DPI: `input bit[7:0] msg[]` — `[]` setelah
                // nama port (unpacked dynamic dim). Skip agar `,`/`)` berikut
                // ditemukan; multi-dim unpacked `[N]` juga di-skip.
                while self.peek() == &Token::LBrack {
                    self.advance();
                    if self.peek() != &Token::RBrack {
                        // size-expr `[N]` — skip sampai `]`
                        let mut depth = 1;
                        while depth > 0 && self.peek() != &Token::Eof {
                            match self.peek() {
                                Token::LBrack => depth += 1,
                                Token::RBrack => {
                                    depth -= 1;
                                    if depth == 0 {
                                        self.advance();
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            self.advance();
                        }
                    } else {
                        self.advance(); // `[]` kosong
                    }
                }
                args.push(DpiArg {
                    direction,
                    dtype,
                    name: arg_name,
                });
                if self.peek() == &Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        self.skip_semi();
        Ok(DpiImport {
            name,
            return_type,
            args,
            is_task,
        })
    }

    pub(crate) fn skip_dpi_range(&mut self) {
        // Skip ALL consecutive packed dimensions: `bit[3:0][7:0][31:0] msg`
        // (multi-dim packed array di DPI-C — pola lazim di dv dpi_pkg).
        while self.peek() == &Token::LBrack {
            self.advance();
            let mut depth = 1;
            while depth > 0 && self.peek() != &Token::Eof {
                match self.peek() {
                    Token::LBrack => depth += 1,
                    Token::RBrack => {
                        depth -= 1;
                        if depth == 0 {
                            self.advance();
                            break;
                        }
                    }
                    _ => {}
                }
                self.advance();
            }
        }
    }

    pub(crate) fn try_parse_dpi_type(&mut self) -> Option<DataType> {
        let dt = match self.peek() {
            Token::Byte => {
                self.advance();
                DataType::Byte
            }
            Token::Shortint => {
                self.advance();
                DataType::Shortint
            }
            Token::Int => {
                self.advance();
                DataType::Int
            }
            Token::Longint => {
                self.advance();
                DataType::Longint
            }
            Token::Integer => {
                self.advance();
                DataType::Integer
            }
            Token::Real => {
                self.advance();
                DataType::Real
            }
            Token::RealTime => {
                self.advance();
                DataType::Realtime
            }
            Token::Bit => {
                self.advance();
                DataType::Bit
            }
            Token::Logic => {
                self.advance();
                DataType::Logic
            }
            Token::String => {
                self.advance();
                DataType::String
            }
            Token::Ident(s) if s == "chandle" => {
                self.advance();
                DataType::Longint
            }
            _ => return None,
        };
        // Modifier signed/unsigned (C-style): `int unsigned x`, `bit signed y`
        // — umum di header DPI OpenTitan (`otbn_model_dpi.svh`).
        if matches!(self.peek(), Token::Signed | Token::Unsigned) {
            self.advance();
        }
        Some(dt)
    }
}
