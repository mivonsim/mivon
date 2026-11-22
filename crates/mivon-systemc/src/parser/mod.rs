//! SystemC Parser — Parses SystemC tokens into AST.

mod lexer;
use lexer::{Lexer, SpannedToken, Token};

/// SystemC AST node types.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCNode {
    /// Module declaration (SC_MODULE)
    Module {
        name: String,
        members: Vec<SystemCMember>,
        line: usize,
    },
    /// Constructor (SC_CTOR)
    Ctor {
        name: String,
        body: Vec<SystemCStmt>,
        line: usize,
    },
    /// sc_main entry point
    ScMain {
        params: Vec<String>,
        body: Vec<SystemCStmt>,
        line: usize,
    },
}

/// Module members
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCMember {
    Port(SystemCPort),
    Signal(SystemCSignal),
    Process(SystemCProcess),
    Clock(SystemCClock),
    Channel(SystemCChannel),
    Method(SystemCMethod),
    Variable(SystemCVariable),
    Typedef(SystemCTypedef),
}

/// Port declaration
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCPort {
    pub name: String,
    pub direction: SystemCPortDirection,
    pub type_name: String,
    pub template_args: Vec<String>,
}

/// Port direction
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCPortDirection {
    In,
    Out,
    InOut,
    Signal,
}

/// Signal declaration
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCSignal {
    pub name: String,
    pub type_name: String,
    pub template_args: Vec<String>,
    pub initial_value: Option<String>,
}

/// Process declaration (SC_METHOD, SC_THREAD, SC_CTHREAD)
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCProcess {
    pub kind: SystemCProcessKind,
    pub name: String,
    pub sensitivity: Vec<SystemCSensitivity>,
    pub line: usize,
}

/// Process kind
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCProcessKind {
    Method,
    Thread,
    CThread,
}

/// Clock declaration
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCClock {
    pub name: String,
    pub template_args: Vec<String>,
    pub constructor_args: Vec<String>,
    pub line: usize,
}

/// Channel declaration
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCChannel {
    pub name: String,
    pub kind: SystemCChannelKind,
    pub template_args: Vec<String>,
    pub line: usize,
}

/// Channel kind
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCChannelKind {
    Fifo,
    Event,
    Mutex,
    Semaphore,
}

/// Sensitivity entry
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCSensitivity {
    pub signal: String,
    pub edge: SystemCEdge,
}

/// Edge type
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCEdge {
    Pos,
    Neg,
    None,
}

/// Method declaration
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCMethod {
    pub name: String,
    pub return_type: String,
    pub params: Vec<SystemCParam>,
    pub body: Vec<SystemCStmt>,
    pub is_const: bool,
    pub line: usize,
}

/// Variable declaration
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCVariable {
    pub name: String,
    pub type_name: String,
    pub template_args: Vec<String>,
    pub initial_value: Option<String>,
    pub is_static: bool,
    pub is_const: bool,
    pub line: usize,
}

/// Typedef declaration
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCTypedef {
    pub name: String,
    pub type_def: String,
    pub line: usize,
}

/// Parameter
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCParam {
    pub name: String,
    pub type_name: String,
    pub is_reference: bool,
    pub is_const: bool,
}

/// Statements
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCStmt {
    /// Expression statement
    Expr(String),
    /// Variable declaration
    VarDecl(SystemCVariable),
    /// If statement
    If {
        condition: String,
        then_branch: Vec<SystemCStmt>,
        else_branch: Vec<SystemCStmt>,
        line: usize,
    },
    /// While loop
    While {
        condition: String,
        body: Vec<SystemCStmt>,
        line: usize,
    },
    /// For loop
    For {
        init: Option<String>,
        condition: Option<String>,
        step: Option<String>,
        body: Vec<SystemCStmt>,
        line: usize,
    },
    /// Return statement
    Return {
        value: Option<String>,
        line: usize,
    },
    /// Break statement
    Break { line: usize },
    /// Continue statement
    Continue { line: usize },
    /// Wait statement
    Wait {
        amount: Option<String>,
        unit: Option<String>,
        line: usize,
    },
    /// SC_REPORT_* macro
    Report {
        kind: SystemCReportKind,
        args: Vec<String>,
        line: usize,
    },
    /// sc_trace call
    Trace {
        args: Vec<String>,
        line: usize,
    },
    /// sc_start call
    Start {
        args: Vec<String>,
        line: usize,
    },
    /// sc_stop call
    Stop { line: usize },
    /// Compound statement (block)
    Block { stmts: Vec<SystemCStmt>, line: usize },
    /// Switch statement
    Switch {
        expr: String,
        cases: Vec<SystemCCase>,
        line: usize,
    },
    /// Case label
    CaseLabel { value: String, stmts: Vec<SystemCStmt> },
    /// Default label
    DefaultLabel { stmts: Vec<SystemCStmt> },
    /// sc_fifo/sc_channel read/write
    ChannelAccess {
        channel: String,
        method: String, // read, write, nb_read, nb_write
        args: Vec<String>,
        line: usize,
    },
    /// Method call
    MethodCall {
        object: String,
        method: String,
        args: Vec<String>,
        line: usize,
    },
    /// Sensitivity list
    Sensitivity {
        signals: Vec<SystemCSensitivity>,
        line: usize,
    },
}

/// Case item
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCCase {
    pub value: String,
    pub stmts: Vec<SystemCStmt>,
}

/// Report kind
#[derive(Debug, Clone, PartialEq)]
pub enum SystemCReportKind {
    Info,
    Warning,
    Error,
    Fatal,
}

/// Parser errors
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    UnexpectedToken { expected: String, found: Token, line: usize, column: usize },
    UnexpectedEof { line: usize, column: usize },
    InvalidSyntax { message: String, line: usize, column: usize },
}

/// SystemC Parser
pub struct Parser {
    tokens: Vec<SpannedToken>,
    current: usize,
}

impl Parser {
    pub fn new(tokens: Vec<SpannedToken>) -> Self {
        Self { tokens, current: 0 }
    }

    fn peek(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.current)
    }

    fn peek_n(&self, n: usize) -> Option<&SpannedToken> {
        self.tokens.get(self.current + n)
    }

    fn advance(&mut self) -> Option<SpannedToken> {
        if self.current < self.tokens.len() {
            let tok = self.tokens[self.current].clone();
            self.current += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn expect(&mut self, token_type: Token) -> Result<SpannedToken, ParseError> {
        if let Some(tok) = self.peek() {
            if std::mem::discriminant(&tok.token) == std::mem::discriminant(&token_type) {
                return Ok(self.advance().unwrap());
            }
            return Err(ParseError::UnexpectedToken {
                expected: format!("{:?}", token_type),
                found: tok.token.clone(),
                line: tok.line,
                column: tok.column,
            });
        }
        Err(ParseError::UnexpectedEof {
            line: self.tokens.last().map(|t| t.line).unwrap_or(0),
            column: self.tokens.last().map(|t| t.column).unwrap_or(0),
        })
    }

    fn check(&self, token_type: &Token) -> bool {
        if let Some(tok) = self.peek() {
            std::mem::discriminant(&tok.token) == std::mem::discriminant(token_type)
        } else {
            false
        }
    }

    fn check_ident(&self) -> bool {
        if let Some(tok) = self.peek() {
            matches!(tok.token, Token::Ident(_))
        } else {
            false
        }
    }

    fn consume_ident(&mut self) -> Result<String, ParseError> {
        if let Some(tok) = self.peek() {
            if let Token::Ident(name) = &tok.token {
                let name = name.clone();
                self.advance();
                return Ok(name);
            }
        }
        Err(ParseError::UnexpectedToken {
            expected: "identifier".to_string(),
            found: self.peek().map(|t| t.token.clone()).unwrap_or(Token::Eof),
            line: self.peek().map(|t| t.line).unwrap_or(0),
            column: self.peek().map(|t| t.column).unwrap_or(0),
        })
    }

    fn consume_type_name(&mut self) -> Result<String, ParseError> {
        if let Some(tok) = self.peek() {
            match &tok.token {
                Token::Ident(name) => {
                    let name = name.clone();
                    self.advance();
                    return Ok(name);
                }
                Token::Bool => { self.advance(); return Ok("bool".to_string()); }
                Token::Int => { self.advance(); return Ok("int".to_string()); }
                Token::Char => { self.advance(); return Ok("char".to_string()); }
                Token::Void => { self.advance(); return Ok("void".to_string()); }
                Token::Double => { self.advance(); return Ok("double".to_string()); }
                Token::Float => { self.advance(); return Ok("float".to_string()); }
                Token::SCLogic => { self.advance(); return Ok("sc_logic".to_string()); }
                Token::SCUInt => { self.advance(); return Ok("sc_uint".to_string()); }
                Token::SCInt => { self.advance(); return Ok("sc_int".to_string()); }
                Token::SCBigUInt => { self.advance(); return Ok("sc_biguint".to_string()); }
                Token::SCBigInt => { self.advance(); return Ok("sc_bigint".to_string()); }
                Token::SCFixed => { self.advance(); return Ok("sc_fixed".to_string()); }
                Token::SCUFixed => { self.advance(); return Ok("sc_ufixed".to_string()); }
                Token::SCBit => { self.advance(); return Ok("sc_bit".to_string()); }
                Token::SCBv => { self.advance(); return Ok("sc_bv".to_string()); }
                _ => {}
            }
        }
        Err(ParseError::UnexpectedToken {
            expected: "type name".to_string(),
            found: self.peek().map(|t| t.token.clone()).unwrap_or(Token::Eof),
            line: self.peek().map(|t| t.line).unwrap_or(0),
            column: self.peek().map(|t| t.column).unwrap_or(0),
        })
    }

    fn consume_generic_arg(&mut self) -> Result<String, ParseError> {
        if let Some(tok) = self.peek() {
            match &tok.token {
                Token::Ident(name) => {
                    let name = name.clone();
                    self.advance();
                    return Ok(name);
                }
                Token::StringLit(s) => {
                    let s = s.clone();
                    self.advance();
                    return Ok(format!("\"{}\"", s));
                }
                Token::IntegerLit(s) => {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
                Token::FloatLit(s) => {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
                Token::Bool => { self.advance(); return Ok("bool".to_string()); }
                Token::Int => { self.advance(); return Ok("int".to_string()); }
                Token::Char => { self.advance(); return Ok("char".to_string()); }
                Token::Void => { self.advance(); return Ok("void".to_string()); }
                Token::Double => { self.advance(); return Ok("double".to_string()); }
                Token::Float => { self.advance(); return Ok("float".to_string()); }
                Token::SCLogic => { self.advance(); return Ok("sc_logic".to_string()); }
                Token::SCUInt => { self.advance(); return Ok("sc_uint".to_string()); }
                Token::SCInt => { self.advance(); return Ok("sc_int".to_string()); }
                Token::SCBigUInt => { self.advance(); return Ok("sc_biguint".to_string()); }
                Token::SCBigInt => { self.advance(); return Ok("sc_bigint".to_string()); }
                Token::SCFixed => { self.advance(); return Ok("sc_fixed".to_string()); }
                Token::SCUFixed => { self.advance(); return Ok("sc_ufixed".to_string()); }
                Token::SCBit => { self.advance(); return Ok("sc_bit".to_string()); }
                Token::SCBv => { self.advance(); return Ok("sc_bv".to_string()); }
                _ => {}
            }
        }
        Err(ParseError::UnexpectedToken {
            expected: "generic argument".to_string(),
            found: self.peek().map(|t| t.token.clone()).unwrap_or(Token::Eof),
            line: self.peek().map(|t| t.line).unwrap_or(0),
            column: self.peek().map(|t| t.column).unwrap_or(0),
        })
    }

    /// Parse a complete SystemC file
    pub fn parse(&mut self) -> Result<Vec<SystemCNode>, ParseError> {
        let mut nodes = Vec::new();
        while !self.check(&Token::Eof) {
            if self.check(&Token::SCModule) {
                nodes.push(self.parse_module()?);
            } else if self.check(&Token::SCMain) {
                nodes.push(self.parse_sc_main()?);
            } else if self.check(&Token::Template) {
                // Skip template declarations for now
                self.skip_template();
            } else if self.check(&Token::Typedef) {
                // Skip typedef for now
                self.skip_typedef();
            } else if self.check_ident() {
                // Could be a class/struct definition or function
                if self.peek_n(1).map(|t| matches!(t.token, Token::Ident(_))).unwrap_or(false) {
                    // Type name followed by identifier - could be variable or function
                    self.parse_top_level_decl()?;
                } else {
                    self.advance();
                }
            } else {
                self.advance();
            }
        }
        Ok(nodes)
    }

    fn skip_template(&mut self) {
        self.advance(); // template
        if self.check(&Token::LParen) {
            self.advance();
            // Skip template parameters
            while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                self.advance();
            }
            self.expect(Token::RParen).ok();
        }
    }

    fn skip_typedef(&mut self) {
        self.advance(); // typedef
        while !self.check(&Token::SemiColon) && !self.check(&Token::Eof) {
            self.advance();
        }
        if self.check(&Token::SemiColon) {
            self.advance();
        }
    }

    fn parse_top_level_decl(&mut self) -> Result<(), ParseError> {
        // Consume type name
        let type_name = self.consume_ident()?;
        
        // Check if it's a class/struct
        if self.check_ident() {
            let name = self.consume_ident()?;
            
            if self.check(&Token::LBrace) {
                // Class/struct definition
                self.parse_class_body(&type_name, &name)?;
            } else if self.check(&Token::LParen) {
                // Function declaration/definition
                self.parse_function_body(&type_name, &name)?;
            } else if self.check(&Token::SemiColon) {
                // Variable declaration
                self.advance();
            }
        }
        Ok(())
    }

    fn parse_class_body(&mut self, _class_type: &str, _name: &str) -> Result<(), ParseError> {
        self.expect(Token::LBrace)?;
        
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if self.check(&Token::Public) || self.check(&Token::Private) || self.check(&Token::Protected) {
                self.advance(); // access specifier
                self.expect(Token::Colon).ok();
            } else if self.check(&Token::SCCTor) {
                // Constructor inside class
                self.parse_ctor()?;
            } else if self.check_ident() {
                // Member variable or method
                let type_name = self.consume_ident()?;
                if self.check_ident() {
                    let member_name = self.consume_ident()?;
                    if self.check(&Token::LParen) {
                        // Method
                        self.parse_method_body(&type_name, &member_name)?;
                    } else if self.check(&Token::SemiColon) {
                        // Member variable
                        self.advance();
                    } else if self.check(&Token::Assign) {
                        // Variable with initialization
                        self.advance();
                        self.consume_ident().ok(); // consume value
                        self.expect(Token::SemiColon).ok();
                    }
                }
            } else {
                self.advance();
            }
        }
        
        self.expect(Token::RBrace)?;
        if self.check(&Token::SemiColon) {
            self.advance();
        }
        Ok(())
    }

    fn parse_function_body(&mut self, _return_type: &str, _name: &str) -> Result<(), ParseError> {
        self.expect(Token::LParen)?;
        
        // Parse parameters
        while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
            let _param_type = self.consume_ident().ok();
            if self.check_ident() {
                self.consume_ident().ok();
            }
            if self.check(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        
        if self.check(&Token::LBrace) {
            // Function body
            self.parse_block()?;
        } else if self.check(&Token::SemiColon) {
            // Function declaration
            self.advance();
        }
        Ok(())
    }

    fn parse_method_body(&mut self, _return_type: &str, _name: &str) -> Result<(), ParseError> {
        self.expect(Token::LParen)?;
        
        // Parse parameters
        while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
            let _param_type = self.consume_ident().ok();
            if self.check_ident() {
                self.consume_ident().ok();
            }
            if self.check(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        
        if self.check(&Token::Const) {
            self.advance();
        }
        
        if self.check(&Token::LBrace) {
            self.parse_block()?;
        } else if self.check(&Token::SemiColon) {
            self.advance();
        }
        Ok(())
    }

    fn parse_module(&mut self) -> Result<SystemCNode, ParseError> {
        self.expect(Token::SCModule)?;
        self.expect(Token::LParen)?;
        let name = self.consume_ident()?;
        self.expect(Token::RParen)?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);

        self.expect(Token::LBrace)?;

        let mut members = Vec::new();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if self.check(&Token::Public) || self.check(&Token::Private) || self.check(&Token::Protected) {
                self.advance();
                self.expect(Token::Colon).ok();
            } else if self.check(&Token::SCIn) || self.check(&Token::SCOut) || 
                      self.check(&Token::SCInout) || self.check(&Token::SCSignal) {
                members.push(SystemCMember::Port(self.parse_port()?));
            } else if self.check(&Token::SCSignal) {
                members.push(SystemCMember::Signal(self.parse_signal()?));
            } else if self.check(&Token::SCMethod) || self.check(&Token::SCThread) || self.check(&Token::SCThreadH) {
                members.push(SystemCMember::Process(self.parse_process()?));
            } else if self.check(&Token::SCClock) {
                members.push(SystemCMember::Clock(self.parse_clock()?));
            } else if self.check(&Token::SCFifo) || self.check(&Token::SCEvent) ||
                      self.check(&Token::SCMutex) || self.check(&Token::SCSemaphore) {
                members.push(SystemCMember::Channel(self.parse_channel()?));
            } else if self.check(&Token::SCCTor) {
                // SC_CTOR inside module
                let ctor_stmt = self.parse_ctor()?;
                // Extract processes from constructor body and add as module members
                if let SystemCStmt::Block { stmts, .. } = &ctor_stmt {
                    for stmt in stmts {
                        if let SystemCStmt::Expr(expr) = stmt {
                            if expr.starts_with("process ") {
                                // Parse the process info from the expression
                                let parts: Vec<&str> = expr.split_whitespace().collect();
                                if parts.len() >= 3 {
                                    let kind_str = parts[1];
                                    let name = parts[2];
                                    let kind = match kind_str {
                                        "SC_METHOD" => SystemCProcessKind::Method,
                                        "SC_THREAD" => SystemCProcessKind::Thread,
                                        "SC_CTHREAD" => SystemCProcessKind::CThread,
                                        _ => continue,
                                    };
                                    members.push(SystemCMember::Process(SystemCProcess {
                                        kind,
                                        name: name.to_string(),
                                        sensitivity: Vec::new(),
                                        line: self.peek().map(|t| t.line).unwrap_or(0),
                                    }));
                                }
                            }
                        }
                    }
                }
            } else if self.check_ident() {
                // Could be method, variable, or typedef
                let type_name = self.consume_ident()?;
                if self.check_ident() {
                    let member_name = self.consume_ident()?;
                    if self.check(&Token::LParen) {
                        // Method declaration
                        let method_name = member_name.clone();
                        let return_type = type_name.clone();
                        members.push(SystemCMember::Method(SystemCMethod {
                            name: method_name,
                            return_type,
                            params: Vec::new(),
                            body: Vec::new(),
                            is_const: false,
                            line: self.peek().map(|t| t.line).unwrap_or(0),
                        }));
                        self.parse_method_body(&type_name, &member_name)?;
                    } else if self.check(&Token::SemiColon) {
                        // Variable
                        self.advance();
                    }
                }
            } else {
                self.advance();
            }
        }

        self.expect(Token::RBrace)?;
        if self.check(&Token::SemiColon) {
            self.advance();
        }

        Ok(SystemCNode::Module { name, members, line })
    }

    fn parse_port(&mut self) -> Result<SystemCPort, ParseError> {
        let direction = if self.check(&Token::SCIn) {
            self.advance(); SystemCPortDirection::In
        } else if self.check(&Token::SCOut) {
            self.advance(); SystemCPortDirection::Out
        } else if self.check(&Token::SCInout) {
            self.advance(); SystemCPortDirection::InOut
        } else {
            self.advance(); SystemCPortDirection::Signal
        };

        self.expect(Token::LAngle)?;
        
        // Parse template arguments
        let mut template_args = Vec::new();
        while !self.check(&Token::RAngle) && !self.check(&Token::Eof) {
            template_args.push(self.consume_type_name()?);
            if self.check(&Token::Comma) {
                self.advance();
            }
        }
        let type_name = template_args.first().cloned().unwrap_or_default();
        self.expect(Token::RAngle)?;

        let name = self.consume_ident()?;
        self.expect(Token::SemiColon)?;

        Ok(SystemCPort {
            name,
            direction,
            type_name,
            template_args,
        })
    }

    fn parse_signal(&mut self) -> Result<SystemCSignal, ParseError> {
        self.expect(Token::SCSignal)?;
        self.expect(Token::LAngle)?;
        
        let mut template_args = Vec::new();
        while !self.check(&Token::RAngle) && !self.check(&Token::Eof) {
            template_args.push(self.consume_type_name()?);
            if self.check(&Token::Comma) {
                self.advance();
            }
        }
        let type_name = template_args.first().cloned().unwrap_or_default();
        self.expect(Token::RAngle)?;

        let name = self.consume_ident()?;
        
        let initial_value = if self.check(&Token::Assign) {
            self.advance();
            Some(self.consume_ident()?)
        } else {
            None
        };

        self.expect(Token::SemiColon)?;

        Ok(SystemCSignal {
            name,
            type_name,
            template_args,
            initial_value,
        })
    }

    fn parse_process(&mut self) -> Result<SystemCProcess, ParseError> {
        let kind = if self.check(&Token::SCMethod) {
            self.advance(); SystemCProcessKind::Method
        } else if self.check(&Token::SCThread) {
            self.advance(); SystemCProcessKind::Thread
        } else {
            self.advance(); SystemCProcessKind::CThread
        };

        self.expect(Token::LParen)?;
        let name = self.consume_ident()?;
        self.expect(Token::RParen)?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::SemiColon)?;

        Ok(SystemCProcess {
            kind,
            name,
            sensitivity: Vec::new(),
            line,
        })
    }

    fn parse_process_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let kind = if self.check(&Token::SCMethod) {
            self.advance(); SystemCProcessKind::Method
        } else if self.check(&Token::SCThread) {
            self.advance(); SystemCProcessKind::Thread
        } else {
            self.advance(); SystemCProcessKind::CThread
        };

        self.expect(Token::LParen)?;
        let name = self.consume_ident()?;
        self.expect(Token::RParen)?;
        let _line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::SemiColon)?;

        Ok(SystemCStmt::Expr(format!("process {} {}", 
            match kind {
                SystemCProcessKind::Method => "SC_METHOD",
                SystemCProcessKind::Thread => "SC_THREAD",
                SystemCProcessKind::CThread => "SC_CTHREAD",
            },
            name
        )))
    }

    fn parse_clock(&mut self) -> Result<SystemCClock, ParseError> {
        self.expect(Token::SCClock)?;
        let name = self.consume_ident()?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        
        let mut template_args = Vec::new();
        let mut constructor_args = Vec::new();
        
        if self.check(&Token::LAngle) {
            self.advance();
            while !self.check(&Token::RAngle) && !self.check(&Token::Eof) {
                template_args.push(self.consume_type_name()?);
                if self.check(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(Token::RAngle)?;
        }
        
        if self.check(&Token::LParen) {
            self.advance();
            while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                constructor_args.push(self.consume_generic_arg()?);
                if self.check(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(Token::RParen)?;
        }
        
        self.expect(Token::SemiColon)?;

        Ok(SystemCClock {
            name,
            template_args,
            constructor_args,
            line,
        })
    }

    fn parse_channel(&mut self) -> Result<SystemCChannel, ParseError> {
        let kind = if self.check(&Token::SCFifo) {
            self.advance(); SystemCChannelKind::Fifo
        } else if self.check(&Token::SCEvent) {
            self.advance(); SystemCChannelKind::Event
        } else if self.check(&Token::SCMutex) {
            self.advance(); SystemCChannelKind::Mutex
        } else {
            self.advance(); SystemCChannelKind::Semaphore
        };

        let line = self.peek().map(|t| t.line).unwrap_or(0);
        let mut template_args = Vec::new();
        
        if self.check(&Token::LAngle) {
            self.advance();
            while !self.check(&Token::RAngle) && !self.check(&Token::Eof) {
                template_args.push(self.consume_type_name()?);
                if self.check(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(Token::RAngle)?;
        }

        let name = self.consume_ident()?;
        self.expect(Token::SemiColon)?;

        Ok(SystemCChannel {
            name,
            kind,
            template_args,
            line,
        })
    }

    fn parse_ctor(&mut self) -> Result<SystemCStmt, ParseError> {
        self.expect(Token::SCCTor)?;
        self.expect(Token::LParen)?;
        let _name = self.consume_ident()?;
        self.expect(Token::RParen)?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);

        self.expect(Token::LBrace)?;
        let mut body = Vec::new();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                body.push(stmt);
            } else {
                self.advance();
            }
        }
        self.expect(Token::RBrace)?;
        if self.check(&Token::SemiColon) {
            self.advance();
        }

        Ok(SystemCStmt::Block { stmts: body, line })
    }

fn parse_sc_main(&mut self) -> Result<SystemCNode, ParseError> {
        self.expect(Token::SCMain)?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
            // Parse parameter: type [name] (simplified)
            let mut param_str = String::new();
            while !self.check(&Token::Comma) && !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                if let Some(tok) = self.peek() {
                    param_str.push_str(&format!("{:?} ", tok.token));
                }
                self.advance();
            }
            if !param_str.trim().is_empty() {
                params.push(param_str.trim().to_string());
            }
            if self.check(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        
        self.expect(Token::LBrace)?;
        let mut body = Vec::new();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                body.push(stmt);
            } else {
                self.advance();
            }
        }
        self.expect(Token::RBrace)?;
        
        Ok(SystemCNode::ScMain { params, body, line })
    }

    fn parse_statement(&mut self) -> Result<Option<SystemCStmt>, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);

        if self.check(&Token::SCMethod) || self.check(&Token::SCThread) || self.check(&Token::SCThreadH) {
            Ok(Some(self.parse_process_stmt()?))
        } else if self.check(&Token::If) {
            Ok(Some(self.parse_if_stmt()?))
        } else if self.check(&Token::While) {
            Ok(Some(self.parse_while_stmt()?))
        } else if self.check(&Token::For) {
            Ok(Some(self.parse_for_stmt()?))
        } else if self.check(&Token::Return) {
            Ok(Some(self.parse_return_stmt()?))
        } else if self.check(&Token::Break) {
            self.advance();
            self.expect(Token::SemiColon)?;
            Ok(Some(SystemCStmt::Break { line }))
        } else if self.check(&Token::Continue) {
            self.advance();
            self.expect(Token::SemiColon)?;
            Ok(Some(SystemCStmt::Continue { line }))
        } else if self.check(&Token::SCWait) {
            Ok(Some(self.parse_wait_stmt()?))
        } else if self.check(&Token::SCReportInfo) || self.check(&Token::SCReportWarning) ||
                  self.check(&Token::SCReportError) || self.check(&Token::SCReportFatal) {
            Ok(Some(self.parse_report_stmt()?))
        } else if self.check(&Token::SCTrace) {
            Ok(Some(self.parse_trace_stmt()?))
        } else if self.check(&Token::SCStart) {
            Ok(Some(self.parse_start_stmt()?))
        } else if self.check(&Token::SCStop) {
            self.advance();
            self.expect(Token::LParen)?;
            self.expect(Token::RParen)?;
            self.expect(Token::SemiColon)?;
            Ok(Some(SystemCStmt::Stop { line }))
        } else if self.check(&Token::SCSensitive) {
            Ok(Some(self.parse_sensitivity_stmt()?))
        } else if self.check(&Token::Switch) {
            Ok(Some(self.parse_switch_stmt()?))
        } else if self.check(&Token::Case) {
            Ok(Some(self.parse_case_label()?))
        } else if self.check(&Token::Default) {
            Ok(Some(self.parse_default_label()?))
        } else if self.check(&Token::LBrace) {
            Ok(Some(self.parse_block()?))
        } else if self.check_ident() {
            // Could be variable declaration, method call, or assignment
            let first = self.consume_ident()?;
            
            if self.check(&Token::LParen) {
                // Method call
                return Ok(Some(self.parse_method_call(first)?));
            } else if self.check(&Token::Scope) {
                // Scope resolution - could be static method call
                self.advance(); // ::
                if self.check_ident() {
                    let method = self.consume_ident()?;
                    if self.check(&Token::LParen) {
                        return Ok(Some(self.parse_method_call(format!("{}::{}", first, method))?));
                    }
                }
                // Fall through to expression
            } else if self.check(&Token::Dot) {
                // Member access - could be channel read/write
                self.advance(); // .
                if self.check_ident() {
                    let method = self.consume_ident()?;
                    if self.check(&Token::LParen) {
                        return Ok(Some(SystemCStmt::ChannelAccess {
                            channel: first,
                            method,
                            args: self.parse_call_args()?,
                            line,
                        }));
                    }
                }
                // Fall through to expression
            } else if self.check(&Token::Arrow) {
                // Pointer member access
                self.advance(); // ->
                if self.check_ident() {
                    let method = self.consume_ident()?;
                    if self.check(&Token::LParen) {
                        return Ok(Some(SystemCStmt::ChannelAccess {
                            channel: first,
                            method,
                            args: self.parse_call_args()?,
                            line,
                        }));
                    }
                }
                // Fall through to expression
            } else if self.check(&Token::SemiColon) {
                // Expression statement
                self.advance();
                return Ok(Some(SystemCStmt::Expr(first)));
            } else if self.check(&Token::Assign) {
                // Assignment
                self.advance();
                let value = self.consume_ident()?; // simplified
                self.expect(Token::SemiColon)?;
                return Ok(Some(SystemCStmt::Expr(format!("{} = {}", first, value))));
            } else if self.check(&Token::Comma) {
                // Multiple declarations - skip
                self.advance();
            } else if self.check(&Token::LAngle) {
                // Template instantiation as type
                // e.g., sc_signal<bool> sig;
                let mut template_args = Vec::new();
                while !self.check(&Token::RAngle) && !self.check(&Token::Eof) {
                    template_args.push(self.consume_type_name()?);
                    if self.check(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RAngle)?;
                
                if self.check_ident() {
                    let name = self.consume_ident()?;
                    let initial_value = if self.check(&Token::Assign) {
                        self.advance();
                        Some(self.consume_ident()?)
                    } else {
                        None
                    };
                    self.expect(Token::SemiColon)?;
                    return Ok(Some(SystemCStmt::VarDecl(SystemCVariable {
                        name,
                        type_name: first,
                        template_args,
                        initial_value,
                        is_static: false,
                        is_const: false,
                        line,
                    })));
                }
            }
            
            // Default: expression statement
            self.expect(Token::SemiColon).ok();
            Ok(Some(SystemCStmt::Expr(first)))
        } else {
            Ok(None)
        }
    }

    fn parse_method_call(&mut self, object: String) -> Result<SystemCStmt, ParseError> {
        let method = self.consume_ident()?;
        let args = self.parse_call_args()?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::SemiColon)?;
        
        Ok(SystemCStmt::MethodCall { object, method, args, line })
    }

    fn parse_call_args(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
            args.push(self.consume_generic_arg()?);
            if self.check(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        Ok(args)
    }

    fn parse_if_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::If)?;
        self.expect(Token::LParen)?;
        let condition = self.consume_ident()?; // simplified
        self.expect(Token::RParen)?;

        let mut then_branch = Vec::new();
        let mut else_branch = Vec::new();

        if self.check(&Token::LBrace) {
            then_branch = self.parse_block_stmts()?;
        } else {
            if let Some(stmt) = self.parse_statement()? {
                then_branch.push(stmt);
            }
        }

        if self.check(&Token::Else) {
            self.advance();
            if self.check(&Token::LBrace) {
                else_branch = self.parse_block_stmts()?;
            } else {
                if let Some(stmt) = self.parse_statement()? {
                    else_branch.push(stmt);
                }
            }
        }

        Ok(SystemCStmt::If { condition, then_branch, else_branch, line })
    }

    fn parse_while_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::While)?;
        self.expect(Token::LParen)?;
        let condition = self.consume_ident()?; // simplified
        self.expect(Token::RParen)?;

        let body = if self.check(&Token::LBrace) {
            self.parse_block_stmts()?
        } else {
            let mut stmts = Vec::new();
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            }
            stmts
        };

        Ok(SystemCStmt::While { condition, body, line })
    }

    fn parse_for_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::For)?;
        self.expect(Token::LParen)?;
        
        let init = if !self.check(&Token::SemiColon) {
            Some(self.consume_ident()?) // simplified
        } else {
            None
        };
        self.expect(Token::SemiColon)?;
        
        let condition = if !self.check(&Token::SemiColon) {
            Some(self.consume_ident()?)
        } else {
            None
        };
        self.expect(Token::SemiColon)?;
        
        let step = if !self.check(&Token::RParen) {
            Some(self.consume_ident()?)
        } else {
            None
        };
        self.expect(Token::RParen)?;

        let body = if self.check(&Token::LBrace) {
            self.parse_block_stmts()?
        } else {
            let mut stmts = Vec::new();
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            }
            stmts
        };

        Ok(SystemCStmt::For { init, condition, step, body, line })
    }

    fn parse_return_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Return)?;
        let value = if !self.check(&Token::SemiColon) {
            Some(self.consume_generic_arg()?)
        } else {
            None
        };
        self.expect(Token::SemiColon)?;
        Ok(SystemCStmt::Return { value, line })
    }

    fn parse_wait_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::SCWait)?;
        
        let mut amount = None;
        let mut unit = None;
        
        if self.check(&Token::LParen) {
            self.advance();
            if !self.check(&Token::RParen) {
                amount = Some(self.consume_ident()?);
                if self.check(&Token::Comma) {
                    self.advance();
                    unit = Some(self.consume_ident()?);
                }
            }
            self.expect(Token::RParen)?;
        }
        
        self.expect(Token::SemiColon)?;
        
        Ok(SystemCStmt::Wait { amount, unit, line })
    }

    fn parse_report_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        let kind = if self.check(&Token::SCReportInfo) {
            self.advance(); SystemCReportKind::Info
        } else if self.check(&Token::SCReportWarning) {
            self.advance(); SystemCReportKind::Warning
        } else if self.check(&Token::SCReportError) {
            self.advance(); SystemCReportKind::Error
        } else {
            self.advance(); SystemCReportKind::Fatal
        };
        
        let args = self.parse_call_args()?;
        self.expect(Token::SemiColon)?;
        
        Ok(SystemCStmt::Report { kind, args, line })
    }

    fn parse_trace_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::SCTrace)?;
        let args = self.parse_call_args()?;
        self.expect(Token::SemiColon)?;
        Ok(SystemCStmt::Trace { args, line })
    }

    fn parse_start_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::SCStart)?;
        let args = self.parse_call_args()?;
        self.expect(Token::SemiColon)?;
        Ok(SystemCStmt::Start { args, line })
    }

    fn parse_sensitivity_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::SCSensitive)?;
        
        let mut signals = Vec::new();
        while self.check(&Token::ShiftLeft) {
            self.advance(); // <<
            let signal = self.consume_ident()?;
            let edge = if self.check(&Token::Dot) {
                self.advance();
                if self.check_ident() {
                    let edge_name = self.consume_ident()?;
                    // Handle both .pos() and pos()
                    if self.check(&Token::LParen) {
                        self.advance(); // (
                        self.expect(Token::RParen)?; // )
                    }
                    if edge_name == "pos" { SystemCEdge::Pos }
                    else if edge_name == "neg" { SystemCEdge::Neg }
                    else { SystemCEdge::None }
                } else {
                    SystemCEdge::None
                }
            } else if self.check_ident() {
                // Handle pos() or neg() without dot
                let edge_name = self.consume_ident()?;
                if self.check(&Token::LParen) {
                    self.advance(); // (
                    self.expect(Token::RParen)?; // )
                }
                if edge_name == "pos" { SystemCEdge::Pos }
                else if edge_name == "neg" { SystemCEdge::Neg }
                else { SystemCEdge::None }
            } else {
                SystemCEdge::None
            };
            signals.push(SystemCSensitivity { signal, edge });
        }
        
        self.expect(Token::SemiColon)?;
        Ok(SystemCStmt::Sensitivity { signals, line })
    }

    fn parse_switch_stmt(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Switch)?;
        self.expect(Token::LParen)?;
        let expr = self.consume_ident()?; // simplified
        self.expect(Token::RParen)?;
        
        self.expect(Token::LBrace)?;
        let mut cases = Vec::new();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if self.check(&Token::Case) {
                cases.push(self.parse_case()?);
            } else if self.check(&Token::Default) {
                cases.push(SystemCCase {
                    value: "default".to_string(),
                    stmts: self.parse_default_label_stmts()?,
                });
            } else {
                self.advance();
            }
        }
        self.expect(Token::RBrace)?;
        
        Ok(SystemCStmt::Switch { expr, cases, line })
    }

    fn parse_case(&mut self) -> Result<SystemCCase, ParseError> {
        self.expect(Token::Case)?;
        let value = self.consume_ident()?; // simplified
        self.expect(Token::Colon)?;
        
        let mut stmts = Vec::new();
        while !self.check(&Token::Case) && !self.check(&Token::Default) && !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }
        
        Ok(SystemCCase { value, stmts })
    }

    fn parse_case_label(&mut self) -> Result<SystemCStmt, ParseError> {
        self.expect(Token::Case)?;
        let value = self.consume_ident()?;
        self.expect(Token::Colon)?;
        
        let mut stmts = Vec::new();
        while !self.check(&Token::Case) && !self.check(&Token::Default) && !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }
        
        Ok(SystemCStmt::CaseLabel { value, stmts })
    }

    fn parse_default_label(&mut self) -> Result<SystemCStmt, ParseError> {
        self.expect(Token::Default)?;
        self.expect(Token::Colon)?;
        
        let stmts = self.parse_default_label_stmts()?;
        Ok(SystemCStmt::DefaultLabel { stmts })
    }

    fn parse_default_label_stmts(&mut self) -> Result<Vec<SystemCStmt>, ParseError> {
        let mut stmts = Vec::new();
        while !self.check(&Token::Case) && !self.check(&Token::Default) && !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }
        Ok(stmts)
    }

    fn parse_block(&mut self) -> Result<SystemCStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::LBrace)?;
        let stmts = self.parse_block_stmts()?;
        self.expect(Token::RBrace)?;
        Ok(SystemCStmt::Block { stmts, line })
    }

    fn parse_block_stmts(&mut self) -> Result<Vec<SystemCStmt>, ParseError> {
        let mut stmts = Vec::new();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }
        Ok(stmts)
    }
}

/// Public API: Parse SystemC source into AST nodes.
pub fn parse_systemc(source: &str) -> Result<Vec<SystemCNode>, ParseError> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize();
    let mut parser = Parser::new(tokens);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_module() {
        let src = "SC_MODULE(test) { sc_in<bool> clk; SC_CTOR(test) {} };";
        let nodes = parse_systemc(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let SystemCNode::Module { name, members, .. } = &nodes[0] {
            assert_eq!(name, "test");
            assert_eq!(members.len(), 1);
            if let SystemCMember::Port(port) = &members[0] {
                assert_eq!(port.name, "clk");
                assert_eq!(port.type_name, "bool");
            }
        }
    }

    #[test]
    fn test_parse_module_with_process() {
        let src = "SC_MODULE(alu) { SC_CTOR(alu) { SC_METHOD(run); } };";
        let nodes = parse_systemc(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let SystemCNode::Module { members, .. } = &nodes[0] {
            assert!(members.iter().any(|m| matches!(m, SystemCMember::Process(_))));
        }
    }

    #[test]
    fn test_parse_sc_main() {
        let src = "int sc_main(int argc, char* argv[]) { sc_start(100, SC_NS); return 0; }";
        let nodes = parse_systemc(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let SystemCNode::ScMain { params, body, .. } = &nodes[0] {
            assert_eq!(params.len(), 2);
            assert!(!body.is_empty());
        }
    }

    #[test]
    fn test_parse_ports() {
        let src = "SC_MODULE(test) { sc_in<bool> clk; sc_out<int> data; sc_inout<sc_logic> bidir; sc_signal<int> sig; };";
        let nodes = parse_systemc(src).unwrap();
        if let SystemCNode::Module { members, .. } = &nodes[0] {
            let ports: Vec<_> = members.iter().filter_map(|m| match m {
                SystemCMember::Port(p) => Some(p),
                _ => None,
            }).collect();
            assert_eq!(ports.len(), 4);
        }
    }

    #[test]
    fn test_parse_clock() {
        let src = "SC_MODULE(test) { sc_clock clk; sc_clock fast_clk(\"fast\", 10, SC_NS); };";
        let nodes = parse_systemc(src).unwrap();
        if let SystemCNode::Module { members, .. } = &nodes[0] {
            let clocks: Vec<_> = members.iter().filter_map(|m| match m {
                SystemCMember::Clock(c) => Some(c),
                _ => None,
            }).collect();
            assert_eq!(clocks.len(), 2);
        }
    }

    #[test]
    fn test_parse_channels() {
        let src = "SC_MODULE(test) { sc_fifo<int> data_fifo; sc_event done_evt; sc_mutex bus_mutex; sc_semaphore port_sem; };";
        let nodes = parse_systemc(src).unwrap();
        if let SystemCNode::Module { members, .. } = &nodes[0] {
            let channels: Vec<_> = members.iter().filter_map(|m| match m {
                SystemCMember::Channel(c) => Some(c),
                _ => None,
            }).collect();
            assert_eq!(channels.len(), 4);
        }
    }

    #[test]
    fn test_parse_sensitivity() {
        let src = "SC_MODULE(test) { SC_CTOR(test) { sensitive << clk.pos() << rst.neg(); } };";
        let nodes = parse_systemc(src).unwrap();
        // Just verify it parses without error
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_parse_if_stmt() {
        let src = "SC_MODULE(test) { void run() { if (a) { b = 1; } else { b = 0; } } };";
        let nodes = parse_systemc(src).unwrap();
        // Just verify it parses without error
    }

    #[test]
    fn test_parse_while_stmt() {
        let src = "SC_MODULE(test) { void run() { while (cond) { wait(); } } };";
        let nodes = parse_systemc(src).unwrap();
    }

    #[test]
    fn test_parse_for_stmt() {
        let src = "SC_MODULE(test) { void run() { for (int i = 0; i < 10; i++) { wait(); } } };";
        let nodes = parse_systemc(src).unwrap();
    }

    #[test]
    fn test_parse_wait() {
        let src = "SC_MODULE(test) { void run() { wait(10, SC_NS); wait(); } };";
        let nodes = parse_systemc(src).unwrap();
    }

    #[test]
    fn test_parse_report() {
        let src = "SC_MODULE(test) { void run() { SC_REPORT_INFO(\"ID\", \"msg\"); } };";
        let nodes = parse_systemc(src).unwrap();
    }

    #[test]
    fn test_parse_trace() {
        let src = "SC_MODULE(test) { void run() { sc_trace(tf, clk, \"clk\"); } };";
        let nodes = parse_systemc(src).unwrap();
    }

    #[test]
    fn test_parse_multiple_modules() {
        let src = "SC_MODULE(a) {}; SC_MODULE(b) {};";
        let nodes = parse_systemc(src).unwrap();
        assert_eq!(nodes.len(), 2);
    }
}