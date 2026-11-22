//! VHDL Parser — Parses VHDL tokens into AST.

mod lexer;
use lexer::{Lexer, SpannedToken, Token};

/// VHDL AST node types.
#[derive(Debug, Clone, PartialEq)]
pub enum VhdlNode {
    /// Entity declaration
    Entity {
        name: String,
        generics: Vec<VhdlGeneric>,
        ports: Vec<VhdlPort>,
        line: usize,
    },
    /// Architecture declaration
    Architecture {
        name: String,
        entity_name: String,
        declarative_items: Vec<VhdlDeclItem>,
        statements: Vec<VhdlStmt>,
        line: usize,
    },
    /// Package declaration
    Package {
        name: String,
        items: Vec<VhdlDeclItem>,
        line: usize,
    },
    /// Package body
    PackageBody {
        name: String,
        items: Vec<VhdlDeclItem>,
        line: usize,
    },
    /// Configuration
    Configuration {
        name: String,
        entity_name: String,
        line: usize,
    },
}

/// Generic declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlGeneric {
    pub name: String,
    pub type_name: String,
    pub default_value: Option<String>,
}

/// Port declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlPort {
    pub name: String,
    pub mode: String, // in, out, inout, buffer, linkage
    pub type_name: String,
}

/// Declarative items in architecture/package
#[derive(Debug, Clone, PartialEq)]
pub enum VhdlDeclItem {
    Signal(VhdlSignal),
    Variable(VhdlVariable),
    Constant(VhdlConstant),
    TypeDecl(VhdlTypeDecl),
    SubtypeDecl(VhdlSubtypeDecl),
    Component(VhdlComponent),
    Function(VhdlSubprogram),
    Procedure(VhdlSubprogram),
    Attribute(VhdlAttribute),
    Alias(VhdlAlias),
    FileDecl(VhdlFileDecl),
}

/// Signal declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlSignal {
    pub name: String,
    pub type_name: String,
    pub initial_value: Option<String>,
}

/// Variable declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlVariable {
    pub name: String,
    pub type_name: String,
    pub initial_value: Option<String>,
}

/// Constant declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlConstant {
    pub name: String,
    pub type_name: String,
    pub value: String,
}

/// Type declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlTypeDecl {
    pub name: String,
    pub type_def: VhdlTypeDef,
}

/// Subtype declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlSubtypeDecl {
    pub name: String,
    pub base_type: String,
    pub constraint: Option<String>,
}

/// Type definitions
#[derive(Debug, Clone, PartialEq)]
pub enum VhdlTypeDef {
    ScalarRange { left: String, right: String, direction: String }, // to/downto
    Enum(Vec<String>),
    Record(Vec<VhdlRecordField>),
    Array { index_type: String, element_type: String },
    Access(String),
    File(String),
}

/// Record field
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlRecordField {
    pub name: String,
    pub type_name: String,
}

/// Component declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlComponent {
    pub name: String,
    pub ports: Vec<VhdlPort>,
    pub generics: Vec<VhdlGeneric>,
}

/// Subprogram (function/procedure)
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlSubprogram {
    pub name: String,
    pub is_function: bool,
    pub parameters: Vec<VhdlParam>,
    pub return_type: Option<String>,
    pub body: Vec<VhdlStmt>,
}

/// Parameter
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlParam {
    pub name: String,
    pub mode: String, // in, out, inout
    pub type_name: String,
}

/// Attribute declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlAttribute {
    pub name: String,
    pub type_name: String,
}

/// Alias declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlAlias {
    pub name: String,
    pub type_name: String,
    pub target: String,
}

/// File declaration
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlFileDecl {
    pub name: String,
    pub type_name: String,
    pub mode: String, // in, out
}

/// Statements
#[derive(Debug, Clone, PartialEq)]
pub enum VhdlStmt {
    /// Signal assignment: <=
    SignalAssign {
        target: String,
        value: String,
        line: usize,
    },
    /// Variable assignment: :=
    VariableAssign {
        target: String,
        value: String,
        line: usize,
    },
    /// If statement
    If {
        condition: String,
        then_stmts: Vec<VhdlStmt>,
        elsif_branches: Vec<(String, Vec<VhdlStmt>)>,
        else_stmts: Vec<VhdlStmt>,
        line: usize,
    },
    /// Case statement
    Case {
        expression: String,
        alternatives: Vec<VhdlCaseAlt>,
        line: usize,
    },
    /// Loop statement
    Loop {
        label: Option<String>,
        stmts: Vec<VhdlStmt>,
        line: usize,
    },
    /// For loop
    ForLoop {
        label: Option<String>,
        parameter: String,
        range: String,
        stmts: Vec<VhdlStmt>,
        line: usize,
    },
    /// While loop (wait until)
    WhileLoop {
        condition: String,
        stmts: Vec<VhdlStmt>,
        line: usize,
    },
    /// Exit statement
    Exit {
        label: Option<String>,
        condition: Option<String>,
        line: usize,
    },
    /// Next statement
    Next {
        label: Option<String>,
        condition: Option<String>,
        line: usize,
    },
    /// Return statement
    Return {
        value: Option<String>,
        line: usize,
    },
    /// Wait statement
    Wait {
        sensitivity: Option<String>,
        condition: Option<String>,
        timeout: Option<String>,
        line: usize,
    },
    /// Null statement
    Null { line: usize },
    /// Assert statement
    Assert {
        condition: String,
        report: Option<String>,
        severity: Option<String>,
        line: usize,
    },
    /// Report statement
    Report {
        message: String,
        severity: Option<String>,
        line: usize,
    },
    /// Generate statement
    Generate {
        label: Option<String>,
        scheme: String, // for, if
        stmts: Vec<VhdlStmt>,
        line: usize,
    },
    /// Component instantiation
    Instance {
        label: String,
        component_name: String,
        generic_map: Vec<(String, String)>,
        port_map: Vec<(String, String)>,
        line: usize,
    },
    /// Block statement
    Block {
        label: String,
        stmts: Vec<VhdlStmt>,
        line: usize,
    },
    /// Process statement
    Process {
        label: Option<String>,
        sensitivity: Vec<String>,
        declarative_items: Vec<VhdlDeclItem>,
        stmts: Vec<VhdlStmt>,
        line: usize,
    },
}

/// Case alternative
#[derive(Debug, Clone, PartialEq)]
pub struct VhdlCaseAlt {
    pub choices: Vec<String>,
    pub stmts: Vec<VhdlStmt>,
}

/// Parser errors
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    UnexpectedToken { expected: String, found: Token, line: usize, column: usize },
    UnexpectedEof { line: usize, column: usize },
    InvalidSyntax { message: String, line: usize, column: usize },
}

/// VHDL Parser
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

    /// Parse a complete VHDL file
    pub fn parse(&mut self) -> Result<Vec<VhdlNode>, ParseError> {
        let mut nodes = Vec::new();
        while !self.check(&Token::Eof) {
            if self.check(&Token::Library) {
                // Skip library declarations for now
                self.skip_library();
            } else if self.check(&Token::Use) {
                // Skip use clauses
                self.skip_use();
            } else if self.check(&Token::Entity) {
                nodes.push(self.parse_entity()?);
            } else if self.check(&Token::Architecture) {
                nodes.push(self.parse_architecture()?);
            } else if self.check(&Token::Package) {
                if self.peek_n(1).map(|t| matches!(t.token, Token::Body)).unwrap_or(false) {
                    nodes.push(self.parse_package_body()?);
                } else {
                    nodes.push(self.parse_package()?);
                }
            } else if self.check(&Token::Configuration) {
                nodes.push(self.parse_configuration()?);
            } else {
                // Skip unknown
                self.advance();
            }
        }
        Ok(nodes)
    }

    fn skip_library(&mut self) {
        self.advance(); // library
        while let Some(tok) = self.peek() {
            if matches!(tok.token, Token::SemiColon) {
                self.advance();
                break;
            }
            self.advance();
        }
    }

    fn skip_use(&mut self) {
        self.advance(); // use
        while let Some(tok) = self.peek() {
            if matches!(tok.token, Token::SemiColon) {
                self.advance();
                break;
            }
            self.advance();
        }
    }

    fn parse_entity(&mut self) -> Result<VhdlNode, ParseError> {
        self.expect(Token::Entity)?;
        let name = self.consume_ident()?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);

        self.expect(Token::Is)?;

        let mut generics = Vec::new();
        let mut ports = Vec::new();

        // Parse generic clause if present
        if self.check(&Token::Generic) {
            generics = self.parse_generic_clause()?;
        }

        // Parse port clause if present
        if self.check(&Token::Port) {
            ports = self.parse_port_clause()?;
        }

        self.expect(Token::End)?;
        if self.check(&Token::Entity) {
            self.advance();
        }
        // Optional entity name after 'end entity'
        if self.check_ident() {
            self.consume_ident()?;
        }
        self.expect(Token::SemiColon)?;

        Ok(VhdlNode::Entity { name, generics, ports, line })
    }

    fn parse_generic_clause(&mut self) -> Result<Vec<VhdlGeneric>, ParseError> {
        self.expect(Token::Generic)?;
        self.expect(Token::LParen)?;
        let mut generics = Vec::new();

        while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
            let name = self.consume_ident()?;
            self.expect(Token::Colon)?;
            let type_name = self.consume_ident()?;

            let default_value = if self.check(&Token::VarAssign) {
                self.advance();
                Some(self.parse_literal_or_ident()?)
            } else {
                None
            };

            generics.push(VhdlGeneric { name, type_name, default_value });

            if self.check(&Token::SemiColon) {
                self.advance();
            } else if self.check(&Token::RParen) {
                break;
            }
        }

        self.expect(Token::RParen)?;
        self.expect(Token::SemiColon)?;
        Ok(generics)
    }

    fn parse_literal_or_ident(&mut self) -> Result<String, ParseError> {
        if self.check(&Token::IntegerLit("".to_string())) {
            if let Some(tok) = self.peek() {
                if let Token::IntegerLit(s) = &tok.token {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
            }
        }
        if self.check(&Token::RealLit("".to_string())) {
            if let Some(tok) = self.peek() {
                if let Token::RealLit(s) = &tok.token {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
            }
        }
        if self.check(&Token::StringLit("".to_string())) {
            if let Some(tok) = self.peek() {
                if let Token::StringLit(s) = &tok.token {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
            }
        }
        if self.check(&Token::CharLit(' ')) {
            if let Some(tok) = self.peek() {
                if let Token::CharLit(c) = &tok.token {
                    let c = *c;
                    self.advance();
                    return Ok(c.to_string());
                }
            }
        }
        // Fallback to identifier
        self.consume_ident()
    }

    fn parse_port_clause(&mut self) -> Result<Vec<VhdlPort>, ParseError> {
        self.expect(Token::Port)?;
        self.expect(Token::LParen)?;
        let mut ports = Vec::new();

        while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
            eprintln!("  parse_port_clause: current token = {:?}", self.peek());
            let name = self.consume_ident()?;
            eprintln!("    port name = {}", name);
            self.expect(Token::Colon)?;

            // Mode can be a keyword (in, out, inout, buffer, linkage) or identifier
            let mode = if self.check(&Token::In) {
                self.advance(); "in".to_string()
            } else if self.check(&Token::Out) {
                self.advance(); "out".to_string()
            } else if self.check(&Token::Inout) {
                self.advance(); "inout".to_string()
            } else if self.check(&Token::Buffer) {
                self.advance(); "buffer".to_string()
            } else if self.check(&Token::Linkage) {
                self.advance(); "linkage".to_string()
            } else {
                self.consume_ident()?
            };
            eprintln!("    port mode = {}", mode);

            let type_name = self.consume_ident()?;
            eprintln!("    port type = {}", type_name);

            ports.push(VhdlPort { name, mode, type_name });

            if self.check(&Token::SemiColon) {
                self.advance();
            } else if self.check(&Token::RParen) {
                break;
            }
        }

        self.expect(Token::RParen)?;
        self.expect(Token::SemiColon)?;
        Ok(ports)
    }

    fn parse_architecture(&mut self) -> Result<VhdlNode, ParseError> {
        self.expect(Token::Architecture)?;
        let name = self.consume_ident()?;
        self.expect(Token::Of)?;
        let entity_name = self.consume_ident()?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);

        self.expect(Token::Is)?;

        // Parse declarative region
        let mut declarative_items = Vec::new();
        while !self.check(&Token::Begin) && !self.check(&Token::Eof) {
            if let Some(item) = self.parse_decl_item()? {
                declarative_items.push(item);
            } else {
                self.advance();
            }
        }

        self.expect(Token::Begin)?;

        // Parse statement region
        let mut statements = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                statements.push(stmt);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Architecture) {
            self.advance();
        }
        // Optional architecture name after 'end architecture'
        if self.check_ident() {
            self.consume_ident()?;
        }
        self.expect(Token::SemiColon)?;

        Ok(VhdlNode::Architecture {
            name,
            entity_name,
            declarative_items,
            statements,
            line,
        })
    }

    fn parse_decl_item(&mut self) -> Result<Option<VhdlDeclItem>, ParseError> {
        if self.check(&Token::Signal) {
            Ok(Some(self.parse_signal_decl()?))
        } else if self.check(&Token::Variable) {
            Ok(Some(self.parse_variable_decl()?))
        } else if self.check(&Token::Constant) {
            Ok(Some(self.parse_constant_decl()?))
        } else if self.check(&Token::Type) {
            Ok(Some(self.parse_type_decl()?))
        } else if self.check(&Token::Subtype) {
            Ok(Some(self.parse_subtype_decl()?))
        } else if self.check(&Token::Component) {
            Ok(Some(self.parse_component_decl()?))
        } else if self.check(&Token::Function) {
            Ok(Some(self.parse_subprogram_decl(true)?))
        } else if self.check(&Token::Procedure) {
            Ok(Some(self.parse_subprogram_decl(false)?))
        } else if self.check(&Token::Attribute) {
            Ok(Some(self.parse_attribute_decl()?))
        } else if self.check(&Token::Alias) {
            Ok(Some(self.parse_alias_decl()?))
        } else if self.check(&Token::File) {
            Ok(Some(self.parse_file_decl()?))
        } else {
            Ok(None)
        }
    }

    fn parse_signal_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Signal)?;
        let name = self.consume_ident()?;
        eprintln!("    signal name = {}", name);
        self.expect(Token::Colon)?;
        let type_name = self.consume_ident()?;
        eprintln!("    signal type = {}", type_name);

        let initial_value = if self.check(&Token::VarAssign) {
            self.advance();
            Some(self.parse_literal_or_ident()?)
        } else {
            None
        };

        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::Signal(VhdlSignal { name, type_name, initial_value }))
    }

    fn parse_variable_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Variable)?;
        let name = self.consume_ident()?;
        self.expect(Token::Colon)?;
        let type_name = self.consume_ident()?;

        let initial_value = if self.check(&Token::VarAssign) {
            self.advance();
            Some(self.consume_ident()?)
        } else {
            None
        };

        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::Variable(VhdlVariable { name, type_name, initial_value }))
    }

    fn parse_constant_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Constant)?;
        let name = self.consume_ident()?;
        self.expect(Token::Colon)?;
        let type_name = self.consume_ident()?;
        self.expect(Token::VarAssign)?;
        let value = self.parse_literal_or_ident()?;
        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::Constant(VhdlConstant { name, type_name, value }))
    }

    fn parse_type_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Type)?;
        let name = self.consume_ident()?;
        self.expect(Token::Is)?;

        let type_def = if self.check(&Token::LParen) {
            // Enum type: (val1, val2, ...)
            self.expect(Token::LParen)?;
            let mut values = Vec::new();
            while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                values.push(self.consume_ident()?);
                if self.check(&Token::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            VhdlTypeDef::Enum(values)
        } else if self.check(&Token::Record) {
            self.advance(); // record
            let mut fields = Vec::new();
            while !self.check(&Token::End) && !self.check(&Token::Eof) {
                let field_name = self.consume_ident()?;
                self.expect(Token::Colon)?;
                let field_type = self.consume_ident()?;
                self.expect(Token::SemiColon)?;
                fields.push(VhdlRecordField { name: field_name, type_name: field_type });
            }
            self.expect(Token::End)?;
            if self.check(&Token::Record) { self.advance(); }
            VhdlTypeDef::Record(fields)
        } else if self.check(&Token::Array) {
            self.advance(); // array
            // Simplified: array (index_type) of element_type
            let index_type = self.consume_ident()?;
            self.expect(Token::Of)?;
            let element_type = self.consume_ident()?;
            VhdlTypeDef::Array { index_type, element_type }
        } else if self.check(&Token::Access) {
            self.advance(); // access
            let target_type = self.consume_ident()?;
            VhdlTypeDef::Access(target_type)
        } else if self.check(&Token::File) {
            self.advance(); // file
            let type_name = self.consume_ident()?;
            VhdlTypeDef::File(type_name)
        } else {
            // Scalar range type: range left to/downto right
            let left = self.consume_ident()?;
            let direction = if self.check(&Token::To) { "to" } else { "downto" };
            if self.check(&Token::To) { self.advance(); } else { self.advance(); } // downto
            let right = self.consume_ident()?;
            VhdlTypeDef::ScalarRange { left, right, direction: direction.to_string() }
        };

        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::TypeDecl(VhdlTypeDecl { name, type_def }))
    }

    fn parse_subtype_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Subtype)?;
        let name = self.consume_ident()?;
        self.expect(Token::Is)?;
        let base_type = self.consume_ident()?;

        let constraint = if self.check(&Token::Range) {
            self.advance();
            let left = self.consume_ident()?;
            let dir = if self.check(&Token::To) { "to" } else { "downto" };
            if self.check(&Token::To) { self.advance(); } else { self.advance(); }
            let right = self.consume_ident()?;
            Some(format!("range {} {} {}", left, dir, right))
        } else {
            None
        };

        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::SubtypeDecl(VhdlSubtypeDecl { name, base_type, constraint }))
    }

    fn parse_component_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Component)?;
        let name = self.consume_ident()?;
        let mut generics = Vec::new();
        let mut ports = Vec::new();

        if self.check(&Token::Generic) {
            generics = self.parse_generic_clause()?;
        }
        if self.check(&Token::Port) {
            ports = self.parse_port_clause()?;
        }

        self.expect(Token::End)?;
        if self.check(&Token::Component) { self.advance(); }
        self.expect(Token::SemiColon)?;

        Ok(VhdlDeclItem::Component(VhdlComponent { name, ports, generics }))
    }

    fn parse_subprogram_decl(&mut self, is_function: bool) -> Result<VhdlDeclItem, ParseError> {
        if is_function { self.expect(Token::Function)?; } else { self.expect(Token::Procedure)?; }
        let name = self.consume_ident()?;

        let mut parameters = Vec::new();
        let mut return_type = None;

        if self.check(&Token::LParen) {
            self.advance();
            while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                let param_name = self.consume_ident()?;
                let mode = if self.check(&Token::In) { self.advance(); "in".to_string() }
                    else if self.check(&Token::Out) { self.advance(); "out".to_string() }
                    else if self.check(&Token::Inout) { self.advance(); "inout".to_string() }
                    else { "in".to_string() };
                self.expect(Token::Colon)?;
                let type_name = self.consume_ident()?;
                parameters.push(VhdlParam { name: param_name, mode, type_name });
                if self.check(&Token::SemiColon) { self.advance(); }
            }
            self.expect(Token::RParen)?;
        }

        if is_function {
            if self.check(&Token::Return) {
                self.advance();
                return_type = Some(self.consume_ident()?);
            }
        }

        self.expect(Token::Is)?;

        // Parse declarative items in subprogram
        let mut body = Vec::new();
        while !self.check(&Token::Begin) && !self.check(&Token::Eof) {
            if let Some(_item) = self.parse_decl_item()? {
                // Convert decl items to stmts for simplicity
            } else {
                self.advance();
            }
        }

        self.expect(Token::Begin)?;
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                body.push(stmt);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if is_function { self.expect(Token::Function)?; } else { self.expect(Token::Procedure)?; }
        self.consume_ident()?; // name
        self.expect(Token::SemiColon)?;

        if is_function {
            Ok(VhdlDeclItem::Function(VhdlSubprogram { name, is_function: true, parameters, return_type, body }))
        } else {
            Ok(VhdlDeclItem::Procedure(VhdlSubprogram { name, is_function: false, parameters, return_type, body }))
        }
    }

    fn parse_attribute_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Attribute)?;
        let name = self.consume_ident()?;
        self.expect(Token::Colon)?;
        let type_name = self.consume_ident()?;
        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::Attribute(VhdlAttribute { name, type_name }))
    }

    fn parse_alias_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::Alias)?;
        let name = self.consume_ident()?;
        self.expect(Token::Colon)?;
        let type_name = self.consume_ident()?;
        self.expect(Token::Is)?;
        let target = self.consume_ident()?;
        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::Alias(VhdlAlias { name, type_name, target }))
    }

    fn parse_file_decl(&mut self) -> Result<VhdlDeclItem, ParseError> {
        self.expect(Token::File)?;
        let name = self.consume_ident()?;
        self.expect(Token::Colon)?;
        let type_name = self.consume_ident()?;
        let mode = if self.check(&Token::In) { self.advance(); "in".to_string() }
            else if self.check(&Token::Out) { self.advance(); "out".to_string() }
            else { "in".to_string() };
        self.expect(Token::SemiColon)?;
        Ok(VhdlDeclItem::FileDecl(VhdlFileDecl { name, type_name, mode }))
    }

    fn parse_statement(&mut self) -> Result<Option<VhdlStmt>, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);

        if self.check(&Token::If) {
            Ok(Some(self.parse_if_stmt()?))
        } else if self.check(&Token::Case) {
            Ok(Some(self.parse_case_stmt()?))
        } else if self.check(&Token::Loop) {
            Ok(Some(self.parse_loop_stmt()?))
        } else if self.check(&Token::For) {
            Ok(Some(self.parse_for_loop()?))
        } else if self.check(&Token::While) || self.check(&Token::Wait) {
            Ok(Some(self.parse_wait_or_while()?))
        } else if self.check(&Token::Exit) {
            Ok(Some(self.parse_exit_stmt()?))
        } else if self.check(&Token::Next) {
            Ok(Some(self.parse_next_stmt()?))
        } else if self.check(&Token::Return) {
            Ok(Some(self.parse_return_stmt()?))
        } else if self.check(&Token::Null) {
            self.advance();
            self.expect(Token::SemiColon)?;
            Ok(Some(VhdlStmt::Null { line }))
        } else if self.check(&Token::Assert) {
            Ok(Some(self.parse_assert_stmt()?))
        } else if self.check(&Token::Report) {
            Ok(Some(self.parse_report_stmt()?))
        } else if self.check(&Token::Generate) {
            Ok(Some(self.parse_generate_stmt()?))
        } else if self.check(&Token::Process) {
            Ok(Some(self.parse_process_stmt()?))
        } else if self.check(&Token::Block) {
            Ok(Some(self.parse_block_stmt()?))
        } else if let Some(tok) = self.peek() {
            if let Token::Ident(_) = &tok.token {
                // Could be label or assignment
                if self.peek_n(1).map(|t| matches!(t.token, Token::Colon)).unwrap_or(false) {
                    // Label followed by statement
                    Ok(Some(self.parse_labeled_stmt()?))
                } else {
                    // Check for assignment
                    Ok(Some(self.parse_assignment()?))
                }
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn parse_labeled_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let label = self.consume_ident()?;
        self.expect(Token::Colon)?;

        // Check what follows the label
        if self.check(&Token::Process) {
            return self.parse_process_stmt_with_label(Some(label));
        } else if self.check(&Token::Block) {
            return self.parse_block_stmt_with_label(Some(label));
        } else if self.check(&Token::Generate) {
            return self.parse_generate_stmt_with_label(Some(label));
        } else if self.check(&Token::Loop) {
            return self.parse_loop_stmt_with_label(Some(label));
        } else if self.check(&Token::Entity) {
            // Entity instantiation: label: entity lib.entity [arch] port map (...)
            return self.parse_entity_instantiation(label);
        }

        // If none of the above, it's likely a label on a statement
        // Just parse the next statement
        let stmt = self.parse_statement()?.ok_or(ParseError::InvalidSyntax {
            message: "Expected statement after label".to_string(),
            line: self.peek().map(|t| t.line).unwrap_or(0),
            column: self.peek().map(|t| t.column).unwrap_or(0),
        });
        stmt
    }

    fn parse_entity_instantiation(&mut self, label: String) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Entity)?;

        // Parse library.entity_name (simplified: just consume identifiers and dots)
        let mut comp_parts = Vec::new();
        while self.check_ident() || self.check(&Token::Dot) {
            if let Some(tok) = self.peek() {
                comp_parts.push(format!("{:?}", tok.token));
                self.advance();
            } else {
                break;
            }
        }
        let full_name = comp_parts.join(" ");
        // Extract just the entity name (last identifier)
        let component_name = comp_parts.last()
            .map(|s| s.trim_start_matches("Ident(\"").trim_end_matches("\")").to_string())
            .unwrap_or(full_name);

        // Optional architecture specification
        let _architecture = if self.check(&Token::Ident("architecture".to_string())) || self.check(&Token::Ident("arch".to_string())) {
            self.advance();
            if self.check_ident() { self.consume_ident()?; }
            true
        } else {
            false
        };

        // Generic map (optional)
        let mut generic_map = Vec::new();
        if self.check(&Token::Generic) {
            self.advance(); // generic
            self.expect(Token::Map)?;
            self.expect(Token::LParen)?;
            while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                let name = self.consume_ident()?;
                self.expect(Token::Arrow)?;
                let value = self.parse_literal_or_ident()?;
                generic_map.push((name, value));
                if self.check(&Token::Comma) { self.advance(); }
            }
            self.expect(Token::RParen)?;
        }

        // Port map (optional)
        let mut port_map = Vec::new();
        if self.check(&Token::Port) {
            self.advance(); // port
            self.expect(Token::Map)?;
            self.expect(Token::LParen)?;
            while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                let name = self.consume_ident()?;
                self.expect(Token::Arrow)?;
                let value = self.parse_literal_or_ident()?;
                port_map.push((name, value));
                if self.check(&Token::Comma) { self.advance(); }
            }
            self.expect(Token::RParen)?;
        }

        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Instance {
            label,
            component_name,
            generic_map,
            port_map,
            line,
        })
    }

    fn parse_process_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        self.parse_process_stmt_with_label(None)
    }

    fn parse_process_stmt_with_label(&mut self, label: Option<String>) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Process)?;

        let mut sensitivity = Vec::new();
        if self.check(&Token::LParen) {
            self.advance();
            if self.check(&Token::All) {
                self.advance();
            } else {
                while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                    sensitivity.push(self.consume_ident()?);
                    if self.check(&Token::Comma) { self.advance(); }
                }
            }
            self.expect(Token::RParen)?;
        }

        // Check if there's a declarative region (optional 'is')
        let mut declarative_items = Vec::new();
        if self.check(&Token::Is) {
            self.advance(); // consume 'is'

            // Parse declarative items
            while !self.check(&Token::Begin) && !self.check(&Token::Eof) {
                if let Some(item) = self.parse_decl_item()? {
                    declarative_items.push(item);
                } else {
                    self.advance();
                }
            }
        }

        self.expect(Token::Begin)?;

        // Parse statements
        let mut stmts = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Process) { self.advance(); }
        if label.is_some() { self.consume_ident()?; }
        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Process { label, sensitivity, declarative_items, stmts, line })
    }

    fn parse_block_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        self.parse_block_stmt_with_label(None)
    }

    fn parse_block_stmt_with_label(&mut self, label: Option<String>) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Block)?;

        if self.check(&Token::LParen) {
            self.advance();
            // Skip guard expression
            while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                self.advance();
            }
            self.expect(Token::RParen)?;
        }

        self.expect(Token::Is)?;

        let mut stmts = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Block) { self.advance(); }
        let label_str = label.unwrap_or_default();
        if !label_str.is_empty() { self.consume_ident()?; }
        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Block { label: label_str, stmts, line })
    }

    fn parse_generate_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        self.parse_generate_stmt_with_label(None)
    }

    fn parse_generate_stmt_with_label(&mut self, label: Option<String>) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        if label.is_none() && self.peek_n(1).map(|t| matches!(t.token, Token::Colon)).unwrap_or(false) {
            let lbl = self.consume_ident()?;
            self.expect(Token::Colon)?;
            return self.parse_generate_stmt_with_label(Some(lbl));
        }

        self.expect(Token::Generate)?;
        let scheme = if self.check(&Token::For) { "for".to_string() } else { "if".to_string() };

        if self.check(&Token::For) {
            self.advance(); // for
            let _param = self.consume_ident()?;
            self.expect(Token::In)?;
            let _range = self.consume_ident()?; // simplified
        } else if self.check(&Token::If) {
            self.advance(); // if
            // condition
            let _ = self.consume_ident()?;
        }

        self.expect(Token::Generate)?;

        let mut stmts = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Generate) { self.advance(); }
        if label.is_some() { self.consume_ident()?; }
        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Generate { label, scheme, stmts, line })
    }

    fn parse_if_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::If)?;

        // Parse condition - consume tokens until 'then'
        let mut condition_parts = Vec::new();
        while !self.check(&Token::Then) && !self.check(&Token::Eof) {
            if let Some(tok) = self.peek() {
                condition_parts.push(format!("{:?}", tok.token));
                self.advance();
            } else {
                break;
            }
        }
        let condition = condition_parts.join(" ");

        self.expect(Token::Then)?;

        let mut then_stmts = Vec::new();
        let mut elsif_branches = Vec::new();
        let mut else_stmts = Vec::new();

        while !self.check(&Token::End) && !self.check(&Token::Elsif) && !self.check(&Token::Else) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                then_stmts.push(stmt);
            } else {
                self.advance();
            }
        }

        while self.check(&Token::Elsif) {
            self.advance(); // elsif
            let cond = self.consume_ident()?; // simplified
            self.expect(Token::Then)?;
            let mut branch_stmts = Vec::new();
            while !self.check(&Token::End) && !self.check(&Token::Elsif) && !self.check(&Token::Else) && !self.check(&Token::Eof) {
                if let Some(stmt) = self.parse_statement()? {
                    branch_stmts.push(stmt);
                } else {
                    self.advance();
                }
            }
            elsif_branches.push((cond, branch_stmts));
        }

        if self.check(&Token::Else) {
            self.advance(); // else
            while !self.check(&Token::End) && !self.check(&Token::Eof) {
                if let Some(stmt) = self.parse_statement()? {
                    else_stmts.push(stmt);
                } else {
                    self.advance();
                }
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::If) { self.advance(); }
        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::If { condition, then_stmts, elsif_branches, else_stmts, line })
    }

    fn parse_case_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Case)?;

        // Parse case expression - simplified: consume until 'is'
        let mut expr_parts = Vec::new();
        while !self.check(&Token::Is) && !self.check(&Token::Eof) {
            if let Some(tok) = self.peek() {
                expr_parts.push(format!("{:?}", tok.token));
                self.advance();
            } else {
                break;
            }
        }
        let expression = expr_parts.join(" ");

        self.expect(Token::Is)?;

        let mut alternatives = Vec::new();

        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if self.check(&Token::When) {
                self.advance(); // when
                let mut choices = Vec::new();
                while !self.check(&Token::Arrow) && !self.check(&Token::Eof) {
                    choices.push(self.parse_case_choice()?);
                    if self.check(&Token::Comma) { self.advance(); }
                }
                eprintln!("  case choices: {:?}", choices);
                self.expect(Token::Arrow)?;

                let mut stmts = Vec::new();
                while !self.check(&Token::When) && !self.check(&Token::End) && !self.check(&Token::Eof) {
                    if let Some(stmt) = self.parse_statement()? {
                        stmts.push(stmt);
                    } else {
                        self.advance();
                    }
                }
                alternatives.push(VhdlCaseAlt { choices, stmts });
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Case) { self.advance(); }
        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Case { expression, alternatives, line })
    }

    fn parse_case_choice(&mut self) -> Result<String, ParseError> {
        if self.check(&Token::IntegerLit("".to_string())) {
            if let Some(tok) = self.peek() {
                if let Token::IntegerLit(s) = &tok.token {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
            }
        }
        if self.check(&Token::RealLit("".to_string())) {
            if let Some(tok) = self.peek() {
                if let Token::RealLit(s) = &tok.token {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
            }
        }
        if self.check(&Token::CharLit(' ')) {
            if let Some(tok) = self.peek() {
                if let Token::CharLit(c) = &tok.token {
                    let c = *c;
                    self.advance();
                    return Ok(format!("'{}'", c));
                }
            }
        }
        if self.check(&Token::StringLit("".to_string())) {
            if let Some(tok) = self.peek() {
                if let Token::StringLit(s) = &tok.token {
                    let s = s.clone();
                    self.advance();
                    return Ok(format!("\"{}\"", s));
                }
            }
        }
        if self.check(&Token::Others) {
            self.advance();
            return Ok("others".to_string());
        }
        // Fallback to identifier
        self.consume_ident()
    }

    fn parse_loop_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        self.parse_loop_stmt_with_label(None)
    }

    fn parse_loop_stmt_with_label(&mut self, label: Option<String>) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Loop)?;

        let mut stmts = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Loop) { self.advance(); }
        if label.is_some() { self.consume_ident()?; }
        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Loop { label, stmts, line })
    }

    fn parse_for_loop(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        let label = if self.peek_n(1).map(|t| matches!(t.token, Token::Colon)).unwrap_or(false) {
            Some(self.consume_ident()?)
        } else {
            None
        };

        self.expect(Token::For)?;
        let parameter = self.consume_ident()?;
        self.expect(Token::In)?;
        let range = self.consume_ident()?; // simplified
        self.expect(Token::Loop)?;

        let mut stmts = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Loop) { self.advance(); }
        if label.is_some() { self.consume_ident()?; }
        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::ForLoop { label, parameter, range, stmts, line })
    }

    fn parse_wait_or_while(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);

        if self.check(&Token::Wait) {
            self.advance(); // wait
            let sensitivity = if self.check(&Token::On) {
                self.advance(); Some(self.consume_ident()?)
            } else { None };
            let condition = if self.check(&Token::Until) {
                self.advance(); Some(self.consume_ident()?)
            } else { None };
            let timeout = if self.check(&Token::For) {
                self.advance(); Some(self.consume_ident()?)
            } else { None };
            self.expect(Token::SemiColon)?;

            Ok(VhdlStmt::Wait { sensitivity, condition, timeout, line })
        } else {
            // While loop (not standard VHDL, but handle gracefully)
            self.advance(); // while
            let condition = self.consume_ident()?;
            self.expect(Token::Loop)?;

            let mut stmts = Vec::new();
            while !self.check(&Token::End) && !self.check(&Token::Eof) {
                if let Some(stmt) = self.parse_statement()? {
                    stmts.push(stmt);
                } else {
                    self.advance();
                }
            }

            self.expect(Token::End)?;
            if self.check(&Token::Loop) { self.advance(); }
            self.expect(Token::SemiColon)?;

            Ok(VhdlStmt::WhileLoop { condition, stmts, line })
        }
    }

    fn parse_exit_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Exit)?;

        let label = if self.check_ident() {
            Some(self.consume_ident()?)
        } else { None };

        let condition = if self.check(&Token::When) {
            self.advance(); Some(self.consume_ident()?)
        } else { None };

        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Exit { label, condition, line })
    }

    fn parse_next_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Next)?;

        let label = if self.check_ident() {
            Some(self.consume_ident()?)
        } else { None };

        let condition = if self.check(&Token::When) {
            self.advance(); Some(self.consume_ident()?)
        } else { None };

        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Next { label, condition, line })
    }

    fn parse_return_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Return)?;

        let value = if self.check_ident() {
            Some(self.consume_ident()?)
        } else { None };

        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Return { value, line })
    }

    fn parse_assert_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Assert)?;
        let condition = self.consume_ident()?; // simplified

        let report = if self.check(&Token::Report) {
            self.advance(); Some(self.consume_ident()?)
        } else { None };

        let severity = if self.check(&Token::Severity) {
            self.advance(); Some(self.consume_ident()?)
        } else { None };

        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Assert { condition, report, severity, line })
    }

    fn parse_report_stmt(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Report)?;
        let message = self.consume_ident()?; // simplified

        let severity = if self.check(&Token::Severity) {
            self.advance(); Some(self.consume_ident()?)
        } else { None };

        self.expect(Token::SemiColon)?;

        Ok(VhdlStmt::Report { message, severity, line })
    }

    fn parse_assignment(&mut self) -> Result<VhdlStmt, ParseError> {
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        let target = self.consume_ident()?;

        if self.check(&Token::Assign) { // <=
            self.advance();
            let value = self.consume_ident()?; // simplified
            self.expect(Token::SemiColon)?;
            Ok(VhdlStmt::SignalAssign { target, value, line })
        } else if self.check(&Token::VarAssign) { // :=
            self.advance();
            let value = self.consume_ident()?; // simplified
            self.expect(Token::SemiColon)?;
            Ok(VhdlStmt::VariableAssign { target, value, line })
        } else {
            // Component instantiation
            self.expect(Token::Colon)?;
            let component_name = self.consume_ident()?;

            let mut generic_map = Vec::new();
            let mut port_map = Vec::new();

            if self.check(&Token::Generic) {
                self.advance(); // generic
                self.expect(Token::Map)?;
                self.expect(Token::LParen)?;
                while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                    let name = self.consume_ident()?;
                    self.expect(Token::Arrow)?;
                    let value = self.consume_ident()?;
                    generic_map.push((name, value));
                    if self.check(&Token::Comma) { self.advance(); }
                }
                self.expect(Token::RParen)?;
            }

            if self.check(&Token::Port) {
                self.advance(); // port
                self.expect(Token::Map)?;
                self.expect(Token::LParen)?;
                while !self.check(&Token::RParen) && !self.check(&Token::Eof) {
                    let name = self.consume_ident()?;
                    self.expect(Token::Arrow)?;
                    let value = self.consume_ident()?;
                    port_map.push((name, value));
                    if self.check(&Token::Comma) { self.advance(); }
                }
                self.expect(Token::RParen)?;
            }

            self.expect(Token::SemiColon)?;

            Ok(VhdlStmt::Instance {
                label: target,
                component_name,
                generic_map,
                port_map,
                line,
            })
        }
    }

    fn parse_package(&mut self) -> Result<VhdlNode, ParseError> {
        self.expect(Token::Package)?;
        let name = self.consume_ident()?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Is)?;

        let mut items = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(item) = self.parse_decl_item()? {
                items.push(item);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::Package) { self.advance(); }
        // Optional package name after 'end package'
        if self.check_ident() {
            self.consume_ident()?;
        }
        self.expect(Token::SemiColon)?;

        Ok(VhdlNode::Package { name, items, line })
    }

    fn parse_package_body(&mut self) -> Result<VhdlNode, ParseError> {
        self.expect(Token::Package)?;
        self.expect(Token::Body)?;
        let name = self.consume_ident()?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Is)?;

        let mut items = Vec::new();
        while !self.check(&Token::End) && !self.check(&Token::Eof) {
            if let Some(item) = self.parse_decl_item()? {
                items.push(item);
            } else {
                self.advance();
            }
        }

        self.expect(Token::End)?;
        if self.check(&Token::PackageBody) || self.check(&Token::Package) { self.advance(); }
        if self.check(&Token::Body) { self.advance(); }
        self.consume_ident()?; // name
        self.expect(Token::SemiColon)?;

        Ok(VhdlNode::PackageBody { name, items, line })
    }

    fn parse_configuration(&mut self) -> Result<VhdlNode, ParseError> {
        self.expect(Token::Configuration)?;
        let name = self.consume_ident()?;
        let line = self.peek().map(|t| t.line).unwrap_or(0);
        self.expect(Token::Of)?;
        let entity_name = self.consume_ident()?;
        self.expect(Token::Is)?;

        self.expect(Token::End)?;
        if self.check(&Token::Configuration) { self.advance(); }
        self.consume_ident()?; // name
        self.expect(Token::SemiColon)?;

        Ok(VhdlNode::Configuration { name, entity_name, line })
    }
}

/// Public API: Parse VHDL source into AST nodes.
pub fn parse_vhdl(source: &str) -> Result<Vec<VhdlNode>, ParseError> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize();
    let mut parser = Parser::new(tokens);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_entity() {
        let src = "entity test is port (a : in bit); end entity;";
        eprintln!("Parsing: {}", src);
        let nodes = parse_vhdl(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let VhdlNode::Entity { name, ports, .. } = &nodes[0] {
            assert_eq!(name, "test");
            assert_eq!(ports.len(), 1);
            assert_eq!(ports[0].name, "a");
            assert_eq!(ports[0].mode, "in");
        }
    }

    #[test]
    fn test_parse_entity_with_generic() {
        let src = "entity test is generic (WIDTH : integer := 8); port (a : in bit); end entity;";
        let nodes = parse_vhdl(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let VhdlNode::Entity { generics, .. } = &nodes[0] {
            assert_eq!(generics.len(), 1);
            assert_eq!(generics[0].name, "width");
            assert_eq!(generics[0].type_name, "integer");
        }
    }

    #[test]
    fn test_parse_architecture() {
        let src = "architecture rtl of test is begin end architecture;";
        let nodes = parse_vhdl(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let VhdlNode::Architecture { name, entity_name, .. } = &nodes[0] {
            assert_eq!(name, "rtl");
            assert_eq!(entity_name, "test");
        }
    }

    #[test]
    fn test_parse_signal_decl() {
        let src = "architecture rtl of test is signal a : bit; begin end architecture;";
        let nodes = parse_vhdl(src).unwrap();
        if let VhdlNode::Architecture { declarative_items, .. } = &nodes[0] {
            assert_eq!(declarative_items.len(), 1);
            if let VhdlDeclItem::Signal(sig) = &declarative_items[0] {
                assert_eq!(sig.name, "a");
                assert_eq!(sig.type_name, "bit");
            }
        }
    }

    #[test]
    fn test_parse_process() {
        let src = "architecture rtl of test is begin process (clk) begin end process; end architecture;";
        let nodes = parse_vhdl(src).unwrap();
        if let VhdlNode::Architecture { statements, .. } = &nodes[0] {
            assert_eq!(statements.len(), 1);
            if let VhdlStmt::Process { sensitivity, .. } = &statements[0] {
                assert_eq!(sensitivity.len(), 1);
                assert_eq!(sensitivity[0], "clk");
            }
        }
    }

    #[test]
    fn test_parse_if_stmt() {
        let src = "architecture rtl of test is begin process begin if a = '1' then null; end if; end process; end architecture;";
        let nodes = parse_vhdl(src).unwrap();
        if let VhdlNode::Architecture { statements, .. } = &nodes[0] {
            if let VhdlStmt::Process { stmts, .. } = &statements[0] {
                assert_eq!(stmts.len(), 1);
                if let VhdlStmt::If { condition, .. } = &stmts[0] {
                    assert!(!condition.is_empty());
                }
            }
        }
    }

    #[test]
    fn test_parse_case_stmt() {
        let src = "architecture rtl of test is begin process begin case sel is when '0' => null; when others => null; end case; end process; end architecture;";
        let nodes = parse_vhdl(src).unwrap();
        if let VhdlNode::Architecture { statements, .. } = &nodes[0] {
            if let VhdlStmt::Process { stmts, .. } = &statements[0] {
                assert_eq!(stmts.len(), 1);
                if let VhdlStmt::Case { alternatives, .. } = &stmts[0] {
                    assert_eq!(alternatives.len(), 2);
                }
            }
        }
    }

    #[test]
    fn test_parse_component_inst() {
        let src = "architecture rtl of test is begin u1: entity work.and2 port map (a => in1, b => in2, y => out1); end architecture;";
        let nodes = parse_vhdl(src).unwrap();
        if let VhdlNode::Architecture { statements, .. } = &nodes[0] {
            assert_eq!(statements.len(), 1);
            if let VhdlStmt::Instance { label, component_name, .. } = &statements[0] {
                assert_eq!(label, "u1");
                assert_eq!(component_name, "and2");
            }
        }
    }

    #[test]
    fn test_parse_package() {
        let src = "package my_pkg is constant C : integer := 5; end package;";
        let nodes = parse_vhdl(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let VhdlNode::Package { name, items, .. } = &nodes[0] {
            assert_eq!(name, "my_pkg");
            assert_eq!(items.len(), 1);
        }
    }

    #[test]
    fn test_parse_multiple_entities() {
        let src = "entity e1 is end entity; entity e2 is end entity;";
        let nodes = parse_vhdl(src).unwrap();
        assert_eq!(nodes.len(), 2);
    }
}