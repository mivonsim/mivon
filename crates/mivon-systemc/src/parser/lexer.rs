//! SystemC Lexer — Tokenizes SystemC/C++ source code for SystemC constructs.

use std::fmt;

/// SystemC Token types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    // SystemC Keywords/Macros
    SCModule,
    SCCTor,
    SCMethod,
    SCThread,
    SCThreadH,
    SCClock,
    SCStart,
    SCStop,
    SCMain,
    SCFifo,
    SCEvent,
    SCMutex,
    SCSemaphore,
    SCIn,
    SCOut,
    SCInout,
    SCSignal,
    // SystemC Types
    SCLogic,
    SCUInt,
    SCInt,
    SCBigUInt,
    SCBigInt,
    SCFixed,
    SCUFixed,
    SCBit,
    SCBv,
    SCSensitive,
    SCWait,
    SCReportInfo,
    SCReportWarning,
    SCReportError,
    SCReportFatal,
    SCTrace,
    // C++ Keywords
    Class,
    Struct,
    Template,
    Typename,
    Typedef,
    Using,
    Namespace,
    Public,
    Private,
    Protected,
    Virtual,
    Override,
    Final,
    Const,
    Static,
    Inline,
    Explicit,
    Friend,
    Volatile,
    Mutable,
    Auto,
    // Control flow
    If,
    Else,
    While,
    For,
    Return,
    Break,
    Continue,
    Switch,
    Case,
    Default,
    // Types
    Int,
    Bool,
    Char,
    Void,
    Double,
    Float,
    // Operators and punctuation
    SemiColon,      // ;
    Colon,          // :
    Comma,          // ,
    Dot,            // .
    Arrow,          // ->
    Scope,          // ::
    LParen,         // (
    RParen,         // )
    LBracket,       // [
    RBracket,       // ]
    LBrace,         // {
    RBrace,         // }
    LAngle,         // <
    RAngle,         // >
    Assign,         // =
    Lt,             // <
    Gt,             // >
    Le,             // <=
    Ge,             // >=
    Eq,             // ==
    Neq,            // !=
    Plus,           // +
    Minus,          // -
    Star,           // *
    Slash,          // /
    Mod,            // %
    BitAnd,         // &
    BitOr,          // |
    BitXor,         // ^
    BitNot,         // ~
    LogicalAnd,     // &&
    LogicalOr,      // ||
    LogicalNot,     // !
    ShiftLeft,      // <<
    ShiftRight,     // >>
    // Literals
    Ident(String),
    StringLit(String),
    CharLit(char),
    IntegerLit(String),
    FloatLit(String),
    // Preprocessor
    Hash,           // #
    // Special
    Eof,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::SCModule => write!(f, "SC_MODULE"),
            Token::SCCTor => write!(f, "SC_CTOR"),
            Token::SCMethod => write!(f, "SC_METHOD"),
            Token::SCThread => write!(f, "SC_THREAD"),
            Token::SCThreadH => write!(f, "SC_CTHREAD"),
            Token::SCClock => write!(f, "sc_clock"),
            Token::SCStart => write!(f, "sc_start"),
            Token::SCStop => write!(f, "sc_stop"),
            Token::SCMain => write!(f, "sc_main"),
            Token::SCFifo => write!(f, "sc_fifo"),
            Token::SCEvent => write!(f, "sc_event"),
            Token::SCMutex => write!(f, "sc_mutex"),
            Token::SCSemaphore => write!(f, "sc_semaphore"),
            Token::SCIn => write!(f, "sc_in"),
            Token::SCOut => write!(f, "sc_out"),
            Token::SCInout => write!(f, "sc_inout"),
            Token::SCSignal => write!(f, "sc_signal"),
            Token::SCLogic => write!(f, "sc_logic"),
            Token::SCUInt => write!(f, "sc_uint"),
            Token::SCInt => write!(f, "sc_int"),
            Token::SCBigUInt => write!(f, "sc_biguint"),
            Token::SCBigInt => write!(f, "sc_bigint"),
            Token::SCFixed => write!(f, "sc_fixed"),
            Token::SCUFixed => write!(f, "sc_ufixed"),
            Token::SCBit => write!(f, "sc_bit"),
            Token::SCBv => write!(f, "sc_bv"),
            Token::SCSensitive => write!(f, "sensitive"),
            Token::SCWait => write!(f, "wait"),
            Token::SCReportInfo => write!(f, "SC_REPORT_INFO"),
            Token::SCReportWarning => write!(f, "SC_REPORT_WARNING"),
            Token::SCReportError => write!(f, "SC_REPORT_ERROR"),
            Token::SCReportFatal => write!(f, "SC_REPORT_FATAL"),
            Token::SCTrace => write!(f, "sc_trace"),
            Token::Class => write!(f, "class"),
            Token::Struct => write!(f, "struct"),
            Token::Template => write!(f, "template"),
            Token::Typename => write!(f, "typename"),
            Token::Typedef => write!(f, "typedef"),
            Token::Using => write!(f, "using"),
            Token::Namespace => write!(f, "namespace"),
            Token::Public => write!(f, "public"),
            Token::Private => write!(f, "private"),
            Token::Protected => write!(f, "protected"),
            Token::Virtual => write!(f, "virtual"),
            Token::Override => write!(f, "override"),
            Token::Final => write!(f, "final"),
            Token::Const => write!(f, "const"),
            Token::Static => write!(f, "static"),
            Token::Inline => write!(f, "inline"),
            Token::Explicit => write!(f, "explicit"),
            Token::Friend => write!(f, "friend"),
            Token::Volatile => write!(f, "volatile"),
            Token::Mutable => write!(f, "mutable"),
            Token::Auto => write!(f, "auto"),
            Token::If => write!(f, "if"),
            Token::Else => write!(f, "else"),
            Token::While => write!(f, "while"),
            Token::For => write!(f, "for"),
            Token::Return => write!(f, "return"),
            Token::Break => write!(f, "break"),
            Token::Continue => write!(f, "continue"),
            Token::Switch => write!(f, "switch"),
            Token::Case => write!(f, "case"),
            Token::Default => write!(f, "default"),
            Token::Int => write!(f, "int"),
            Token::Bool => write!(f, "bool"),
            Token::Char => write!(f, "char"),
            Token::Void => write!(f, "void"),
            Token::Double => write!(f, "double"),
            Token::Float => write!(f, "float"),
            Token::SemiColon => write!(f, ";"),
            Token::Colon => write!(f, ":"),
            Token::Comma => write!(f, ","),
            Token::Dot => write!(f, "."),
            Token::Arrow => write!(f, "->"),
            Token::Scope => write!(f, "::"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LAngle => write!(f, "<"),
            Token::RAngle => write!(f, ">"),
            Token::Assign => write!(f, "="),
            Token::Lt => write!(f, "<"),
            Token::Gt => write!(f, ">"),
            Token::Le => write!(f, "<="),
            Token::Ge => write!(f, ">="),
            Token::Eq => write!(f, "=="),
            Token::Neq => write!(f, "!="),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Mod => write!(f, "%"),
            Token::BitAnd => write!(f, "&"),
            Token::BitOr => write!(f, "|"),
            Token::BitXor => write!(f, "^"),
            Token::BitNot => write!(f, "~"),
            Token::LogicalAnd => write!(f, "&&"),
            Token::LogicalOr => write!(f, "||"),
            Token::LogicalNot => write!(f, "!"),
            Token::ShiftLeft => write!(f, "<<"),
            Token::ShiftRight => write!(f, ">>"),
            Token::Ident(s) => write!(f, "ident({})", s),
            Token::StringLit(s) => write!(f, "\"{}\"", s),
            Token::CharLit(c) => write!(f, "'{}'", c),
            Token::IntegerLit(s) => write!(f, "int({})", s),
            Token::FloatLit(s) => write!(f, "float({})", s),
            Token::Hash => write!(f, "#"),
            Token::Eof => write!(f, "EOF"),
        }
    }
}

/// Token with position information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpannedToken {
    pub token: Token,
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

/// SystemC Lexer.
pub struct Lexer {
    input: Vec<char>,
    position: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            position: 0,
            line: 1,
            column: 1,
        }
    }

    fn current_char(&self) -> Option<char> {
        self.input.get(self.position).copied()
    }

    fn peek_char(&self, offset: usize) -> Option<char> {
        self.input.get(self.position + offset).copied()
    }

    fn advance(&mut self) {
        if let Some(c) = self.current_char() {
            if c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            self.position += 1;
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.current_char() {
            if c.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        if self.current_char() == Some('/') && self.peek_char(1) == Some('/') {
            self.advance(); // /
            self.advance(); // /
            while let Some(c) = self.current_char() {
                if c == '\n' {
                    break;
                }
                self.advance();
            }
        }
    }

    fn skip_block_comment(&mut self) {
        if self.current_char() == Some('/') && self.peek_char(1) == Some('*') {
            self.advance(); // /
            self.advance(); // *
            while let Some(c) = self.current_char() {
                if c == '*' && self.peek_char(1) == Some('/') {
                    self.advance(); // *
                    self.advance(); // /
                    break;
                }
                if c == '\n' {
                    self.line += 1;
                    self.column = 1;
                }
                self.advance();
            }
        }
    }

    fn read_identifier(&mut self) -> String {
        let mut ident = String::new();
        while let Some(c) = self.current_char() {
            if c.is_alphanumeric() || c == '_' {
                ident.push(c);
                self.advance();
            } else {
                break;
            }
        }
        ident
    }

    fn read_number(&mut self) -> (String, bool) {
        let mut num = String::new();
        let mut has_dot = false;
        while let Some(c) = self.current_char() {
            if c.is_ascii_digit() {
                num.push(c);
                self.advance();
            } else if c == '.' && !has_dot && self.peek_char(1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
                has_dot = true;
                num.push(c);
                self.advance();
            } else if (c == 'e' || c == 'E') && !num.is_empty() {
                num.push(c);
                self.advance();
                if let Some(sign) = self.current_char() {
                    if sign == '+' || sign == '-' {
                        num.push(sign);
                        self.advance();
                    }
                }
            } else if c == 'f' || c == 'F' {
                num.push(c);
                self.advance();
                has_dot = true;
                break;
            } else {
                break;
            }
        }
        (num, has_dot)
    }

    fn read_string(&mut self) -> String {
        let mut s = String::new();
        self.advance(); // skip opening "
        while let Some(c) = self.current_char() {
            if c == '"' {
                if self.peek_char(1) == Some('"') {
                    // Escaped quote
                    s.push('"');
                    self.advance();
                    self.advance();
                } else {
                    self.advance(); // skip closing "
                    break;
                }
            } else if c == '\\' {
                // Handle escape sequences
                self.advance();
                if let Some(esc) = self.current_char() {
                    match esc {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        '\'' => s.push('\''),
                        _ => s.push(esc),
                    }
                    self.advance();
                }
            } else {
                s.push(c);
                self.advance();
            }
        }
        s
    }

    fn read_char(&mut self) -> char {
        // We're already past the opening '
        let c = self.current_char().unwrap_or(' ');
        self.advance(); // consume the character
        if self.current_char() == Some('\'') {
            self.advance(); // skip closing '
        }
        c
    }

    fn next_token(&mut self) -> SpannedToken {
        self.skip_whitespace();
        self.skip_line_comment();
        self.skip_block_comment();
        self.skip_whitespace();

        let start_line = self.line;
        let start_column = self.column;
        let start_offset = self.position;

        let token = match self.current_char() {
            None => Token::Eof,
            Some(c) => match c {
                ';' => { self.advance(); Token::SemiColon }
                ':' => {
                    self.advance();
                    if self.current_char() == Some(':') {
                        self.advance();
                        Token::Scope
                    } else {
                        Token::Colon
                    }
                }
                ',' => { self.advance(); Token::Comma }
                '.' => {
                    self.advance();
                    if self.current_char() == Some('*') {
                        // .* or .-> - just treat as dot for now
                        Token::Dot
                    } else {
                        Token::Dot
                    }
                }
                '(' => { self.advance(); Token::LParen }
                ')' => { self.advance(); Token::RParen }
                '[' => { self.advance(); Token::LBracket }
                ']' => { self.advance(); Token::RBracket }
                '{' => { self.advance(); Token::LBrace }
                '}' => { self.advance(); Token::RBrace }
                '<' => {
                    self.advance();
                    if self.current_char() == Some('<') {
                        self.advance();
                        Token::ShiftLeft
                    } else if self.current_char() == Some('=') {
                        self.advance();
                        Token::Le
                    } else {
                        Token::LAngle
                    }
                }
                '>' => {
                    self.advance();
                    if self.current_char() == Some('>') {
                        self.advance();
                        Token::ShiftRight
                    } else if self.current_char() == Some('=') {
                        self.advance();
                        Token::Ge
                    } else {
                        Token::RAngle
                    }
                }
                '=' => {
                    self.advance();
                    if self.current_char() == Some('=') {
                        self.advance();
                        Token::Eq
                    } else {
                        Token::Assign
                    }
                }
                '!' => {
                    self.advance();
                    if self.current_char() == Some('=') {
                        self.advance();
                        Token::Neq
                    } else {
                        Token::LogicalNot
                    }
                }
                '+' => { self.advance(); Token::Plus }
                '-' => {
                    self.advance();
                    if self.current_char() == Some('>') {
                        self.advance();
                        Token::Arrow
                    } else {
                        Token::Minus
                    }
                }
                '*' => { self.advance(); Token::Star }
                '/' => { self.advance(); Token::Slash }
                '%' => { self.advance(); Token::Mod }
                '&' => {
                    self.advance();
                    if self.current_char() == Some('&') {
                        self.advance();
                        Token::LogicalAnd
                    } else {
                        Token::BitAnd
                    }
                }
                '|' => {
                    self.advance();
                    if self.current_char() == Some('|') {
                        self.advance();
                        Token::LogicalOr
                    } else {
                        Token::BitOr
                    }
                }
                '^' => { self.advance(); Token::BitXor }
                '~' => { self.advance(); Token::BitNot }
                '#' => { self.advance(); Token::Hash }
                '\'' => { self.advance(); Token::CharLit(self.read_char()) }
                '"' => { Token::StringLit(self.read_string()) }
                c if c.is_ascii_digit() => {
                    let (num, has_dot) = self.read_number();
                    if has_dot {
                        Token::FloatLit(num)
                    } else {
                        Token::IntegerLit(num)
                    }
                }
                c if c.is_alphabetic() || c == '_' => {
                    let ident = self.read_identifier();
                    match ident.as_str() {
                        "SC_MODULE" => Token::SCModule,
                        "SC_CTOR" => Token::SCCTor,
                        "SC_METHOD" => Token::SCMethod,
                        "SC_THREAD" => Token::SCThread,
                        "SC_CTHREAD" => Token::SCThreadH,
                        "sc_clock" => Token::SCClock,
                        "sc_start" => Token::SCStart,
                        "sc_stop" => Token::SCStop,
                        "sc_main" => Token::SCMain,
                        "sc_fifo" => Token::SCFifo,
                        "sc_event" => Token::SCEvent,
                        "sc_mutex" => Token::SCMutex,
                        "sc_semaphore" => Token::SCSemaphore,
                        "sc_in" => Token::SCIn,
                        "sc_out" => Token::SCOut,
                        "sc_inout" => Token::SCInout,
                        "sc_signal" => Token::SCSignal,
                        "sc_logic" => Token::SCLogic,
                        "sc_uint" => Token::SCUInt,
                        "sc_int" => Token::SCInt,
                        "sc_biguint" => Token::SCBigUInt,
                        "sc_bigint" => Token::SCBigInt,
                        "sc_fixed" => Token::SCFixed,
                        "sc_ufixed" => Token::SCUFixed,
                        "sc_bit" => Token::SCBit,
                        "sc_bv" => Token::SCBv,
                        "sensitive" => Token::SCSensitive,
                        "wait" => Token::SCWait,
                        "SC_REPORT_INFO" => Token::SCReportInfo,
                        "SC_REPORT_WARNING" => Token::SCReportWarning,
                        "SC_REPORT_ERROR" => Token::SCReportError,
                        "SC_REPORT_FATAL" => Token::SCReportFatal,
                        "sc_trace" => Token::SCTrace,
                        "class" => Token::Class,
                        "struct" => Token::Struct,
                        "template" => Token::Template,
                        "typename" => Token::Typename,
                        "typedef" => Token::Typedef,
                        "using" => Token::Using,
                        "namespace" => Token::Namespace,
                        "public" => Token::Public,
                        "private" => Token::Private,
                        "protected" => Token::Protected,
                        "virtual" => Token::Virtual,
                        "override" => Token::Override,
                        "final" => Token::Final,
                        "const" => Token::Const,
                        "static" => Token::Static,
                        "inline" => Token::Inline,
                        "explicit" => Token::Explicit,
                        "friend" => Token::Friend,
                        "volatile" => Token::Volatile,
                        "mutable" => Token::Mutable,
                        "auto" => Token::Auto,
                        "if" => Token::If,
                        "else" => Token::Else,
                        "while" => Token::While,
                        "for" => Token::For,
                        "return" => Token::Return,
                        "break" => Token::Break,
                        "continue" => Token::Continue,
                        "switch" => Token::Switch,
                        "case" => Token::Case,
                        "default" => Token::Default,
                        "int" => Token::Int,
                        "bool" => Token::Bool,
                        "char" => Token::Char,
                        "void" => Token::Void,
                        "double" => Token::Double,
                        "float" => Token::Float,
                        _ => Token::Ident(ident),
                    }
                }
                _ => {
                    self.advance();
                    Token::Ident(c.to_string())
                }
            },
        };

        SpannedToken {
            token,
            line: start_line,
            column: start_column,
            offset: start_offset,
        }
    }

    pub fn tokenize(&mut self) -> Vec<SpannedToken> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = matches!(tok.token, Token::Eof);
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sc_module_token() {
        let src = "SC_MODULE(test)";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::SCModule));
        assert!(matches!(tokens[1].token, Token::LParen));
        assert!(matches!(tokens[2].token, Token::Ident(_)));
    }

    #[test]
    fn test_sc_ctor_token() {
        let src = "SC_CTOR(test)";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::SCCTor));
    }

    #[test]
    fn test_port_tokens() {
        let src = "sc_in<bool> a; sc_out<int> b;";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        eprintln!("Tokens: {:?}", tokens);
        assert!(matches!(tokens[0].token, Token::SCIn));
        assert!(matches!(tokens[1].token, Token::LAngle));
        assert!(matches!(tokens[2].token, Token::Bool));
        assert!(matches!(tokens[6].token, Token::SCOut));
        assert!(matches!(tokens[7].token, Token::LAngle));
        assert!(matches!(tokens[8].token, Token::Int));
    }

    #[test]
    fn test_process_tokens() {
        let src = "SC_METHOD(run); SC_THREAD(worker); SC_CTHREAD(tick, clk);";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        eprintln!("Tokens: {:?}", tokens);
        assert!(matches!(tokens[0].token, Token::SCMethod));
        assert!(matches!(tokens[5].token, Token::SCThread));
        assert!(matches!(tokens[10].token, Token::SCThreadH));
    }

    #[test]
    fn test_sc_clock_token() {
        let src = "sc_clock clk;";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::SCClock));
        assert!(matches!(tokens[1].token, Token::Ident(_)));
    }

    #[test]
    fn test_string_literal() {
        let src = "\"hello world\"";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        eprintln!("Tokens: {:?}", tokens);
        assert!(matches!(&tokens[0].token, Token::StringLit(s) if s == "hello world"));
    }

    #[test]
    fn test_char_literal() {
        let src = "'1'";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        eprintln!("Tokens: {:?}", tokens);
        assert!(matches!(tokens[0].token, Token::CharLit('1')));
    }

    #[test]
    fn test_numbers() {
        let src = "123 45.67 1.2e-3";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(&tokens[0].token, Token::IntegerLit(s) if s == "123"));
        assert!(matches!(&tokens[1].token, Token::FloatLit(s) if s == "45.67"));
        assert!(matches!(&tokens[2].token, Token::FloatLit(s) if s == "1.2e-3"));
    }

    #[test]
    fn test_comments() {
        let src = "// comment\nSC_MODULE(test)";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::SCModule));
    }

    #[test]
    fn test_block_comment() {
        let src = "/* comment */ SC_MODULE(test)";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::SCModule));
    }

    #[test]
    fn test_shift_operators() {
        let src = "a << b >> c;";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[1].token, Token::ShiftLeft));
        assert!(matches!(tokens[3].token, Token::ShiftRight));
    }
}