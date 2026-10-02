//! Fast byte-level lexer — bekerja pada `&[u8]` langsung, bukan `Vec<char>`.
//!
//! Optimasi:
//! - Input byte slice langsung (tanpa konversi Vec<char>)
//! - `Symbol::intern()` untuk identifier → tanpa alokasi String
//! - `u64` word-at-a-time untuk whitespace scanning (SWAR)
//! - Line/col tracking via byte offset
//!
//! Compatibility: menghasilkan Token yang sama dengan legacy lexer.

use mivon_core::intern::Symbol;
use mivon_parser::lexer::Token;

/// Fast byte-level lexer — zero-copy dari input `&[u8]`.
pub struct FastLexer<'a> {
    input: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
    /// `line directive mapping: (cumulative_line_at_directive, directive_value, file_path).
    /// Populated during lexing; consumed by CompileSession → parser via with_file_line_map.
    pub file_line_map: Vec<(usize, usize, String)>,
}

impl<'a> FastLexer<'a> {
    /// Create a new fast lexer.
    pub fn new(input: &'a str, _file_path: &str) -> Self {
        FastLexer {
            input: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            file_line_map: Vec::new(),
        }
    }

    /// Create a new fast lexer from bytes.
    pub fn from_bytes(input: &'a [u8], _file_path: &str) -> Self {
        FastLexer {
            input,
            pos: 0,
            line: 1,
            col: 1,
            file_line_map: Vec::new(),
        }
    }

    /// Get the next token from the input.
    pub fn next_token(&mut self) -> (Token, usize, usize) {
        self.skip_whitespace_and_comments();

        if self.pos >= self.input.len() {
            return (Token::Eof, self.line, self.col);
        }

        let start_line = self.line;
        let start_col = self.col;
        let c = self.input[self.pos];

        // Identifier or keyword (starts with alpha, _, or \ — NOT $).
        // `$` dipisah menjadi Token::Dollar agar `$time`, `$display`, dst
        // di-lex sebagai Dollar + Ident (konsisten dengan lexer legacy).
        // Menggabungkan `$` ke dalam ident membuat `if ($time)` gagal parse
        // karena `$time)` terbaca sebagai satu ident.
        if c.is_ascii_alphabetic() || c == b'_' || c == b'\\' {
            return (self.read_ident_or_keyword(), start_line, start_col);
        }
        if c == b'$' {
            self.skip_byte();
            return (Token::Dollar, start_line, start_col);
        }

        // Number literal
        if c.is_ascii_digit() {
            return (self.read_number(), start_line, start_col);
        }

        // String literal
        if c == b'"' {
            return (self.read_string(), start_line, start_col);
        }

        // Operators and punctuation
        let tok = self.read_operator_or_punct();
        (tok, start_line, start_col)
    }

    // ─── Position Helpers ───

    fn peek(&self) -> u8 {
        if self.pos < self.input.len() {
            self.input[self.pos]
        } else {
            0
        }
    }

    fn peek_next(&self) -> u8 {
        if self.pos + 1 < self.input.len() {
            self.input[self.pos + 1]
        } else {
            0
        }
    }

    fn advance(&mut self) -> u8 {
        let c = self.input[self.pos];
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        c
    }

    fn skip_byte(&mut self) {
        if self.pos < self.input.len() {
            let c = self.input[self.pos];
            self.pos += 1;
            if c == b'\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
    }

    /// Get remaining bytes as a slice.
    #[allow(dead_code)]
    fn remaining(&self) -> &'a [u8] {
        &self.input[self.pos..]
    }

    // ─── Fast Whitespace Skipping (SWAR: u64 word-at-a-time) ───

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            self.skip_whitespace_fast();
            if self.pos >= self.input.len() {
                break;
            }
            // Single-line comment //
            if self.peek() == b'/' && self.peek_next() == b'/' {
                self.skip_single_line_comment();
                continue;
            }
            // Multi-line comment /* */
            if self.peek() == b'/' && self.peek_next() == b'*' {
                self.skip_multi_line_comment();
                continue;
            }
            // `line directive
            if self.peek() == b'`' {
                if self.handle_line_directive() {
                    continue;
                }
                // Backtick lain (makro tak terdefinisi sisa dari preprocessor).
                // JANGAN lewati seluruh baris — itu menghapus statement dan
                // merusak parse berikutnya (mis. `default: `uvm_fatal(...)`
                // → `endcase` jadi "expected expression, found Endcase").
                // Ekspor token Error agar parser melewatinya sama seperti
                // lexer legacy (Token::Error("unexpected character")).
                break;
            }
            break;
        }
    }

    /// Skip whitespace using SIMD-accelerated detection.
    ///
    /// Menggunakan AVX2/SSE4.2 intrinsics jika CPU mendukung,
    /// fallback ke scalar byte-by-byte.
    fn skip_whitespace_fast(&mut self) {
        if self.pos >= self.input.len() {
            return;
        }

        let remaining = &self.input[self.pos..];
        let ws_count = crate::frontend::simd::count_whitespace(remaining);

        // Advance past whitespace bytes
        for _ in 0..ws_count {
            self.skip_byte();
        }
    }

    #[allow(dead_code)]
    fn skip_whitespace_scalar(&mut self) {
        let remaining = &self.input[self.pos..];
        let ws_count = crate::frontend::simd::count_whitespace_scalar(remaining);
        for _ in 0..ws_count {
            self.skip_byte();
        }
    }

    fn skip_single_line_comment(&mut self) {
        while self.pos < self.input.len() {
            if self.input[self.pos] == b'\n' {
                self.skip_byte();
                break;
            }
            self.skip_byte();
        }
    }

    fn skip_multi_line_comment(&mut self) {
        // skip /*
        self.skip_byte(); // '/'
        self.skip_byte(); // '*'
        loop {
            if self.pos >= self.input.len() {
                break;
            }
            if self.peek() == b'*' && self.peek_next() == b'/' {
                self.skip_byte(); // '*'
                self.skip_byte(); // '/'
                break;
            }
            self.skip_byte();
        }
    }

    fn handle_line_directive(&mut self) -> bool {
        let saved = self.pos;
        // Collect line
        let mut line_bytes = Vec::new();
        while self.pos < self.input.len() && self.input[self.pos] != b'\n' {
            line_bytes.push(self.input[self.pos]);
            self.pos += 1;
        }
        let content = String::from_utf8_lossy(&line_bytes);

        // Find `line command
        let trimmed = content.trim_start();
        if !trimmed.starts_with("`line") && !trimmed.starts_with("`LINE") {
            self.pos = saved;
            return false;
        }
        // Parse line number
        let after_cmd = trimmed[5..].trim();
        let (num_str, path) = if let Some(quote_pos) = after_cmd.find('\"') {
            let p = after_cmd[quote_pos + 1..]
                .split('"')
                .next()
                .unwrap_or("")
                .to_string();
            (after_cmd[..quote_pos].trim().to_string(), p)
        } else {
            (after_cmd.trim().to_string(), String::new())
        };

        if let Ok(new_line) = num_str.parse::<usize>() {
            // Track directive for resolve_source_file (same as legacy Lexer).
            // Position is CUMULATIVE line (we do NOT reset self.line) so
            // tokens get globally unique line numbers after base offset.
            if !path.is_empty() {
                self.file_line_map.push((self.line, new_line, path));
            }
            // Consume newline, keep cumulative line counter
            if self.pos < self.input.len() && self.input[self.pos] == b'\n' {
                self.pos += 1;
                self.line += 1;
                self.col = 1;
            }
            return true;
        }

        self.pos = saved;
        false
    }

    // ─── Identifier / Keyword Scanner ───

    fn read_ident_or_keyword(&mut self) -> Token {
        let start = self.pos;
        let c = self.peek();

        // Escaped identifier: \name
        if c == b'\\' {
            self.skip_byte(); // consume backslash
            let id_start = self.pos;
            while self.pos < self.input.len() {
                let c = self.input[self.pos];
                if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                    self.skip_byte(); // consume trailing whitespace
                    break;
                }
                self.skip_byte();
            }
            let end = if self.pos >= self.input.len() {
                self.pos // EOF: include last char
            } else {
                self.pos - 1 // whitespace consumed: exclude it
            };
            let ident = std::str::from_utf8(&self.input[id_start..end]).unwrap_or("");
            return Token::Ident(Symbol::intern(ident));
        }

        // Normal identifier or keyword. `$` SAH sebagai karakter lanjutan di
        // dalam ident (ident mulai dengan alpha/_/\\ di-dispatch terpisah;
        // `$` di awal → Token::Dollar). Konsisten dengan lexer legacy
        // (mivon-parser): `$value$plusargs`, `$fwrite`, `name$suffix` di-lex
        // sebagai Dollar + Ident("value$plusargs"). Tanpa ini, sistem
        // function yang mengandung `$` di tengah (mis. `$value$plusargs`)
        // terpecah jadi Dollar Ident(value) Dollar Ident(plusargs) → parse
        // error berantai yang memotong module (F37/F38 fix opentitan).
        while self.pos < self.input.len() {
            let c = self.input[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
                self.skip_byte();
            } else {
                break;
            }
        }

        let raw = &self.input[start..self.pos];
        let s = std::str::from_utf8(raw).unwrap_or("");

        // Keyword matching via match on raw bytes
        match s {
            "module" => Token::Module,
            "endmodule" => Token::Endmodule,
            "input" => Token::Input,
            "output" => Token::Output,
            "inout" => Token::Inout,
            "ref" => Token::Ref,
            "wire" => Token::Wire,
            "wand" => Token::Wand,
            "wor" => Token::Wor,
            "tri" => Token::Tri,
            "tri0" => Token::Tri0,
            "tri1" => Token::Tri1,
            "triand" => Token::TriAnd,
            "trior" => Token::TriOr,
            "supply0" => Token::Supply0,
            "supply1" => Token::Supply1,
            "reg" => Token::Reg,
            "logic" => Token::Logic,
            "int" => Token::Int,
            "integer" => Token::Integer,
            "signed" => Token::Signed,
            "unsigned" => Token::Unsigned,
            "always" => Token::Always,
            "always_comb" => Token::AlwaysComb,
            "always_ff" => Token::AlwaysFF,
            "always_latch" => Token::AlwaysLatch,
            "initial" => Token::Initial,
            "final" => Token::Final,
            "assign" => Token::Assign,
            "begin" => Token::Begin,
            "end" => Token::End,
            "if" => Token::If,
            "else" => Token::Else,
            "case" => Token::Case,
            "casex" => Token::CaseX,
            "casez" => Token::CaseZ,
            "endcase" => Token::Endcase,
            "for" => Token::For,
            "foreach" => Token::Foreach,
            "while" => Token::While,
            "do" => Token::Do,
            "repeat" => Token::Repeat,
            "forever" => Token::Forever,
            "fork" => Token::Fork,
            "join" => Token::Join,
            "join_any" => Token::JoinAny,
            "join_none" => Token::JoinNone,
            "posedge" => Token::PosEdge,
            "negedge" => Token::NegEdge,
            "or" => Token::Or,
            "param" => Token::Param,
            "parameter" => Token::Parameter,
            "localparam" => Token::LocalParam,
            "genvar" => Token::GenVar,
            "generate" => Token::Generate,
            "endgenerate" => Token::EndGenerate,
            "default" => Token::Default,
            "disable" => Token::Disable,
            "force" => Token::Force,
            "release" => Token::Release,
            "deassign" => Token::Deassign,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "wait" => Token::Wait,
            "null" => Token::Null,
            "string" => Token::String,
            "mailbox" => Token::Mailbox,
            "semaphore" => Token::Semaphore,
            "function" => Token::Function,
            "endfunction" => Token::EndFunction,
            "task" => Token::Task,
            "endtask" => Token::EndTask,
            "automatic" | "auto" => Token::Auto,
            "static" => Token::Static,
            "real" => Token::Real,
            "wreal" => Token::WReal,
            "time" => Token::Time,
            "realtime" => Token::RealTime,
            "none" => Token::None,
            "some" => Token::Some_,
            "and" => Token::And,
            "xor" => Token::Xor,
            "nand" => Token::Nand,
            "nor" => Token::Nor,
            "xnor" => Token::Xnor,
            "buf" => Token::Buf,
            "not" => Token::NotGate,
            "class" => Token::Class,
            "endclass" => Token::EndClass,
            "virtual" => Token::Virtual,
            "extends" => Token::Extends,
            "this" => Token::This,
            "new" => Token::New,
            "void" => Token::Void,
            "return" => Token::Return,
            "enum" => Token::Enum,
            "type" => Token::Type,
            "typedef" => Token::Typedef,
            "bit" => Token::Bit,
            "byte" => Token::Byte,
            "shortint" => Token::Shortint,
            "longint" => Token::Longint,
            "struct" => Token::Struct,
            "union" => Token::Union,
            "endenum" => Token::EndEnum,
            "modport" => Token::ModPort,
            "program" => Token::Program,
            "endprogram" => Token::EndProgram,
            "interface" => Token::Interface,
            "endinterface" => Token::EndInterface,
            "rand" => Token::Rand,
            "randc" => Token::RandC,
            "constraint" => Token::Constraint,
            "soft" => Token::Soft,
            "let" => Token::Let,
            "checker" => Token::Checker,
            "endchecker" => Token::EndChecker,
            "alias" => Token::Alias,
            "nettype" => Token::Nettype,
            "restrict" => Token::Restrict,
            "const" => Token::Const,
            "var" => Token::Var,
            "solve" => Token::Solve,
            "inside" => Token::Inside,
            "unique" => Token::Unique,
            "priority" => Token::Priority,
            "unique0" => Token::Unique0,
            "assert" => Token::Assert,
            "assume" => Token::Assume,
            "cover" => Token::Cover,
            "expect" => Token::Expect,
            "wait_order" => Token::WaitOrder,
            "property" => Token::Property,
            "sequence" => Token::Sequence,
            "endsequence" => Token::EndSequence,
            "package" => Token::Package,
            "endpackage" => Token::EndPackage,
            "import" => Token::Import,
            "export" => Token::Export,
            "bind" => Token::Bind,
            "specify" => Token::Specify,
            "endspecify" => Token::EndSpecify,
            "specparam" => Token::SpecParam,
            "clocking" => Token::Clocking,
            "endclocking" => Token::EndClocking,
            "config" => Token::Config,
            "endconfig" => Token::EndConfig,
            "design" => Token::Design,
            "liblist" => Token::Liblist,
            "cell" => Token::Cell,
            "use" => Token::Use,
            "instance" => Token::Instance,
            "covergroup" => Token::Covergroup,
            "endgroup" => Token::EndGroup,
            "primitive" => Token::Primitive,
            "endprimitive" => Token::EndPrimitive,
            "table" => Token::Table,
            "endtable" => Token::EndTable,
            "coverpoint" => Token::Coverpoint,
            "cross" => Token::Cross,
            "bins" => Token::Bins,
            "illegal_bins" => Token::IllegalBins,
            "ignore_bins" => Token::IgnoreBins,
            "option" => Token::Option_,
            _ => Token::Ident(Symbol::intern(s)),
        }
    }

    // ─── Number Scanner ───

    fn read_number(&mut self) -> Token {
        let start = self.pos;

        // Collect digits and format characters
        while self.pos < self.input.len() {
            let c = self.input[self.pos];
            if c.is_ascii_digit() || c == b'_' || c == b'x' || c == b'z' || c == b'X' || c == b'Z' {
                self.skip_byte();
            } else if c == b'\'' {
                // Check for size cast: 22'(expr)
                if self.peek_next() == b'(' {
                    break;
                }
                // Sized format: 8'b1010. Bangun string literal secara manual
                // (width + ' + base + value) agar whitespace di antara base &
                // value (`7'h 0`) TIDAK ikut terslice — legacy lexer membangun
                // string char-by-char tanpa spasi; FastLexer yang slicing
                // input[start..pos] menyertakan spasi → parse_verilog_number
                // mendapat value " 0" (leading space) → parse gagal → parameter
                // package tidak ter-resolve → E2001.
                let width_part = std::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
                let mut literal = String::with_capacity(16);
                literal.push_str(width_part);
                literal.push('\'');
                self.skip_byte();
                // Base character (bisa 'sh' untuk signed)
                while self.pos < self.input.len() && self.input[self.pos].is_ascii_alphabetic() {
                    literal.push(self.input[self.pos] as char);
                    self.skip_byte();
                }
                // Skip whitespace before value
                while self.pos < self.input.len() && self.input[self.pos].is_ascii_whitespace() {
                    self.skip_byte();
                }
                // Read value part
                while self.pos < self.input.len() {
                    let c = self.input[self.pos];
                    if c.is_ascii_alphanumeric()
                        || c == b'_'
                        || c == b'x'
                        || c == b'z'
                        || c == b'X'
                        || c == b'Z'
                        || c == b'?'
                    {
                        literal.push(c as char);
                        self.skip_byte();
                    } else {
                        break;
                    }
                }
                return self.parse_verilog_number(&literal);
            } else {
                break;
            }
        }

        // Literal real TANPA titik desimal tapi DENGAN eksponen: `1e3`,
        // `10e2`, `2E3`, `1e-2` (LRM 1800 §6.8.3). PRA-FIX hanya
        // `<digits>.<digits>[exp]` yang jadi real → `1e3` ter-tokenize
        // Number("1") + ident("e3") sehingga literal hilang / salah nilai.
        // Syarat: `e`/`E` langsung setelah digit diikuti digit (atau tanda) —
        // menjaga identifier seperti `1e` tetap identifier.
        if (self.peek() == b'e' || self.peek() == b'E') && self.pos > start {
            let exp_start = self.pos;
            self.skip_byte();
            if self.peek() == b'+' || self.peek() == b'-' {
                self.skip_byte();
            }
            let mut exp_digits = 0usize;
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.skip_byte();
                exp_digits += 1;
            }
            if exp_digits > 0 {
                let s = std::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
                return Token::RealNum(Symbol::intern(&s.replace('_', "")));
            }
            // Bukan eksponen (`1e` → identifier `e`) — rollback.
            self.pos = exp_start;
        }

        // Check for real number (decimal point)
        if self.peek() == b'.' {
            self.skip_byte();
            while self.pos < self.input.len() {
                let c = self.input[self.pos];
                if c.is_ascii_digit() || c == b'_' {
                    self.skip_byte();
                } else {
                    break;
                }
            }
            // Check for exponent
            if self.peek() == b'e' || self.peek() == b'E' {
                self.skip_byte();
                if self.peek() == b'+' || self.peek() == b'-' {
                    self.skip_byte();
                }
                while self.pos < self.input.len() {
                    if self.input[self.pos].is_ascii_digit() {
                        self.skip_byte();
                    } else {
                        break;
                    }
                }
            }
            let s = std::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
            return Token::RealNum(Symbol::intern(s));
        }

        // Plain decimal — strip underscore digit separator (F38 fix: `10_000_000`
        // tanpa `'` jatuh ke sini, BUKAN parse_verilog_number; tanpa strip,
        // const-eval gagal → "signal '10_000_000' not found").
        let s = std::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
        Token::Number {
            value: Symbol::intern(&s.replace('_', "")),
            base: None,
            width: None,
            is_signed: false,
        }
    }

    fn parse_verilog_number(&self, s: &str) -> Token {
        let parts: Vec<&str> = s.split('\'').collect();
        if parts.len() != 2 {
            // Plain decimal (tanpa ukuran): `10_000_000` — strip underscore
            // digit separator agar const-eval bisa parse i64 (F38 fix: tanpa
            // ini `int x = 10_000_000;` di interface → "signal not found").
            return Token::Number {
                value: Symbol::intern(&s.replace('_', "")),
                base: None,
                width: None,
                is_signed: false,
            };
        }

        let width_str = parts[0];
        let width = if width_str.is_empty() {
            None
        } else {
            width_str.replace('_', "").parse::<usize>().ok()
        };

        let rest = parts[1];
        if rest.is_empty() {
            return Token::Number {
                value: Symbol::intern(s),
                base: None,
                width: None,
                is_signed: false,
            };
        }

        let mut chars = rest.chars();
        let first_char = chars.next().unwrap();
        let (base_char, is_signed) = match first_char {
            's' | 'S' => (chars.next().unwrap_or('d'), true),
            _ => (first_char, false),
        };
        let value_part: String = chars.collect();

        let base = match base_char {
            'b' | 'B' => Some(2),
            'o' | 'O' => Some(8),
            'd' | 'D' => Some(10),
            'h' | 'H' => Some(16),
            _ => None,
        };

        Token::Number {
            value: Symbol::intern(&value_part.replace('_', "").replace('?', "z")),
            base,
            width,
            is_signed,
        }
    }

    // ─── String Scanner ───

    fn read_string(&mut self) -> Token {
        self.skip_byte(); // consume opening "
        let mut s = String::new();
        while self.pos < self.input.len() {
            let c = self.input[self.pos];
            if c == b'"' {
                self.skip_byte(); // consume closing "
                break;
            }
            if c == b'\\' {
                self.skip_byte();
                if self.pos < self.input.len() {
                    let esc = self.input[self.pos];
                    match esc {
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'\\' => s.push('\\'),
                        b'"' => s.push('"'),
                        other => {
                            s.push('\\');
                            s.push(other as char);
                        }
                    }
                    self.skip_byte();
                }
            } else {
                s.push(self.advance() as char);
            }
        }
        Token::StringLit(Symbol::intern(&s))
    }

    // ─── Operator / Punctuation Scanner ───

    fn read_operator_or_punct(&mut self) -> Token {
        let c = self.advance();
        match c {
            b'+' => {
                if self.peek() == b'+' {
                    self.skip_byte();
                    Token::Increment
                } else if self.peek() == b'=' {
                    self.skip_byte();
                    Token::PlusAssign
                } else if self.peek() == b':' {
                    self.skip_byte();
                    Token::PlusColon
                } else {
                    Token::Plus
                }
            }
            b'-' => {
                if self.peek() == b'-' {
                    self.skip_byte();
                    Token::Decrement
                } else if self.peek() == b'>' {
                    self.skip_byte();
                    Token::Arrow
                } else if self.peek() == b':' {
                    self.skip_byte();
                    Token::MinusColon
                } else if self.peek() == b'=' {
                    // -=
                    self.skip_byte();
                    Token::MinusAssign
                } else {
                    Token::Minus
                }
            }
            b'*' => {
                if self.peek() == b'*' {
                    self.skip_byte();
                    Token::StarStar
                } else if self.peek() == b'>' {
                    self.skip_byte();
                    Token::StarArrow
                } else if self.peek() == b'=' {
                    // *=
                    self.skip_byte();
                    Token::MulAssign
                } else {
                    Token::Star
                }
            }
            b'/' => {
                if self.peek() == b'=' {
                    // /=
                    self.skip_byte();
                    Token::DivAssign
                } else {
                    Token::Slash
                }
            }
            b'%' => {
                if self.peek() == b'=' {
                    // %=
                    self.skip_byte();
                    Token::ModAssign
                } else {
                    Token::Percent
                }
            }
            b'=' => {
                if self.peek() == b'>' {
                    self.skip_byte();
                    Token::FatArrow // => (transition bin SVA / implicasi)
                } else if self.peek() == b'=' && self.peek_next() == b'=' {
                    self.skip_byte();
                    self.skip_byte();
                    Token::Equiv
                } else if self.peek() == b'=' && self.peek_next() == b'?' {
                    self.skip_byte();
                    self.skip_byte();
                    Token::CaseEq
                } else if self.peek() == b'=' {
                    self.skip_byte();
                    Token::Eq
                } else if self.peek() == b'*' {
                    self.skip_byte();
                    Token::WildcardEq
                } else {
                    Token::BlockingAssign
                }
            }
            b'<' => {
                if self.peek() == b'=' {
                    self.skip_byte();
                    Token::NonBlockingAssign
                } else if self.peek() == b'<' {
                    self.skip_byte();
                    if self.peek() == b'<' {
                        self.skip_byte();
                        Token::Sshl
                    } else if self.peek() == b'=' {
                        // <<=
                        self.skip_byte();
                        Token::ShlAssign
                    } else {
                        Token::Shl
                    }
                } else if self.peek() == b'-' {
                    self.skip_byte();
                    if self.peek() == b'>' {
                        self.skip_byte();
                        Token::BiDirArrow
                    } else {
                        Token::Minus
                    }
                } else {
                    Token::Lt
                }
            }
            b'>' => {
                if self.peek() == b'=' {
                    self.skip_byte();
                    Token::Ge
                } else if self.peek() == b'>' {
                    self.skip_byte();
                    if self.peek() == b'>' {
                        self.skip_byte();
                        Token::Sshr
                    } else if self.peek() == b'=' {
                        // >>=
                        self.skip_byte();
                        Token::ShrAssign
                    } else {
                        Token::Shr
                    }
                } else {
                    Token::Gt
                }
            }
            b'!' => {
                if self.peek() == b'=' {
                    self.skip_byte();
                    if self.peek() == b'=' {
                        self.skip_byte();
                        Token::NotEquiv
                    } else if self.peek() == b'?' {
                        self.skip_byte();
                        Token::CaseNeq
                    } else if self.peek() == b'*' {
                        self.skip_byte();
                        Token::WildcardNeq
                    } else {
                        Token::Neq
                    }
                } else {
                    Token::Not
                }
            }
            b'~' => {
                if self.peek() == b'&' {
                    self.skip_byte();
                    Token::TildeAmp
                } else if self.peek() == b'|' {
                    self.skip_byte();
                    Token::TildePipe
                } else if self.peek() == b'^' {
                    self.skip_byte();
                    Token::CaretTilde
                } else {
                    Token::Tilde
                }
            }
            b'&' => {
                if self.peek() == b'&' {
                    self.skip_byte();
                    Token::AmpAmp
                } else if self.peek() == b'=' {
                    // &=
                    self.skip_byte();
                    Token::AndAssign
                } else {
                    Token::Amp
                }
            }
            b'|' => {
                if self.peek() == b'|' {
                    self.skip_byte();
                    Token::PipePipe
                } else if self.peek() == b'=' {
                    // |=
                    self.skip_byte();
                    Token::OrAssign
                } else {
                    Token::Pipe
                }
            }
            b'^' => {
                if self.peek() == b'~' {
                    self.skip_byte();
                    Token::CaretTilde
                } else if self.peek() == b'=' {
                    self.skip_byte();
                    Token::XorAssign
                } else {
                    Token::Caret
                }
            }
            b':' => {
                if self.peek() == b':' {
                    self.skip_byte();
                    Token::Scope
                } else if self.peek() == b'=' {
                    self.skip_byte();
                    Token::AssignOp
                } else {
                    Token::Colon
                }
            }
            b'?' => Token::Question,
            b'(' => Token::LParen,
            b')' => Token::RParen,
            b'{' => Token::LBrace,
            b'}' => Token::RBrace,
            b'[' => Token::LBrack,
            b']' => Token::RBrack,
            b';' => Token::Semi,
            b',' => Token::Comma,
            b'.' => Token::Dot,
            b'#' => {
                // FIX BUG (VERIF-32 sesi): `##` (delay temporal SVA) harus
                // satu token HashHash — paritas dengan lexer legacy
                // (lexer.rs). Tanpa ini `a ##1 b` di-lex sebagai `a # # 1`
                // → parser gagal → module-level assert property DI-SKIP
                // diam-diam di seluruh jalur CompileSession (msim/melab/
                // run_fast filelist/LSP).
                if self.peek() == b'#' {
                    self.skip_byte();
                    Token::HashHash
                } else {
                    Token::Hash
                }
            }
            b'@' => Token::At,
            b'$' => Token::Dollar,
            b'\'' => {
                let next = self.peek();
                match next {
                    b'0' => {
                        self.skip_byte();
                        Token::FillLit(mivon_ir::LogicVal::Zero)
                    }
                    b'1' => {
                        self.skip_byte();
                        Token::FillLit(mivon_ir::LogicVal::One)
                    }
                    b'x' | b'X' => {
                        self.skip_byte();
                        Token::FillLit(mivon_ir::LogicVal::X)
                    }
                    b'z' | b'Z' => {
                        self.skip_byte();
                        Token::FillLit(mivon_ir::LogicVal::Z)
                    }
                    // Unsized literals: 'b, 'o, 'd, 'h
                    b'b' | b'B' | b'o' | b'O' | b'd' | b'D' | b'h' | b'H' => {
                        let base_char = next;
                        self.skip_byte();
                        let base = match base_char {
                            b'b' | b'B' => 2,
                            b'o' | b'O' => 8,
                            b'd' | b'D' => 10,
                            _ => 16,
                        };
                        while self.pos < self.input.len()
                            && self.input[self.pos].is_ascii_whitespace()
                        {
                            self.skip_byte();
                        }
                        let mut value = String::new();
                        while self.pos < self.input.len() {
                            let c = self.input[self.pos];
                            if c.is_ascii_alphanumeric()
                                || c == b'_'
                                || c == b'x'
                                || c == b'z'
                                || c == b'X'
                                || c == b'Z'
                            {
                                value.push(self.advance() as char);
                            } else {
                                break;
                            }
                        }
                        Token::Number {
                            value: Symbol::intern(&value.replace('_', "")),
                            base: Some(base),
                            width: None,
                            is_signed: false,
                        }
                    }
                    // Unsized signed literals: 'sb, 'sd, 'sh, 'so
                    b's' | b'S' => {
                        self.skip_byte();
                        match self.peek() {
                            b'b' | b'B' | b'o' | b'O' | b'd' | b'D' | b'h' | b'H' => {
                                let base_char = self.advance();
                                let base = match base_char {
                                    b'b' | b'B' => 2,
                                    b'o' | b'O' => 8,
                                    b'd' | b'D' => 10,
                                    _ => 16,
                                };
                                while self.pos < self.input.len()
                                    && self.input[self.pos].is_ascii_whitespace()
                                {
                                    self.skip_byte();
                                }
                                let mut value = String::new();
                                while self.pos < self.input.len() {
                                    let c = self.input[self.pos];
                                    if c.is_ascii_alphanumeric()
                                        || c == b'_'
                                        || c == b'x'
                                        || c == b'z'
                                        || c == b'X'
                                        || c == b'Z'
                                    {
                                        value.push(self.advance() as char);
                                    } else {
                                        break;
                                    }
                                }
                                Token::Number {
                                    value: Symbol::intern(&value.replace('_', "")),
                                    base: Some(base),
                                    width: None,
                                    is_signed: true,
                                }
                            }
                            _ => Token::Error("expected base after 's in literal".to_string()),
                        }
                    }
                    _ => Token::Quote,
                }
            }
            _ => Token::Error(format!("unexpected character '{}'", c as char)),
        }
    }
}

/// Tokenize entire input at once (convenience function for testing).
pub fn fast_tokenize(input: &str) -> Vec<(Token, usize, usize)> {
    let mut lexer = FastLexer::new(input, "");
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    tokens
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_lexer_hashhash_temporal_delay() {
        // FIX BUG (VERIF-32 sesi): `##` harus satu token HashHash — paritas
        // dengan lexer legacy. Sebelumnya FastLexer menghasilkan dua Hash →
        // parser gagal parse sequence SVA `a ##1 b` → module-level assert
        // property DI-SKIP diam-diam di seluruh jalur CompileSession.
        let tokens = fast_tokenize("a ##1 b");
        assert_eq!(
            tokens[0].0,
            Token::Ident(Symbol::intern("a")),
            "`a` tetap Ident"
        );
        assert_eq!(tokens[1].0, Token::HashHash, "`##` = SATU token HashHash");
        match &tokens[2].0 {
            Token::Number { value, .. } => {
                assert_eq!(value.as_str(), "1", "delay `1` setelah ##");
            }
            other => panic!("expected Number setelah ##, dapat {:?}", other),
        }
        assert_eq!(tokens[3].0, Token::Ident(Symbol::intern("b")));
        // `#` tunggal (delay) tetap Token::Hash.
        let tokens2 = fast_tokenize("#5");
        assert_eq!(tokens2[0].0, Token::Hash, "delay `#5` tetap Hash + Number");
    }

    #[test]
    fn test_fast_lexer_basic_module() {
        let input = "module test; endmodule";
        let tokens = fast_tokenize(input);
        assert_eq!(
            tokens.len(),
            4,
            "expected 4 tokens: Module, Ident, Semi, Endmodule"
        );
        assert_eq!(tokens[0].0, Token::Module);
        assert_eq!(tokens[1].0, Token::Ident(Symbol::intern("test")));
        assert_eq!(tokens[2].0, Token::Semi);
        assert_eq!(tokens[3].0, Token::Endmodule);
    }

    #[test]
    fn test_fast_lexer_identifiers() {
        let input = "wire clk; assign a = b;";
        let tokens = fast_tokenize(input);
        assert!(tokens.len() >= 4);
        assert_eq!(tokens[0].0, Token::Wire);
        assert_eq!(tokens[1].0, Token::Ident(Symbol::intern("clk")));
    }

    #[test]
    fn test_fast_lexer_numbers() {
        let input = "8'b10101010 42 32'habcd";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens.len(), 3);
        if let Token::Number {
            ref value, base, ..
        } = tokens[0].0
        {
            assert_eq!(value, "10101010");
            assert_eq!(base, Some(2));
        } else {
            panic!("expected Number token");
        }
        if let Token::Number {
            ref value, base, ..
        } = tokens[1].0
        {
            assert_eq!(value, "42");
            assert_eq!(base, None);
        } else {
            panic!("expected Number token");
        }
    }

    #[test]
    fn test_fast_lexer_strings() {
        let input = r#"module "hello" endmodule"#;
        let tokens = fast_tokenize(input);
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].0, Token::Module);
        assert_eq!(tokens[1].0, Token::StringLit(Symbol::intern("hello")));
        assert_eq!(tokens[2].0, Token::Endmodule);
    }

    #[test]
    fn test_fast_lexer_fatarrow_transition() {
        // BUG (VERIF-32): `=>` tidak di-token sebagai FatArrow di FastLexer —
        // menghasilkan BlockingAssign + Gt → covergroup transition bin
        // `(0 => 1)` gagal "expected RParen, found BlockingAssign". Paritas
        // dengan lexer legacy.
        let tokens = fast_tokenize("(0 => 1), (1 => 0)");
        let has_fat = tokens.iter().any(|t| t.0 == Token::FatArrow);
        assert!(
            has_fat,
            "`=>` harus Token::FatArrow (paritas legacy), dapat: {:?}",
            tokens
        );
    }

    #[test]
    fn test_fast_lexer_operators() {
        let input = "a + b - c * d / e";
        let tokens = fast_tokenize(input);
        let ops: Vec<&Token> = tokens.iter().map(|t| &t.0).collect();
        assert_eq!(ops[0], &Token::Ident(Symbol::intern("a")));
        assert_eq!(ops[1], &Token::Plus);
        assert_eq!(ops[2], &Token::Ident(Symbol::intern("b")));
        assert_eq!(ops[3], &Token::Minus);
    }

    #[test]
    fn test_fast_lexer_comparison() {
        let input = "a === b !== c == d != e";
        let tokens = fast_tokenize(input);
        let ops: Vec<&Token> = tokens.iter().map(|t| &t.0).collect();
        assert_eq!(ops[0], &Token::Ident(Symbol::intern("a")));
        assert_eq!(ops[1], &Token::Equiv);
        assert_eq!(ops[3], &Token::NotEquiv);
        assert_eq!(ops[5], &Token::Eq);
        assert_eq!(ops[7], &Token::Neq);
    }

    #[test]
    fn test_fast_lexer_blocking_assign() {
        // Test that '=' is BlockingAssign
        let input = "a = b;";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens[1].0, Token::BlockingAssign);
    }

    #[test]
    fn test_fast_lexer_nonblocking_assign() {
        let input = "a <= b;";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens[1].0, Token::NonBlockingAssign);
    }

    #[test]
    fn test_fast_lexer_comments() {
        let input = "a // comment\nb /* block */ c";
        let tokens = fast_tokenize(input);
        let idents: Vec<&str> = tokens
            .iter()
            .filter_map(|t| match &t.0 {
                Token::Ident(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(idents, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_fast_lexer_line_tracking() {
        let input = "a\nb\nc";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].1, 1); // line 1
        assert_eq!(tokens[1].1, 2); // line 2
        assert_eq!(tokens[2].1, 3); // line 3
    }

    #[test]
    fn test_fast_lexer_edge_triggers() {
        let input = "posedge clk";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens[0].0, Token::PosEdge);
        assert_eq!(tokens[1].0, Token::Ident(Symbol::intern("clk")));
    }

    #[test]
    fn test_fast_lexer_scope_operator() {
        let input = "pkg::item";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].0, Token::Ident(Symbol::intern("pkg")));
        assert_eq!(tokens[1].0, Token::Scope);
        assert_eq!(tokens[2].0, Token::Ident(Symbol::intern("item")));
    }

    #[test]
    fn test_fast_lexer_fill_lit() {
        let input = "'0 '1 'x 'z";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].0, Token::FillLit(mivon_ir::LogicVal::Zero));
        assert_eq!(tokens[1].0, Token::FillLit(mivon_ir::LogicVal::One));
        assert_eq!(tokens[2].0, Token::FillLit(mivon_ir::LogicVal::X));
        assert_eq!(tokens[3].0, Token::FillLit(mivon_ir::LogicVal::Z));
    }

    #[test]
    fn test_fast_lexer_line_directive() {
        let input = "`line 42 \"test.sv\"\nmodule test;";
        let mut lexer = FastLexer::new(input, "");
        let (tok, line, _col) = lexer.next_token();
        assert_eq!(tok, Token::Module);
        // Cumulative line: directive at line 1, content starts at line 2
        assert_eq!(line, 2);
        // file_line_map records (position, directive value, path)
        assert_eq!(lexer.file_line_map.len(), 1);
        assert_eq!(lexer.file_line_map[0], (1, 42, "test.sv".to_string()));
    }

    #[test]
    fn test_fast_lexer_keywords_not_idents() {
        // All keywords should be tokenized as keyword tokens, not Ident
        let kw_input = "module endmodule input output wire reg logic assign always initial if else for while case begin end function task generate class package interface import";
        let tokens = fast_tokenize(kw_input);
        for (i, t) in tokens.iter().enumerate() {
            assert!(
                !matches!(t.0, Token::Ident(_)),
                "keyword at position {} should not be Ident: {}",
                i,
                t.0
            );
        }
    }

    #[test]
    fn test_fast_lexer_unsized_literal() {
        let input = "'b1010 'hFF 'd42 'o77";
        let tokens = fast_tokenize(input);
        assert_eq!(tokens.len(), 4);
        // Each should be a Number token without width
        for (i, t) in tokens.iter().enumerate() {
            match &t.0 {
                Token::Number { width, .. } => assert!(
                    width.is_none(),
                    "unsized literal at {} should have no width",
                    i
                ),
                other => panic!("expected Number at {}, got {}", i, other),
            }
        }
    }

    #[test]
    fn test_fast_lexer_equivalence_with_legacy() {
        // Test with a simple module that both lexers should handle identically
        let input = "module counter(input clk, input rst, output reg [3:0] count);
    always @(posedge clk) begin
        if (rst) count <= 0;
        else count <= count + 1;
    end
endmodule";

        // Use legacy lexer
        let mut legacy = mivon_parser::lexer::Lexer::new(input);
        let mut legacy_tokens = Vec::new();
        loop {
            let (tok, line, col) = legacy.next_token();
            if tok == Token::Eof {
                break;
            }
            legacy_tokens.push((tok, line, col));
        }

        // Use fast lexer
        let mut fast = FastLexer::new(input, "");
        let mut fast_tokens = Vec::new();
        loop {
            let (tok, line, col) = fast.next_token();
            if tok == Token::Eof {
                break;
            }
            fast_tokens.push((tok, line, col));
        }

        // Compare: same number of tokens, same token types
        assert_eq!(
            legacy_tokens.len(),
            fast_tokens.len(),
            "Token count mismatch: legacy={}, fast={}",
            legacy_tokens.len(),
            fast_tokens.len()
        );

        for (i, (lt, ft)) in legacy_tokens.iter().zip(fast_tokens.iter()).enumerate() {
            assert_eq!(
                std::mem::discriminant(&lt.0),
                std::mem::discriminant(&ft.0),
                "Token discriminant mismatch at position {}: legacy={:?}, fast={:?}",
                i,
                lt.0,
                ft.0
            );
        }
    }

    #[test]
    fn test_fast_lexer_compound_assign_ops() {
        // Semua operator compound assignment harus tokenize dengan benar.
        // Kasus edge: <<= (bukan NonBlockingAssign), >>= (bukan Shr), -= (bukan Minus),
        // >>> (Sshr) tetap tidak tertelan oleh >>=.
        let input = "a += b; a -= b; a *= b; a /= b; a %= b; a &= b; a |= b; a ^= b; a <<= b; a >>= b; a <<< b; a >>> b; a <= b;";
        let tokens = fast_tokenize(input);
        let ops: Vec<&Token> = tokens
            .iter()
            .filter(|(t, _, _)| !matches!(t, Token::Ident(_) | Token::Semi))
            .map(|(t, _, _)| t)
            .collect();
        let expected: Vec<Token> = vec![
            Token::PlusAssign,
            Token::MinusAssign,
            Token::MulAssign,
            Token::DivAssign,
            Token::ModAssign,
            Token::AndAssign,
            Token::OrAssign,
            Token::XorAssign,
            Token::ShlAssign,
            Token::ShrAssign,
            Token::Sshl,
            Token::Sshr,
            Token::NonBlockingAssign,
        ];
        assert_eq!(
            ops.len(),
            expected.len(),
            "jumlah operator tidak cocok: {:?}",
            ops
        );
        for (i, exp) in expected.iter().enumerate() {
            assert_eq!(ops[i], exp, "operator posisi {} tidak cocok", i);
        }
    }

    #[test]
    fn test_fast_lexer_compound_assign_equivalence_with_legacy() {
        // Verifikasi legacy lexer dan FastLexer menghasilkan token yang sama
        // untuk seluruh operator compound assignment + operator pembanding
        // non-compound (regresi silang). Kelas bug <<= di branch salah sudah
        // terjadi dua kali — test ini mencegah regresi lintas lexer.
        let input = "a += b; a -= b; a *= b; a /= b; a %= b; a &= b; a |= b; a ^= b; a <<= b; a >>= b; a <<< b; a >>> b; a <= b; a >= b; a === b; a !== b; a == b; a != b; a << b; a >> b; a < b; a > b;";

        let mut legacy = mivon_parser::lexer::Lexer::new(input);
        let mut legacy_tokens = Vec::new();
        loop {
            let (tok, line, col) = legacy.next_token();
            if tok == Token::Eof {
                break;
            }
            legacy_tokens.push((tok, line, col));
        }

        let fast_tokens = fast_tokenize(input);
        assert_eq!(
            legacy_tokens.len(),
            fast_tokens.len(),
            "Token count mismatch: legacy={}, fast={}",
            legacy_tokens.len(),
            fast_tokens.len()
        );
        for (i, (lt, ft)) in legacy_tokens.iter().zip(fast_tokens.iter()).enumerate() {
            assert_eq!(
                std::mem::discriminant(&lt.0),
                std::mem::discriminant(&ft.0),
                "Token discriminant mismatch at position {}: legacy={:?}, fast={:?}",
                i,
                lt.0,
                ft.0
            );
        }
    }
}

#[cfg(test)]
mod tmp_fast_lexer_tests {
    use super::*;
    #[test]
    fn tmp_dump_user_defined_array_tokens() {
        let src = "typedef struct packed { logic [3:0] a; } P;\nmodule tb;\n  P a2 [1:0];\n  P a3 [2];\nendmodule\n";
        for (i, (t, l, c)) in fast_tokenize(src).iter().enumerate() {
            if *l >= 3 {
                eprintln!("TMPTOK {i} line={l} col={c} {t:?}");
            }
        }
    }
}

#[cfg(test)]
mod tmp_pp_parser_tests {
    use super::*;
    #[test]
    fn tmp_dump_decl_after_preprocess() {
        let src = std::fs::read_to_string("/tmp/mvprobe/s1.sv").expect("src");
        let mut pp = mivon_parser::preprocessor::Preprocessor::new();
        let combined = pp
            .preprocess(&src, None)
            .unwrap_or_else(|e| panic!("preprocess: {e}"));
        for (i, line) in combined.lines().enumerate() {
            eprintln!("TMPPPLINE {i}: {line}");
        }
        let toks = fast_tokenize(&combined);
        let mut p = mivon_parser::Parser::new(toks, "s1.sv");
        let d = p.parse_design().expect("parse");
        for m in &d.modules {
            eprintln!("TMPMOD {}", m.name.as_str());
            for dc in &m.decls {
                for v in &dc.names {
                    eprintln!(
                        "TMPD {} dtype={:?} ar={:?} esz={:?}",
                        v.name.as_str(),
                        dc.dtype,
                        v.array_range,
                        v.array_size_expr
                    );
                }
            }
        }
    }
}
