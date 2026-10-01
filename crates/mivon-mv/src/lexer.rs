//! Mivon HDL (.mv) — Lexer.
//! Tokenizer bahasa baru Mivon. 1 file = 1 tanggung jawab: hanya tokenisasi.
//!
//! Token mencakup: identifier, keyword, literal (desimal/bertipe/real/string/
//! fill), operator biner/unary, dan delimiters. Posisi (line, col) dipertahankan
//! untuk diagnostics.

use crate::MvError;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // ── Literal ──
    Ident(String),
    /// Desimal `123`
    Int(i64),
    /// Literal bertipe: (width, base char, digits) — `8'hFF`, `'b101`
    Sized(Option<i64>, char, String),
    /// `1.5`
    Real(f64),
    /// String `"..."`
    Str(String),
    /// Fill `'0` `'1` `'x` `'z`
    Fill(char),
    /// F33: Quote `'` — type cast `T'(expr)`. Hanya muncul bila `'` BUKAN
    /// bagian literal Fill/Sized (`'0`, `'b101`, `8'hFF`).
    Quote,

    // ── Keyword ──
    Module,
    Package,
    Type,
    Struct,
    Packed,
    Enum,
    Union,
    Interface,
    Modport,
    In,
    Out,
    Inout,
    Sig,
    Reg,
    Const,
    Use,
    Seq,
    Comb,
    Always,
    Latch,
    Initial,
    Final,
    Inst,
    If,
    Else,
    Case,
    Casez,
    Casex,
    Priority,
    Unique,
    Unique0,
    Default,
    For,
    /// F38: `do { ... } while (cond)` — loop post-test
    Do,
    While,
    Repeat,
    Forever,
    Wait,
    /// F45: `disable fork` / `disable <label>` — terminasi child fork / blok bernama
    Disable,
    /// F46: `force x = v` / `release x` — fault injection testbench
    Force,
    Release,
    Return,
    Break,
    Continue,
    Sync,
    Func,
    Task,
    Var,
    Assert,
    PosEdge,
    NegEdge,
    Dollar,
    /// F39: fork/join — konkurrensi branch (SV `fork ... join[_any|_none]`)
    Fork,
    Join,
    JoinAny,
    JoinNone,
    /// `foreach (arr[i]) { ... }` — loop elemen array unpacked
    Foreach,

    /// Escape hatch: `@sv { ... }` — teks SystemVerilog mentah yang di-emit
    /// verbatim (body diambil mentah dari source, ISOLASI dari lexer .mv —
    /// karakter apa pun lolos). Untuk konstruk SV yang belum didukung bahasa.
    RawSvh(String),

    // ── Constraint lanjutan (F12) ──
    Inside,
    Dist,
    Solve,
    Before,

    // ── Delimiter ──
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBrack,
    RBrack,
    Comma,
    Semi,
    Colon,
    Dot,
    DotDot,
    Scope, // ::
    Hash,  // #
    At,    // @
    Arrow, // ->
    Question,
    /// `:=` — bobot dist exact (F12)
    Equiv,
    /// `:/` — bobot dist dibagi (F12)
    ColonSlash,

    // ── Operator ──
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Power, // **
    Eq,    // ==
    Neq,   // !=
    CaseEq,
    CaseNeq,
    Lt,
    Le,
    Gt,
    Ge,
    AmpAmp,
    PipePipe,
    Not,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    Sshl,
    Sshr,
    // F36: increment/decrement `++` / `--`
    PlusPlus,
    MinusMinus,
    // F36: compound assignment `+=` `-=` `*=` `/=` `%=` `<<=` `>>=` `&=` `|=` `^=`
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    ShlEq,
    SshrEq,
    AndEq,
    OrEq,
    XorEq,
    /// `=` blocking assignment
    BlockingAssign,
    /// `<=` non-blocking assignment (juga Le di konteks ekspresi)
    NonBlockingAssign,

    Eof,
}

/// Kata kunci .mv (case-sensitive, huruf kecil).
fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "module" => Tok::Module,
        "package" => Tok::Package,
        "type" => Tok::Type,
        "struct" => Tok::Struct,
        "packed" => Tok::Packed,
        "enum" => Tok::Enum,
        "union" => Tok::Union,
        "interface" => Tok::Interface,
        "modport" => Tok::Modport,
        "in" => Tok::In,
        "out" => Tok::Out,
        "inout" => Tok::Inout,
        "sig" => Tok::Sig,
        "reg" => Tok::Reg,
        "const" => Tok::Const,
        "use" => Tok::Use,
        "seq" => Tok::Seq,
        "comb" => Tok::Comb,
        "always" => Tok::Always,
        "latch" => Tok::Latch,
        "initial" => Tok::Initial,
        "final" => Tok::Final,
        "inst" => Tok::Inst,
        "if" => Tok::If,
        "else" => Tok::Else,
        "case" => Tok::Case,
        "casez" => Tok::Casez,
        "casex" => Tok::Casex,
        "priority" => Tok::Priority,
        "unique" => Tok::Unique,
        "unique0" => Tok::Unique0,
        "default" => Tok::Default,
        "for" => Tok::For,
        "do" => Tok::Do,
        "while" => Tok::While,
        "repeat" => Tok::Repeat,
        "forever" => Tok::Forever,
        "wait" => Tok::Wait,
        "disable" => Tok::Disable,
        "force" => Tok::Force,
        "release" => Tok::Release,
        "return" => Tok::Return,
        "break" => Tok::Break,
        "continue" => Tok::Continue,
        "sync" => Tok::Sync,
        "func" => Tok::Func,
        "task" => Tok::Task,
        "var" => Tok::Var,
        "assert" => Tok::Assert,
        "posedge" => Tok::PosEdge,
        "negedge" => Tok::NegEdge,
        "fork" => Tok::Fork,
        "join" => Tok::Join,
        "join_any" => Tok::JoinAny,
        "join_none" => Tok::JoinNone,
        "foreach" => Tok::Foreach,
        "inside" => Tok::Inside,
        "dist" => Tok::Dist,
        "solve" => Tok::Solve,
        "before" => Tok::Before,
        _ => return None,
    })
}

/// Lex source `.mv` → Vec<(token, line, col)>.
pub fn tokenize(src: &str) -> Result<Vec<(Tok, usize, usize)>, MvError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut col = 1usize;

    let err = |line: usize, col: usize, msg: String| MvError::new(line, col, msg);

    while i < chars.len() {
        let c = chars[i];

        // ── Whitespace ──
        if c.is_whitespace() {
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
            continue;
        }

        // ── Komentar ──
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            col += 2;
            loop {
                if i >= chars.len() {
                    return Err(err(line, col, "komentar /* */ tidak ditutup".to_string()));
                }
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
                if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                    i += 2;
                    col += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        let tok_line = line;
        let tok_col = col;

        // ── Identifier / keyword ──
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
                col += 1;
            }
            let text: String = chars[start..i].iter().collect();
            let tok = if text == "$" {
                Tok::Dollar
            } else {
                keyword(&text).unwrap_or(Tok::Ident(text))
            };
            out.push((tok, tok_line, tok_col));
            continue;
        }

        // ── Number / literal bertipe / fill ──
        if c.is_ascii_digit() {
            // Desimal atau real
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
                col += 1;
            }
            // Real: digits '.' digits
            if i < chars.len()
                && chars[i] == '.'
                && i + 1 < chars.len()
                && chars[i + 1].is_ascii_digit()
            {
                i += 1;
                col += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                    col += 1;
                }
                let text: String = chars[start..i].iter().collect();
                match text.parse::<f64>() {
                    Ok(v) => out.push((Tok::Real(v), tok_line, tok_col)),
                    Err(_) => {
                        return Err(err(
                            tok_line,
                            tok_col,
                            format!("real tidak valid: '{}'", text),
                        ))
                    }
                }
                continue;
            }
            // Literal bertipe: `8'hFF` — digits ' base digits
            if i < chars.len() && chars[i] == '\'' {
                let width: i64 = chars[start..i]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .map_err(|_| err(tok_line, tok_col, "width literal tidak valid".into()))?;
                i += 1;
                col += 1;
                if i >= chars.len() {
                    return Err(err(
                        tok_line,
                        tok_col,
                        "literal bertipe tidak lengkap".to_string(),
                    ));
                }
                let base = chars[i].to_ascii_lowercase();
                if !matches!(base, 'b' | 'o' | 'd' | 'h') {
                    return Err(err(
                        tok_line,
                        tok_col,
                        format!("base literal tidak dikenal: '{}'", base),
                    ));
                }
                i += 1;
                col += 1;
                let dstart = i;
                // `?` = wildcard don't-care (casez/casex) — bagian dari digit.
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric()
                        || chars[i] == '_'
                        || chars[i] == 'x'
                        || chars[i] == 'z'
                        || chars[i] == '?')
                {
                    i += 1;
                    col += 1;
                }
                let digits: String = chars[dstart..i].iter().collect();
                out.push((Tok::Sized(Some(width), base, digits), tok_line, tok_col));
                continue;
            }
            let text: String = chars[start..i].iter().collect();
            let v = text.parse::<i64>().map_err(|_| {
                err(
                    tok_line,
                    tok_col,
                    format!("integer tidak valid: '{}'", text),
                )
            })?;
            out.push((Tok::Int(v), tok_line, tok_col));
            continue;
        }

        // ── Fill literal `'0` `'1` `'x` `'z` ──
        if c == '\'' {
            if i + 1 >= chars.len() {
                return Err(err(tok_line, tok_col, "' tanpa isi".to_string()));
            }
            let n = chars[i + 1];
            if matches!(n, '0' | '1' | 'x' | 'z') {
                // Pastikan bukan awal literal bertipe unsized `'b101`
                let is_fill = i + 2 >= chars.len()
                    || !(chars[i + 2].is_ascii_alphanumeric() || chars[i + 2] == '_')
                    || matches!(chars[i + 2], '0' | '1' | 'x' | 'z');
                if is_fill {
                    out.push((Tok::Fill(n), tok_line, tok_col));
                    i += 2;
                    col += 2;
                    continue;
                }
            }
            // Literal bertipe unsized `'b101`
            if i + 1 < chars.len() {
                let base = chars[i + 1].to_ascii_lowercase();
                if matches!(base, 'b' | 'o' | 'd' | 'h') {
                    i += 2;
                    col += 2;
                    let dstart = i;
                    while i < chars.len()
                        && (chars[i].is_ascii_alphanumeric()
                            || chars[i] == '_'
                            || chars[i] == 'x'
                            || chars[i] == 'z'
                            || chars[i] == '?')
                    {
                        i += 1;
                        col += 1;
                    }
                    let digits: String = chars[dstart..i].iter().collect();
                    out.push((Tok::Sized(None, base, digits), tok_line, tok_col));
                    continue;
                }
            }
            // F33: `'` type cast (`T'(expr)`) — bukan literal Fill/Sized.
            // Parser yang memvalidasi konteks (Quote harus setelah tipe).
            out.push((Tok::Quote, tok_line, tok_col));
            i += 1;
            col += 1;
            continue;
        }

        // ── String ──
        if c == '"' {
            i += 1;
            col += 1;
            let mut s = String::new();
            loop {
                if i >= chars.len() {
                    return Err(err(tok_line, tok_col, "string tidak ditutup".to_string()));
                }
                let ch = chars[i];
                if ch == '"' {
                    i += 1;
                    col += 1;
                    break;
                }
                if ch == '\\' && i + 1 < chars.len() {
                    let esc = chars[i + 1];
                    s.push('\\');
                    s.push(esc);
                    i += 2;
                    col += 2;
                    continue;
                }
                s.push(ch);
                i += 1;
                col += 1;
            }
            out.push((Tok::Str(s), tok_line, tok_col));
            continue;
        }

        // ── Escape hatch `@sv { ... }` ──
        // Body SV mentah diambil LANGSUNG dari source (bukan token stream) —
        // isolasi dari lexer .mv sehingga karakter apa pun (`.foo`, `|->`,
        // `"string"`, dsb.) lolos tanpa error. Scan brace-balance string-aware
        // hingga `}` penutup; seluruh isi jadi SATU token RawSvh.
        if c == '@' && is_atsv(&chars, i) {
            i += 3; // '@sv'
            col += 3;
            // lewati spasi sebelum `{`
            while i < chars.len() && chars[i].is_whitespace() {
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
                i += 1;
            }
            if i < chars.len() && chars[i] == '{' {
                i += 1;
                col += 1;
                let start = i;
                let mut depth = 1usize;
                let mut in_str = false;
                while i < chars.len() {
                    let ch = chars[i];
                    // komentar baris `//` — `{`/`}`/`"` di dalamnya TIDAK dihitung
                    if !in_str && ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                            col += 1;
                        }
                        continue;
                    }
                    // komentar blok `/* ... */` — isi diabaikan brace/string
                    if !in_str && ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
                        i += 2;
                        col += 2;
                        loop {
                            if i >= chars.len() {
                                break;
                            }
                            if chars[i] == '\n' {
                                line += 1;
                                col = 1;
                                i += 1;
                                continue;
                            }
                            if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                                i += 2;
                                col += 2;
                                break;
                            }
                            i += 1;
                            col += 1;
                        }
                        continue;
                    }
                    if in_str {
                        if ch == '"' {
                            in_str = false;
                        } else if ch == '\\' && i + 1 < chars.len() {
                            i += 2;
                            col += 2;
                            continue;
                        }
                    } else if ch == '"' {
                        in_str = true;
                    } else if ch == '{' {
                        depth += 1;
                    } else if ch == '}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if ch == '\n' {
                        line += 1;
                        col = 1;
                        i += 1;
                        continue;
                    }
                    i += 1;
                    col += 1;
                }
                let body: String = chars[start..i].iter().collect();
                i += 1; // konsumsi `}`
                out.push((Tok::RawSvh(body), tok_line, tok_col));
                continue;
            }
            // `@sv` tanpa `{` — fallback: perlakukan sebagai `@` + ident biasa
            // (pembatas berikutnya menangani `@`).
        }

        // ── Operator & delimiter ──
        let (tok, adv): (Tok, usize) = match c {
            '{' => (Tok::LBrace, 1),
            '}' => (Tok::RBrace, 1),
            '(' => (Tok::LParen, 1),
            ')' => (Tok::RParen, 1),
            '[' => (Tok::LBrack, 1),
            ']' => (Tok::RBrack, 1),
            ',' => (Tok::Comma, 1),
            ';' => (Tok::Semi, 1),
            '?' => (Tok::Question, 1),
            '@' => (Tok::At, 1),
            '#' => (Tok::Hash, 1),
            '~' => (Tok::Tilde, 1),
            ':' => {
                if peek(&chars, i, ':') {
                    (Tok::Scope, 2)
                } else if peek(&chars, i, '=') {
                    // `:=` — bobot dist exact (F12)
                    (Tok::Equiv, 2)
                } else if peek(&chars, i, '/') {
                    // `:/` — bobot dist dibagi (F12)
                    (Tok::ColonSlash, 2)
                } else {
                    (Tok::Colon, 1)
                }
            }
            '.' => {
                if peek(&chars, i, '.') {
                    (Tok::DotDot, 2)
                } else {
                    (Tok::Dot, 1)
                }
            }
            '-' => {
                if peek(&chars, i, '>') {
                    (Tok::Arrow, 2)
                } else if peek(&chars, i, '-') {
                    // F36: `--` decrement
                    (Tok::MinusMinus, 2)
                } else if peek(&chars, i, '=') {
                    // F36: `-=` compound
                    (Tok::MinusEq, 2)
                } else {
                    (Tok::Minus, 1)
                }
            }
            '+' => {
                if peek(&chars, i, '+') {
                    // F36: `++` increment
                    (Tok::PlusPlus, 2)
                } else if peek(&chars, i, '=') {
                    // F36: `+=` compound
                    (Tok::PlusEq, 2)
                } else {
                    (Tok::Plus, 1)
                }
            }
            '*' => {
                if peek(&chars, i, '*') {
                    (Tok::Power, 2)
                } else if peek(&chars, i, '=') {
                    // F36: `*=` compound
                    (Tok::StarEq, 2)
                } else {
                    (Tok::Star, 1)
                }
            }
            '/' => {
                if peek(&chars, i, '=') {
                    // F36: `/=` compound
                    (Tok::SlashEq, 2)
                } else {
                    (Tok::Slash, 1)
                }
            }
            '%' => {
                if peek(&chars, i, '=') {
                    // F36: `%=` compound
                    (Tok::PercentEq, 2)
                } else {
                    (Tok::Percent, 1)
                }
            }
            '=' => {
                if peek(&chars, i, '=') {
                    if peek2(&chars, i, '=') {
                        (Tok::CaseEq, 3)
                    } else {
                        (Tok::Eq, 2)
                    }
                } else {
                    (Tok::BlockingAssign, 1)
                }
            }
            '!' => {
                if peek(&chars, i, '=') {
                    if peek2(&chars, i, '=') {
                        (Tok::CaseNeq, 3)
                    } else {
                        (Tok::Neq, 2)
                    }
                } else {
                    (Tok::Not, 1)
                }
            }
            '<' => {
                if peek(&chars, i, '<') {
                    // `<<<` (Sshl) / `<<=` (ShlEq F36) / `<<`
                    if peek2(&chars, i, '<') {
                        (Tok::Sshl, 3)
                    } else if peek2(&chars, i, '=') {
                        (Tok::ShlEq, 3)
                    } else {
                        (Tok::Shl, 2)
                    }
                } else if peek(&chars, i, '=') {
                    (Tok::NonBlockingAssign, 2)
                } else {
                    (Tok::Lt, 1)
                }
            }
            '>' => {
                if peek(&chars, i, '>') {
                    // `>>>` (Sshr) / `>>=` (SshrEq F36) / `>>`
                    if peek2(&chars, i, '>') {
                        (Tok::Sshr, 3)
                    } else if peek2(&chars, i, '=') {
                        (Tok::SshrEq, 3)
                    } else {
                        (Tok::Shr, 2)
                    }
                } else if peek(&chars, i, '=') {
                    (Tok::Ge, 2)
                } else {
                    (Tok::Gt, 1)
                }
            }
            '&' => {
                if peek(&chars, i, '&') {
                    (Tok::AmpAmp, 2)
                } else if peek(&chars, i, '=') {
                    // F36: `&=` compound
                    (Tok::AndEq, 2)
                } else {
                    (Tok::Amp, 1)
                }
            }
            '|' => {
                if peek(&chars, i, '|') {
                    (Tok::PipePipe, 2)
                } else if peek(&chars, i, '=') {
                    // F36: `|=` compound
                    (Tok::OrEq, 2)
                } else {
                    (Tok::Pipe, 1)
                }
            }
            '^' => {
                if peek(&chars, i, '=') {
                    // F36: `^=` compound
                    (Tok::XorEq, 2)
                } else {
                    (Tok::Caret, 1)
                }
            }
            _ => {
                return Err(err(
                    tok_line,
                    tok_col,
                    format!("karakter tidak dikenal: '{}'", c),
                ));
            }
        };
        out.push((tok, tok_line, tok_col));
        i += adv;
        col += adv;
    }

    out.push((Tok::Eof, line, col));
    Ok(out)
}

fn peek(chars: &[char], i: usize, c: char) -> bool {
    i + 1 < chars.len() && chars[i + 1] == c
}

/// Deteksi `@sv` — `@` diikuti huruf `sv` (bukan `@(...)` event control).
/// `i` menunjuk ke `'@'`.
fn is_atsv(chars: &[char], i: usize) -> bool {
    i + 2 < chars.len() && chars[i + 1] == 's' && chars[i + 2] == 'v'
}

fn peek2(chars: &[char], i: usize, c: char) -> bool {
    i + 2 < chars.len() && chars[i + 2] == c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src)
            .unwrap()
            .into_iter()
            .map(|(t, _, _)| t)
            .collect()
    }

    #[test]
    fn lex_basic_keywords() {
        let t = toks("module counter { in clk : bit }");
        assert_eq!(
            t,
            vec![
                Tok::Module,
                Tok::Ident("counter".into()),
                Tok::LBrace,
                Tok::In,
                Tok::Ident("clk".into()),
                Tok::Colon,
                Tok::Ident("bit".into()),
                Tok::RBrace,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn lex_numbers() {
        let t = toks("8'hFF 32'd10 'b101 123 1.5 '0 'x");
        assert_eq!(
            t,
            vec![
                Tok::Sized(Some(8), 'h', "FF".into()),
                Tok::Sized(Some(32), 'd', "10".into()),
                Tok::Sized(None, 'b', "101".into()),
                Tok::Int(123),
                Tok::Real(1.5),
                Tok::Fill('0'),
                Tok::Fill('x'),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn lex_operators() {
        let t = toks("a <= b a == b a <= b a && b a <<< b a[3:0] pkg::item #5 @(posedge clk)");
        assert!(t.contains(&Tok::NonBlockingAssign));
        assert!(t.contains(&Tok::Eq));
        assert!(t.contains(&Tok::AmpAmp));
        assert!(t.contains(&Tok::Sshl));
        assert!(t.contains(&Tok::Scope));
        assert!(t.contains(&Tok::Hash));
        assert!(t.contains(&Tok::At));
        assert!(t.contains(&Tok::PosEdge));
        assert!(t.contains(&Tok::Colon));
    }

    #[test]
    fn lex_comments() {
        let t = toks("// baris satu\nmodule x // komentar\n /* blok */ { }");
        assert!(t.contains(&Tok::Module));
        assert!(t.contains(&Tok::LBrace));
        assert!(t.contains(&Tok::RBrace));
    }

    #[test]
    fn lex_arrow_and_dotdot() {
        let t = toks("i in 0..N a -> b");
        assert!(t.contains(&Tok::DotDot));
        assert!(t.contains(&Tok::Arrow));
    }

    #[test]
    fn lex_constraint_advanced() {
        // F12: keyword constraint lanjutan + bobot dist `:=`/`:/`
        let t = toks("x inside {[1:10], 20} y dist {0 := 1, [1:5] :/ 9} solve a before b");
        assert!(t.contains(&Tok::Inside));
        assert!(t.contains(&Tok::Dist));
        assert!(t.contains(&Tok::Solve));
        assert!(t.contains(&Tok::Before));
        assert!(t.contains(&Tok::Equiv));
        assert!(t.contains(&Tok::ColonSlash));
        // `::` tetap scope, `:` tetap colon
        let t2 = toks("pkg::item a : bit c ? x : y");
        assert!(t2.contains(&Tok::Scope));
        assert!(!t2.contains(&Tok::Equiv));
        assert!(!t2.contains(&Tok::ColonSlash));
    }
}
