//! `mfmt` — Formatter Verilog/SystemVerilog.
//!
//! Lexer-based: token di-lex ulang, di-render dengan indentasi + spasi normal.
//! Mode: stdout (default), `--inplace` (tulis balik), `--check` (report saja).

use mivon_core::error::SimError;
use mivon_parser::lexer::{Lexer, Token};

/// Opsi mfmt.
pub struct FmtArgs<'a> {
    pub files: &'a [String],
    pub inplace: bool,
    pub check: bool,
    pub indent: usize,
}

/// Jalankan mfmt.
pub fn run(args: &FmtArgs) -> Result<(), SimError> {
    let mut changed_any = false;
    for f in args.files {
        let src = std::fs::read_to_string(f).map_err(|e| {
            SimError::with_diag(
                mivon_core::diagnostics::DiagCode::IoError,
                format!("{}: {}", f, e),
            )
        })?;
        let formatted = format_source(&src, args.indent);

        if args.check {
            if formatted.trim() != src.trim() {
                println!("  ! {} — perlu diformat", f);
                changed_any = true;
            } else {
                println!("  ✓ {} — sudah rapi", f);
            }
        } else if args.inplace {
            if formatted != src {
                std::fs::write(f, &formatted).map_err(|e| {
                    SimError::with_diag(
                        mivon_core::diagnostics::DiagCode::IoError,
                        format!("{}: {}", f, e),
                    )
                })?;
                println!("  formatted {}", f);
                changed_any = true;
            } else {
                println!("  ✓ {} — tidak berubah", f);
            }
        } else {
            print!("{}", formatted);
        }
    }

    if args.check && changed_any {
        return Err(SimError::with_diag(
            mivon_core::diagnostics::DiagCode::InvalidSyntax,
            "mfmt --check: ada file yang perlu diformat",
        ));
    }
    Ok(())
}

/// Format source string → string terformat.
pub fn format_source(src: &str, indent_width: usize) -> String {
    let mut lexer = Lexer::new(src);
    let mut tokens: Vec<(Token, usize, usize)> = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if matches!(tok, Token::Eof) {
            break;
        }
        if matches!(tok, Token::Error(_)) {
            continue;
        }
        tokens.push((tok, line, col));
    }

    // ── Kelompokkan token per baris sumber (pertahankan struktur baris) ──
    let mut lines: Vec<Vec<(Token, usize)>> = Vec::new();
    let mut cur: Vec<(Token, usize)> = Vec::new();
    let mut cur_line = 0usize;
    for (tok, line, col) in tokens {
        if cur_line == 0 {
            cur_line = line;
        }
        if line != cur_line {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            cur_line = line;
        }
        cur.push((tok, col));
    }
    if !cur.is_empty() {
        lines.push(cur);
    }

    // ── Render per baris dengan indentasi dinamis ──
    let mut out = String::new();
    let mut base = 0usize;
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        let first = &line[0].0;
        let last = &line[line.len() - 1].0;

        // Dedent bila baris dibuka keyword penutup
        if is_dedent_kw(first) {
            base = base.saturating_sub(1);
        }

        // Indentasi ekstra saat masih di dalam paren belum tertutup
        let paren_extra = paren_depth_at_line_start(src, line[0].1);

        let text = render_line(line);
        if text.trim().is_empty() {
            continue;
        }
        let indent = base + paren_extra;
        out.push_str(&" ".repeat(indent * indent_width));
        out.push_str(&text);
        out.push('\n');

        // Indent naik bila baris dibuka/ditutup konstruksi blok
        if is_indent_kw(first) || is_open_begin_kw(last) {
            base += 1;
        }
    }

    out
}

/// Keyword pembuka scope di awal baris → indent naik.
fn is_indent_kw(tok: &Token) -> bool {
    matches!(
        tok,
        Token::Module
            | Token::Interface
            | Token::Package
            | Token::Class
            | Token::Function
            | Token::Task
            | Token::Program
            | Token::Config
            | Token::Primitive
    )
}

/// Keyword pembuka blok di akhir baris → indent naik.
fn is_open_begin_kw(tok: &Token) -> bool {
    matches!(
        tok,
        Token::Begin
            | Token::Case
            | Token::CaseX
            | Token::CaseZ
            | Token::Fork
            | Token::Generate
            | Token::Do
            | Token::Covergroup
            | Token::Property
            | Token::Sequence
            | Token::Specify
            | Token::Clocking
            | Token::Table
    )
}

/// Keyword penutup di awal baris → indent turun.
fn is_dedent_kw(tok: &Token) -> bool {
    matches!(
        tok,
        Token::Endmodule
            | Token::EndInterface
            | Token::EndPackage
            | Token::EndClass
            | Token::EndFunction
            | Token::EndTask
            | Token::EndProgram
            | Token::EndConfig
            | Token::Endcase
            | Token::EndGenerate
            | Token::End
            | Token::Join
            | Token::JoinAny
            | Token::JoinNone
            | Token::EndGroup
            | Token::EndSpecify
            | Token::EndClocking
            | Token::EndTable
            | Token::EndSequence
            | Token::EndPrimitive
    )
}

/// Hitung kedalaman paren saat baris ini dimulai (dengan menghitung token
/// baris sebelumnya). Implementasi sederhana: skan ulang sampai kolom awal.
fn paren_depth_at_line_start(_src: &str, _col: usize) -> usize {
    // Pendekatan sederhana: dihitung global saat rendering tidak praktis,
    // jadi pakai estimasi 0 — port list multi-baris tidak di-indent ekstra.
    0
}

/// Render satu baris token menjadi teks dengan spasi cerdas.
fn render_line(line: &[(Token, usize)]) -> String {
    let mut s = String::new();
    let mut prev: Option<&Token> = None;

    for (i, (tok, _col)) in line.iter().enumerate() {
        let text = token_text(tok);
        if text.is_empty() {
            continue;
        }
        let next = line.get(i + 1).map(|(t, _)| t);
        let sep = space_between(prev, tok, next, &text);
        if sep && !s.is_empty() && !s.ends_with(' ') {
            s.push(' ');
        }
        s.push_str(&text);
        prev = Some(tok);
    }
    s.trim_end().to_string()
}

/// Tentukan apakah perlu spasi antar token sebelumnya dan sekarang.
fn space_between(prev: Option<&Token>, cur: &Token, next: Option<&Token>, _cur_text: &str) -> bool {
    use Token::*;
    let prev: &Token = match prev {
        Some(p) => p,
        Option::None => return false,
    };

    // Keyword kontrol/tipe diikuti `(`/`[`/`@` — spasi (if (x), reg [3:0],
    // always_ff @(posedge)
    if matches!(
        prev,
        If | While
            | Case
            | CaseX
            | CaseZ
            | Repeat
            | For
            | Function
            | Task
            | Module
            | Interface
            | Always
            | AlwaysFF
            | AlwaysComb
            | AlwaysLatch
            | Initial
            | Wire
            | Reg
            | Logic
            | Int
            | Integer
            | Bit
            | Byte
            | Shortint
            | Longint
            | Parameter
            | LocalParam
            | Const
            | Var
            | Output
            | Input
            | Inout
            | PosEdge
            | NegEdge
            | Wait
            | Fork
            | Unique
            | Priority
            | Rand
            | RandC
            | Assert
            | Assume
            | Cover
            | Covergroup
            | Property
            | Sequence
    ) && matches!(cur, LParen | LBrack | At)
    {
        return true;
    }
    // Bare `'` (Quote) diikuti identifier: tanpa spasi, re-lex menyatu jadi
    // literal ber-base (`'` + `bvalid` → `'b` + `valid`) → tokenisasi berubah
    // → round-trip rusak (temuan fuzzer O3: fmt merusak source).
    // Valid SV tidak punya adjacency Quote→Ident (cast selalu `ident'(x)`),
    // jadi spasi ekstra aman utk semua input valid.
    if matches!(prev, Quote) && matches!(cur, Ident(_)) {
        return true;
    }
    // Tidak ada spasi di sekitar delimiter buka/tutup
    if matches!(
        cur,
        LParen | LBrack | LBrace | Dot | Scope | Comma | Semi | Colon
    ) {
        return false;
    }
    if matches!(
        prev,
        LParen | LBrack | LBrace | Dot | Scope | Comma | Semi | Hash | At | Dollar
    ) {
        return false;
    }
    if matches!(prev, RParen | RBrack | RBrace) {
        // `)` diikuti `(` = panggilan; sisanya beri spasi
        return !matches!(
            cur,
            LParen | Semi | Comma | RParen | RBrack | RBrace | Dot | Colon | Question | LBrack
        );
    }

    // Unary +,-,~,! di depan operand → tanpa spasi sebelum
    if matches!(cur, Plus | Minus | Tilde | Not) {
        let prev_is_op = matches!(
            prev,
            Plus | Minus
                | Tilde
                | Not
                | Star
                | Slash
                | Percent
                | Amp
                | Pipe
                | Caret
                | AmpAmp
                | PipePipe
                | Eq
                | Neq
                | Lt
                | Le
                | Gt
                | Ge
                | AssignOp
                | PlusAssign
                | MinusAssign
                | MulAssign
                | DivAssign
                | BlockingAssign
                | NonBlockingAssign
                | Colon
                | Comma
                | Question
                | Increment
                | Decrement
                | Shl
                | Shr
        );
        let prev_is_open = matches!(prev, LParen | LBrack | LBrace | Hash | At | Return | Wait);
        if prev_is_op || prev_is_open {
            return false;
        }
        return true;
    }

    // `~` / `!` unary → no-space sebelum operand. Amp|Pipe|Caret TIDAK di sini:
    // biner (`a & b`) maupun reduction (`&a`) sama-sama valid, dan spasi
    // dua sisi selalu aman + idempotent. (Sebelumnya Pipe ikut no-space →
    // `a||b` render `a|` — ditemukan fuzzer roundtrip mismatch OpenTitan
    // h8_bry0_32||h8_bry0_16 setelah unicode garbage di-skip jadi Pipe,Pipe.)
    if matches!(prev, Tilde | Not) {
        return false;
    }
    if matches!(cur, Tilde | Not) {
        return false;
    }

    // `$` system task/function: `$display`
    if matches!(prev, Dollar) {
        return false;
    }

    // Operator biner → spasi dua sisi
    if is_bin_op(cur) {
        return true;
    }
    if is_bin_op(prev) {
        return true;
    }

    // `#` — dua kasus: param list `foo #(...)` (spasi setelah nama
    // module/interface/class/ident), atau delay `#5` (tanpa spasi).
    if matches!(prev, Hash) {
        return false;
    }
    if matches!(cur, Hash) {
        let prev_is_name = matches!(prev, Ident(_))
            || matches!(
                prev,
                Module | Interface | Program | Class | Package | Function | Task
            );
        // `forever #5`, `repeat #(...)` — delay setelah keyword timing wajib
        // spasi (tanpa ini `forever#5clk` merge jadi token aneh).
        let prev_is_timing = matches!(prev, Forever | Repeat);
        return prev_is_name || prev_is_timing;
    }
    // `@(posedge ...)` — spasi setelah ident (`cb @(posedge`), tanpa spasi
    // setelah keyword (sudah ditangani rule awal: `always_ff @(`) / di awal.
    if matches!(prev, At) {
        return false;
    }
    if matches!(cur, At) {
        return matches!(prev, Ident(_));
    }
    // `'` cast: `int'(x)`
    if matches!(prev, Quote) || matches!(cur, Quote) {
        return false;
    }
    // `?` di ternary
    if matches!(prev, Question) || matches!(cur, Question) {
        return matches!(cur, Question);
    }

    // Fill literal `'0` `'1` `'x`
    if matches!(prev, FillLit(_)) {
        return matches!(next, Some(&LParen));
    }

    // Angka → ident/angka/`[` tanpa spasi — TAPI angka + keyword wajib spasi
    // (`1'b1 end` → tanpa spasi jadi satu token `1'b1end` = number invalid di
    // base b — input valid jadi tak parseable; bug ditemukan fuzzer O3).
    let cur_is_number = matches!(cur, Number { .. } | RealNum(_));
    let prev_is_number = matches!(prev, Number { .. } | RealNum(_));
    if cur_is_number || prev_is_number {
        if prev_is_number && is_keyword(cur) {
            return true;
        }
        if cur_is_number && is_keyword(prev) {
            return true;
        }
        return false;
    }

    // Keyword diikuti ident: `module foo`, `logic [3:0] bar`
    if matches!(prev, Ident(_)) && matches!(cur, Ident(_)) {
        return true;
    }
    if matches!(prev, Number { .. } | Ident(_)) && matches!(cur, Ident(_)) {
        return true;
    }
    // `pinmu 64'd0` — ident diikuti literal bernomor wajib spasi (kalau tidak
    // lexer number makan ident lanjutan utk literal `...d0pinmux` — ditemukan
    // fuzzer roundtrip mismatch OpenTitan pinmux bind: `64'd0pinmux` satu token).
    if matches!(prev, Ident(_) | RParen | RBrack) && matches!(cur, Number { .. }) {
        return true;
    }

    // Keyword `if`/`while`/`case` diikuti `(` — spasi
    if matches!(
        prev,
        If | While | Case | CaseX | CaseZ | Repeat | For | Function | Task | Module | Interface
    ) && matches!(cur, LParen)
    {
        return true;
    }

    // Nama module ala Yosys/synthesis `module $_DLATCH_P_` ($-prefixed) —
    // `module` + `$` wajib spasi, kalau tidak lexer jadi satu token
    // `module$_DLATCH_P_` → tidak parseable (ditemukan fuzzer golden,
    // OpenTitan map latch primitive).
    if matches!(prev, Module | Interface | Program) && matches!(cur, Dollar | Ident(_)) {
        return true;
    }

    // Default: spasi bila kedua token adalah kata (keyword/ident)
    let prev_is_word = matches!(prev, Ident(_)) || is_keyword(prev);
    let cur_is_word = matches!(cur, Ident(_)) || is_keyword(cur);
    prev_is_word && cur_is_word
}

/// Apakah token adalah operator biner (butuh spasi dua sisi).
fn is_bin_op(tok: &Token) -> bool {
    matches!(
        tok,
        Token::Plus
            | Token::Minus
            | Token::Star
            | Token::Slash
            | Token::Percent
            | Token::Eq
            | Token::Neq
            | Token::Lt
            | Token::Le
            | Token::Gt
            | Token::Ge
            | Token::AmpAmp
            | Token::PipePipe
            | Token::Amp
            | Token::Pipe
            | Token::Caret
            | Token::CaretTilde
            | Token::TildeAmp
            | Token::TildePipe
            | Token::Shl
            | Token::Shr
            | Token::Sshl
            | Token::Sshr
            | Token::StarStar
            | Token::CaseEq
            | Token::CaseNeq
            | Token::WildcardEq
            | Token::WildcardNeq
            | Token::BlockingAssign
            | Token::NonBlockingAssign
            | Token::PlusAssign
            | Token::MinusAssign
            | Token::MulAssign
            | Token::DivAssign
            | Token::ModAssign
            | Token::AndAssign
            | Token::OrAssign
            | Token::XorAssign
            | Token::ShlAssign
            | Token::ShrAssign
            | Token::AssignOp
            | Token::Arrow
            | Token::BiDirArrow
            | Token::StarArrow
    )
}

/// Apakah token termasuk keyword (bukan operator/punctuation)?
fn is_keyword(tok: &Token) -> bool {
    // Token Display untuk keyword menghasilkan teks kata; operator tidak.
    // Gunakan daftar eksplisit via matches pada varian keyword.
    use Token::*;
    matches!(
        tok,
        Module
            | Endmodule
            | Input
            | Output
            | Inout
            | Ref
            | Wire
            | Reg
            | Logic
            | Int
            | Integer
            | Signed
            | Unsigned
            | Wand
            | Wor
            | Tri
            | Tri0
            | Tri1
            | TriAnd
            | TriOr
            | Supply0
            | Supply1
            | Always
            | AlwaysComb
            | AlwaysFF
            | AlwaysLatch
            | Initial
            | Final
            | Assign
            | Begin
            | End
            | If
            | Else
            | Case
            | CaseX
            | CaseZ
            | Endcase
            | For
            | While
            | Do
            | Repeat
            | Forever
            | PosEdge
            | NegEdge
            | Or
            | Param
            | Parameter
            | LocalParam
            | GenVar
            | Generate
            | EndGenerate
            | Function
            | EndFunction
            | Task
            | EndTask
            | Foreach
            | Auto
            | Static
            | Real
            | WReal
            | Time
            | RealTime
            | String
            | Class
            | EndClass
            | Virtual
            | Extends
            | This
            | New
            | Void
            | Break
            | Continue
            | Default
            | Disable
            | Force
            | Release
            | Deassign
            | Return
            | Wait
            | Null
            | None
            | Some_
            | And
            | Xor
            | Nand
            | Nor
            | Buf
            | NotGate
            | Module_
            | Interface
            | EndInterface
            | ModPort
            | Program
            | EndProgram
            | Fork
            | Join
            | JoinAny
            | JoinNone
            | Bit
            | Enum
            | Typedef
            | Byte
            | Shortint
            | Longint
            | Struct
            | Union
            | EndEnum
            | Inside
            | Unique
            | Priority
            | Unique0
            | Rand
            | RandC
            | Constraint
            | Const
            | Var
            | Solve
            | Assert
            | Assume
            | Cover
            | Expect
            | WaitOrder
            | Property
            | Sequence
            | EndSequence
            | Package
            | EndPackage
            | Import
            | Export
            | Mailbox
            | Semaphore
            | Bind
            | Specify
            | EndSpecify
            | SpecParam
            | Clocking
            | EndClocking
            | Config
            | EndConfig
            | Design
            | Liblist
            | Cell
            | Use
            | Instance
            | Covergroup
            | EndGroup
            | Coverpoint
            | Cross
            | Bins
            | IllegalBins
            | IgnoreBins
            | Option_
            | Primitive
            | EndPrimitive
            | Table
            | EndTable
            | Type
    )
}

/// Konversi token → teks.
fn token_text(tok: &Token) -> String {
    match tok {
        Token::Ident(s) => s.as_str().to_string(),
        Token::Number {
            value,
            base,
            width,
            is_signed,
        } => {
            let digits = value.as_str();
            let signed = if *is_signed { "s" } else { "" };
            match base {
                Some(b) => {
                    let bc = match b {
                        2 => 'b',
                        8 => 'o',
                        16 => 'h',
                        10 => 'd',
                        _ => 'd',
                    };
                    match width {
                        Some(w) => format!("{}'{}{}{}", w, signed, bc, digits),
                        None => format!("'{}{}{}", signed, bc, digits),
                    }
                }
                None => digits.to_string(),
            }
        }
        Token::RealNum(s) => s.as_str().to_string(),
        Token::StringLit(s) => {
            // Nilai tersimpan TANPA kutip — render ulang dengan `"..."`.
            // Tanpa ini mfmt merusak string literal (`"mivon"` → `mivon`)
            // dan hasilnya tidak parseable (bug ditemukan fuzzer O3).
            // Lexer (read_string) meng-UNESCAPE saat scan: `\n` → newline,
            // `\t` → tab, `\\` → `\`, `\"` → `"`. Tanpa re-escape di sini,
            // `$display("\n--- X ---\n")` dirender jadi literal newline
            // dalam string → fmt(fmt(s)) != fmt(s) (roundtrip mismatch,
            // bug fuzzer fmt_0000) dan indentasi runtuh di pass kedua.
            // Kebalikan `\` dulu supaya escape hasil decode (mis. `\x41`
            // yang lexer biarkan sebagai backslash+x) tetap utuh.
            // `\r` TIDAK di-escape: lexer `read_string` tidak meng-decode
            // `\r` (unknown escape → `\`+`r`) — escape CR justru membuat
            // fmt(fmt(s)) != fmt(s) (false roundtrip mismatch).
            let mut out = String::with_capacity(s.as_str().len() + 2);
            out.push('"');
            for c in s.as_str().chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    other => out.push(other),
                }
            }
            out.push('"');
            out
        }
        Token::FillLit(v) => format!(
            "'{}",
            match v {
                mivon_ir::LogicVal::Zero => "0",
                mivon_ir::LogicVal::One => "1",
                mivon_ir::LogicVal::X => "x",
                mivon_ir::LogicVal::Z => "z",
            }
        ),
        Token::LParen => "(".into(),
        Token::RParen => ")".into(),
        Token::LBrace => "{".into(),
        Token::RBrace => "}".into(),
        Token::LBrack => "[".into(),
        Token::RBrack => "]".into(),
        Token::Semi => ";".into(),
        Token::Comma => ",".into(),
        Token::Colon => ":".into(),
        Token::Scope => "::".into(),
        Token::Dot => ".".into(),
        Token::Hash => "#".into(),
        Token::At => "@".into(),
        Token::Dollar => "$".into(),
        Token::Question => "?".into(),
        Token::Plus => "+".into(),
        Token::Minus => "-".into(),
        Token::Star => "*".into(),
        Token::Slash => "/".into(),
        Token::Percent => "%".into(),
        Token::Eq => "==".into(),
        Token::Neq => "!=".into(),
        // Lexer memetakan `===` literal ke Token::Equiv (BUKAN CaseEq yang
        // untuk `==?`) — tanpa arm ini mfmt render `Equiv` (Debug) merusak
        // `count === count` → `countEquivcount` (ditemukan fuzzer, golden).
        Token::Equiv => "===".into(),
        Token::CaseEq => "===".into(),
        Token::CaseNeq => "!==".into(),
        Token::WildcardEq => "==*".into(),
        Token::WildcardNeq => "!=*".into(),
        Token::Lt => "<".into(),
        Token::Le => "<=".into(),
        Token::Gt => ">".into(),
        Token::Ge => ">=".into(),
        Token::Tilde => "~".into(),
        Token::Not => "!".into(),
        Token::Amp => "&".into(),
        Token::Pipe => "|".into(),
        Token::Caret => "^".into(),
        Token::TildeAmp => "~&".into(),
        Token::TildePipe => "~|".into(),
        Token::CaretTilde => "^~".into(),
        Token::AmpAmp => "&&".into(),
        Token::PipePipe => "||".into(),
        Token::Shl => "<<".into(),
        Token::Shr => ">>".into(),
        Token::Sshl => "<<<".into(),
        Token::Sshr => ">>>".into(),
        Token::PlusColon => "+:".into(),
        Token::MinusColon => "-:".into(),
        Token::StarStar => "**".into(),
        Token::Increment => "++".into(),
        Token::Decrement => "--".into(),
        Token::AssignOp => ":=".into(),
        Token::PlusAssign => "+=".into(),
        Token::MinusAssign => "-=".into(),
        Token::MulAssign => "*=".into(),
        Token::DivAssign => "/=".into(),
        Token::ModAssign => "%=".into(),
        Token::AndAssign => "&=".into(),
        Token::OrAssign => "|=".into(),
        Token::XorAssign => "^=".into(),
        Token::ShlAssign => "<<=".into(),
        Token::ShrAssign => ">>=".into(),
        Token::BlockingAssign => "=".into(),
        Token::NonBlockingAssign => "<=".into(),
        Token::Quote => "'".into(),
        Token::Arrow => "->".into(),
        Token::BiDirArrow => "<->".into(),
        Token::StarArrow => "*>".into(),
        _ => format!("{}", tok),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi temuan fuzzer (O3 round-trip): input `'''  bvalid <= ...`
    /// (garbage `'''` + spasi sebelum ident) HARUS diformat tetap ber-spasi —
    /// tanpa spasi, re-lex berubah (`'`+`bvalid` → `'b`+`valid`) → output
    /// tidak parseable → fmt merusak source.
    #[test]
    fn quote_ident_space_preserved() {
        let src = "module top;\n  logic bvalid;\n  initial begin\n    '''  bvalid <= 1;\n  end\nendmodule\n";
        let once = format_source(src, 4);
        // Spasi antara `'''` dan bvalid harus ada di output.
        assert!(
            once.contains("'''  bvalid") || once.contains("''' bvalid"),
            "output harus mempertahankan spasi setelah kutip: {once}"
        );
        // Round-trip: fmt(fmt(s)) == fmt(s).
        let twice = format_source(&once, 4);
        assert_eq!(once, twice, "round-trip harus idempoten");
    }

    /// Regresi temuan fuzzer (O3 round-trip): lexer `read_string` meng-UNESCAPE
    /// `\n`/`\t`/`\\`/`\"` saat scan, jadi saat render fmt wajib RE-escape —
    /// tanpa itu `$display("\n--- X ---\n")` keluar sbg literal newline →
    /// `fmt(fmt(s)) != fmt(s)` + indentasi runtuh di pass kedua.
    #[test]
    fn string_escape_roundtrip() {
        let src = "module t;\n  initial begin\n    $display(\"\\n--- X ---\\t\\\\ \\\"\");\n  end\nendmodule\n";
        let once = format_source(src, 4);
        assert!(
            once.contains("\\n--- X ---"),
            "escape \\n harus bertahan di output: {once}"
        );
        assert!(
            once.contains("\\\\"),
            "escape backslash harus bertahan di output: {once}"
        );
        // Tidak boleh ada newline literal di dalam string (tanda kutip
        // kemudian baris baru) — itu tanda re-escape hilang.
        assert!(
            !once.contains("\"\n"),
            "string literal tidak boleh pecah baris: {once}"
        );
        let twice = format_source(&once, 4);
        assert_eq!(once, twice, "round-trip string escape harus idempoten");
    }
}
