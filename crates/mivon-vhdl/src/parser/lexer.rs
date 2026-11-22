//! VHDL Lexer — Tokenizes VHDL source code.

use std::fmt;

/// VHDL Token types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    // Keywords
    Entity,
    Architecture,
    Is,
    Begin,
    End,
    Port,
    Generic,
    Map,
    Signal,
    Variable,
    Constant,
    Type,
    Subtype,
    Component,
    Process,
    Function,
    Procedure,
    Package,
    PackageBody,
    Body,
    Library,
    Use,
    If,
    Then,
    Elsif,
    Else,
    Case,
    When,
    Others,
    Loop,
    Exit,
    Next,
    Return,
    Wait,
    While,
    On,
    Until,
    For,
    Generate,
    In,
    Out,
    Inout,
    Buffer,
    Linkage,
    Downto,
    To,
    Range,
    Array,
    Of,
    Record,
    Null,
    Assert,
    Block,
    Access,
    Report,
    Severity,
    Configuration,
    ForAll,
    Attribute,
    Alias,
    File,
    Open,
    All,
    Others_,
    // Operators and punctuation
    SemiColon,      // ;
    Colon,          // :
    Comma,          // ,
    Dot,            // .
    Arrow,          // =>
    Assign,         // <=
    VarAssign,      // :=
    LParen,         // (
    RParen,         // )
    LBracket,       // [
    RBracket,       // ]
    LBrace,         // {
    RBrace,         // {
    // Comparison
    Eq,             // =
    Neq,            // /=
    Lt,             // <
    Le,             // <=
    Gt,             // >
    Ge,             // >=
    // Arithmetic
    Plus,           // +
    Minus,          // -
    Star,           // *
    Slash,          // /
    DoubleStar,     // **
    Ampersand,      // &
    // Logical
    And,
    Or,
    Nand,
    Nor,
    Xor,
    Xnor,
    Not,
    // Literals
    Ident(String),
    StringLit(String),
    CharLit(char),
    IntegerLit(String),
    RealLit(String),
    // Special
    At,             // @
    Hash,           // #
    Tick,           // '
    // Comments/Whitespace handled by lexer
    Eof,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Entity => write!(f, "entity"),
            Token::Architecture => write!(f, "architecture"),
            Token::Is => write!(f, "is"),
            Token::Begin => write!(f, "begin"),
            Token::End => write!(f, "end"),
            Token::Port => write!(f, "port"),
            Token::Generic => write!(f, "generic"),
            Token::Map => write!(f, "map"),
            Token::Signal => write!(f, "signal"),
            Token::Variable => write!(f, "variable"),
            Token::Constant => write!(f, "constant"),
            Token::Type => write!(f, "type"),
            Token::Subtype => write!(f, "subtype"),
            Token::Component => write!(f, "component"),
            Token::Process => write!(f, "process"),
            Token::Function => write!(f, "function"),
            Token::Procedure => write!(f, "procedure"),
            Token::Package => write!(f, "package"),
            Token::PackageBody => write!(f, "package body"),
            Token::Body => write!(f, "body"),
            Token::Library => write!(f, "library"),
            Token::Use => write!(f, "use"),
            Token::If => write!(f, "if"),
            Token::Then => write!(f, "then"),
            Token::Elsif => write!(f, "elsif"),
            Token::Else => write!(f, "else"),
            Token::Case => write!(f, "case"),
            Token::When => write!(f, "when"),
            Token::Others => write!(f, "others"),
            Token::Loop => write!(f, "loop"),
            Token::Exit => write!(f, "exit"),
            Token::Next => write!(f, "next"),
            Token::Return => write!(f, "return"),
            Token::Wait => write!(f, "wait"),
            Token::While => write!(f, "while"),
            Token::On => write!(f, "on"),
            Token::Until => write!(f, "until"),
            Token::For => write!(f, "for"),
            Token::Generate => write!(f, "generate"),
            Token::In => write!(f, "in"),
            Token::Out => write!(f, "out"),
            Token::Inout => write!(f, "inout"),
            Token::Buffer => write!(f, "buffer"),
            Token::Linkage => write!(f, "linkage"),
            Token::Downto => write!(f, "downto"),
            Token::To => write!(f, "to"),
            Token::Range => write!(f, "range"),
            Token::Array => write!(f, "array"),
            Token::Of => write!(f, "of"),
            Token::Record => write!(f, "record"),
            Token::Null => write!(f, "null"),
            Token::Assert => write!(f, "assert"),
            Token::Block => write!(f, "block"),
            Token::Access => write!(f, "access"),
            Token::Report => write!(f, "report"),
            Token::Severity => write!(f, "severity"),
            Token::Configuration => write!(f, "configuration"),
            Token::ForAll => write!(f, "for all"),
            Token::Attribute => write!(f, "attribute"),
            Token::Alias => write!(f, "alias"),
            Token::File => write!(f, "file"),
            Token::Open => write!(f, "open"),
            Token::All => write!(f, "all"),
            Token::Others_ => write!(f, "others"),
            Token::SemiColon => write!(f, ";"),
            Token::Colon => write!(f, ":"),
            Token::Comma => write!(f, ","),
            Token::Dot => write!(f, "."),
            Token::Arrow => write!(f, "=>"),
            Token::Assign => write!(f, "<="),
            Token::VarAssign => write!(f, ":="),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::Eq => write!(f, "="),
            Token::Neq => write!(f, "/="),
            Token::Lt => write!(f, "<"),
            Token::Le => write!(f, "<="),
            Token::Gt => write!(f, ">"),
            Token::Ge => write!(f, ">="),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::DoubleStar => write!(f, "**"),
            Token::Ampersand => write!(f, "&"),
            Token::And => write!(f, "and"),
            Token::Or => write!(f, "or"),
            Token::Nand => write!(f, "nand"),
            Token::Nor => write!(f, "nor"),
            Token::Xor => write!(f, "xor"),
            Token::Xnor => write!(f, "xnor"),
            Token::Not => write!(f, "not"),
            Token::Ident(s) => write!(f, "ident({})", s),
            Token::StringLit(s) => write!(f, "\"{}\"", s),
            Token::CharLit(c) => write!(f, "'{}'", c),
            Token::IntegerLit(s) => write!(f, "int({})", s),
            Token::RealLit(s) => write!(f, "real({})", s),
            Token::At => write!(f, "@"),
            Token::Hash => write!(f, "#"),
            Token::Tick => write!(f, "'"),
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

/// VHDL Lexer.
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

    fn skip_comment(&mut self) {
        // Single line comment: --
        if self.current_char() == Some('-') && self.peek_char(1) == Some('-') {
            self.advance(); // -
            self.advance(); // -
            while let Some(c) = self.current_char() {
                if c == '\n' {
                    break;
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
            } else {
                s.push(c);
                self.advance();
            }
        }
        s
    }

    fn read_char(&mut self) -> char {
        self.advance(); // skip opening '
        let c = self.current_char().unwrap_or(' ');
        self.advance();
        if self.current_char() == Some('\'') {
            self.advance(); // skip closing '
        }
        c
    }

    fn next_token(&mut self) -> SpannedToken {
        self.skip_whitespace();
        self.skip_comment();
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
                    if self.current_char() == Some('=') {
                        self.advance();
                        Token::VarAssign
                    } else {
                        Token::Colon
                    }
                }
                ',' => { self.advance(); Token::Comma }
                '.' => {
                    self.advance();
                    if self.current_char() == Some('.') {
                        self.advance();
                        Token::Range
                    } else {
                        Token::Dot
                    }
                }
                '(' => { self.advance(); Token::LParen }
                ')' => { self.advance(); Token::RParen }
                '[' => { self.advance(); Token::LBracket }
                ']' => { self.advance(); Token::RBracket }
                '<' => {
                    self.advance();
                    if self.current_char() == Some('=') {
                        self.advance();
                        Token::Assign
                    } else if self.current_char() == Some('>') {
                        self.advance();
                        Token::Neq
                    } else {
                        Token::Lt
                    }
                }
                '>' => {
                    self.advance();
                    if self.current_char() == Some('=') {
                        self.advance();
                        Token::Ge
                    } else {
                        Token::Gt
                    }
                }
                '=' => {
                    self.advance();
                    if self.current_char() == Some('>') {
                        self.advance();
                        Token::Arrow
                    } else {
                        Token::Eq
                    }
                }
                '/' => {
                    self.advance();
                    if self.current_char() == Some('=') {
                        self.advance();
                        Token::Neq
                    } else {
                        Token::Slash
                    }
                }
                '+' => { self.advance(); Token::Plus }
                '-' => {
                    self.advance();
                    if self.current_char() == Some('>') {
                        self.advance();
                        Token::Assign
                    } else {
                        Token::Minus
                    }
                }
                '*' => {
                    self.advance();
                    if self.current_char() == Some('*') {
                        self.advance();
                        Token::DoubleStar
                    } else {
                        Token::Star
                    }
                }
                '&' => { self.advance(); Token::Ampersand }
                '@' => { self.advance(); Token::At }
                '#' => { self.advance(); Token::Hash }
                '\'' => { Token::CharLit(self.read_char()) }
                '"' => { Token::StringLit(self.read_string()) }
                c if c.is_ascii_digit() => {
                    let (num, has_dot) = self.read_number();
                    if has_dot {
                        Token::RealLit(num)
                    } else {
                        Token::IntegerLit(num)
                    }
                }
                c if c.is_alphabetic() || c == '_' => {
                    let ident = self.read_identifier().to_lowercase();
                    match ident.as_str() {
                        "entity" => Token::Entity,
                        "architecture" => Token::Architecture,
                        "is" => Token::Is,
                        "begin" => Token::Begin,
                        "end" => Token::End,
                        "port" => Token::Port,
                        "generic" => Token::Generic,
                        "map" => Token::Map,
                        "signal" => Token::Signal,
                        "variable" => Token::Variable,
                        "constant" => Token::Constant,
                        "type" => Token::Type,
                        "subtype" => Token::Subtype,
                        "component" => Token::Component,
                        "process" => Token::Process,
                        "function" => Token::Function,
                        "procedure" => Token::Procedure,
                        "package" => Token::Package,
                        "package_body" | "packagebody" => Token::PackageBody,
                        "body" => Token::Body,
                        "library" => Token::Library,
                        "use" => Token::Use,
                        "if" => Token::If,
                        "then" => Token::Then,
                        "elsif" => Token::Elsif,
                        "else" => Token::Else,
                        "case" => Token::Case,
                        "when" => Token::When,
                        "others" => Token::Others,
                        "loop" => Token::Loop,
                        "exit" => Token::Exit,
                        "next" => Token::Next,
                        "return" => Token::Return,
                        "wait" => Token::Wait,
                        "while" => Token::While,
                        "block" => Token::Block,
                        "access" => Token::Access,
                        "on" => Token::On,
                        "until" => Token::Until,
                        "for" => Token::For,
                        "generate" => Token::Generate,
                        "in" => Token::In,
                        "out" => Token::Out,
                        "inout" => Token::Inout,
                        "buffer" => Token::Buffer,
                        "linkage" => Token::Linkage,
                        "downto" => Token::Downto,
                        "to" => Token::To,
                        "range" => Token::Range,
                        "array" => Token::Array,
                        "of" => Token::Of,
                        "record" => Token::Record,
                        "null" => Token::Null,
                        "assert" => Token::Assert,
                        "report" => Token::Report,
                        "severity" => Token::Severity,
                        "configuration" => Token::Configuration,
                        "forall" => Token::ForAll,
                        "attribute" => Token::Attribute,
                        "alias" => Token::Alias,
                        "file" => Token::File,
                        "open" => Token::Open,
                        "all" => Token::All,
                        "and" => Token::And,
                        "or" => Token::Or,
                        "nand" => Token::Nand,
                        "nor" => Token::Nor,
                        "xor" => Token::Xor,
                        "xnor" => Token::Xnor,
                        "not" => Token::Not,
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
    fn test_simple_entity() {
        let src = "entity test is port (a : in bit); end entity;";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::Entity));
        assert!(matches!(tokens[1].token, Token::Ident(_)));
        assert!(matches!(tokens[2].token, Token::Is));
        assert!(matches!(tokens[3].token, Token::Port));
        assert!(matches!(tokens[4].token, Token::LParen));
    }

    #[test]
    fn test_architecture() {
        let src = "architecture rtl of test is begin end architecture;";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::Architecture));
        assert!(matches!(tokens[1].token, Token::Ident(_)));
        assert!(matches!(tokens[2].token, Token::Of));
    }

    #[test]
    fn test_operators() {
        let src = "a <= b; c := d; e /= f; g <= h; i >= j;";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        eprintln!("Tokens: {:?}", tokens);
        assert!(matches!(tokens[1].token, Token::Assign));
        assert!(matches!(tokens[5].token, Token::VarAssign));
        assert!(matches!(tokens[9].token, Token::Neq));
        assert!(matches!(tokens[13].token, Token::Assign));
        assert!(matches!(tokens[17].token, Token::Ge));
    }

    #[test]
    fn test_comments() {
        let src = "-- this is a comment\nentity test is end;";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize();
        assert!(matches!(tokens[0].token, Token::Entity));
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
        assert!(matches!(&tokens[1].token, Token::RealLit(s) if s == "45.67"));
        assert!(matches!(&tokens[2].token, Token::RealLit(s) if s == "1.2e-3"));
    }
}