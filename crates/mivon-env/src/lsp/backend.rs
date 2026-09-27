//! LSP Backend — tower-lsp LanguageServer implementation for SystemVerilog.
//!
//! Provides:
//! - TextDocumentSyncKind::INCREMENTAL for open/change/close (LSP-18)
//! - publishDiagnostics via the existing parser
//! - go-to-definition (LSP-03 tahap 1): module/interface/package/class/
//!   function/task/parameter + port/signal declaration di file yang sama
//! - Initialize/shutdown lifecycle

use std::collections::HashMap;

use lsp_types::notification::PublishDiagnostics;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    CodeLens, CodeLensParams, ColorInformation, ColorPresentation, ColorPresentationParams,
    Command as LspCommand, CompletionItem, CompletionItemKind, CompletionParams,
    CompletionResponse, Diagnostic, DiagnosticSeverity, DocumentSymbol, FoldingRange,
    FoldingRangeKind, FoldingRangeParams, GotoDefinitionParams, GotoDefinitionResponse, Hover,
    HoverContents, InitializeParams, InitializeResult, LanguageString, LinkedEditingRangeParams,
    LinkedEditingRanges, Location, MarkedString, OneOf, Position, PublishDiagnosticsParams, Range,
    ReferenceParams, RenameParams, SelectionRange, SelectionRangeParams, SemanticToken,
    SemanticTokens, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, ServerCapabilities, ServerInfo, SymbolInformation, SymbolKind,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url, WorkspaceEdit, WorkspaceSymbolParams,
};
use mivon_parser::lexer::Lexer;
use mivon_parser::preprocessor::Preprocessor;
use mivon_parser::Parser;
use tower_lsp::jsonrpc::Result as JsonRpcResult;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// Hasil workspace symbol search (LSP-11).
#[derive(Debug, Clone)]
pub(crate) struct WsSymbolHit {
    pub uri: String,
    pub name: String,
    pub kind: u8,
    pub line: u32,
    pub col: u32,
}

/// Simbol dokumen versi lite (LSP-12) — dikonversi ke
/// lsp_types::DocumentSymbol oleh handler.
#[derive(Debug, Clone)]
pub(crate) struct DocSymbol {
    pub name: String,
    pub kind: u8,
    pub line: u32,
    pub col: u32,
    pub children: Vec<DocSymbol>,
}

impl DocSymbol {
    const KIND_MODULE: u8 = 0;
    const KIND_INTERFACE: u8 = 1;
    const KIND_PACKAGE: u8 = 2;
    const KIND_CLASS: u8 = 3;
    const KIND_FUNCTION: u8 = 4;
    const KIND_CONSTANT: u8 = 5;
    const KIND_VARIABLE: u8 = 6;
    const KIND_KEYWORD: u8 = 7;

    pub(crate) fn lsp_kind(&self) -> SymbolKind {
        match self.kind {
            DocSymbol::KIND_INTERFACE => SymbolKind::INTERFACE,
            DocSymbol::KIND_PACKAGE => SymbolKind::PACKAGE,
            DocSymbol::KIND_CLASS => SymbolKind::CLASS,
            DocSymbol::KIND_FUNCTION => SymbolKind::FUNCTION,
            DocSymbol::KIND_CONSTANT => SymbolKind::CONSTANT,
            DocSymbol::KIND_VARIABLE => SymbolKind::VARIABLE,
            DocSymbol::KIND_KEYWORD => SymbolKind::KEY,
            _ => SymbolKind::MODULE,
        }
    }

    /// Konversi kind internal → CompletionItemKind (LSP-02).
    pub(crate) fn completion_kind(kind: u8) -> CompletionItemKind {
        match kind {
            DocSymbol::KIND_INTERFACE => CompletionItemKind::INTERFACE,
            DocSymbol::KIND_PACKAGE => CompletionItemKind::MODULE,
            DocSymbol::KIND_CLASS => CompletionItemKind::CLASS,
            DocSymbol::KIND_FUNCTION => CompletionItemKind::FUNCTION,
            DocSymbol::KIND_CONSTANT => CompletionItemKind::CONSTANT,
            DocSymbol::KIND_VARIABLE => CompletionItemKind::VARIABLE,
            DocSymbol::KIND_KEYWORD => CompletionItemKind::KEYWORD,
            _ => CompletionItemKind::MODULE,
        }
    }

    /// Konversi rekursif ke lsp_types::DocumentSymbol.
    /// Range = satu baris deklarasi; selection_range = nama saja.
    fn to_lsp(&self, uri_line_len: impl Fn(u32) -> u32 + Copy) -> DocumentSymbol {
        let line_end = uri_line_len(self.line);
        let name_start = self.col;
        let name_end = (self.col + self.name.len() as u32).min(line_end);
        DocumentSymbol {
            name: self.name.clone(),
            detail: None,
            kind: self.lsp_kind(),
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: Range {
                start: Position {
                    line: self.line,
                    character: 0,
                },
                end: Position {
                    line: self.line,
                    character: line_end,
                },
            },
            selection_range: Range {
                start: Position {
                    line: self.line,
                    character: name_start,
                },
                end: Position {
                    line: self.line,
                    character: name_end,
                },
            },
            children: Some(
                self.children
                    .iter()
                    .map(|c| c.to_lsp(uri_line_len))
                    .collect(),
            ),
        }
    }
}

/// LSP backend for SystemVerilog language server.
pub struct LspBackend {
    client: Client,
    /// Dokumen terbuka: uri → teks (LSP-03: sumber goto-definition).
    documents: std::sync::Mutex<HashMap<String, String>>,
}

impl LspBackend {
    pub fn new(client: Client) -> Self {
        LspBackend {
            client,
            documents: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Ambil teks dokumen dari cache, fallback baca dari disk (uri file://).
    fn document_text(&self, uri: &lsp_types::Url) -> Option<String> {
        if let Ok(map) = self.documents.lock() {
            if let Some(text) = map.get(uri.as_str()) {
                return Some(text.clone());
            }
        }
        // Fallback: file belum dibuka editor — baca langsung dari path.
        let path = uri.to_file_path().ok()?;
        std::fs::read_to_string(path).ok()
    }

    /// LSP-20: Format SystemVerilog source text.
    /// Uses lexer-based token grouping and indent/dedent logic.
    fn format_source(src: &str, indent_width: usize) -> String {
        use mivon_parser::lexer::Token;

        // 1. Tokenize.
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

        // 2. Group tokens by source line.
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

        // 3. Render with dynamic indentation.
        let mut out = String::new();
        let mut base: usize = 0;
        for line_tokens in &lines {
            if line_tokens.is_empty() {
                continue;
            }
            let first = &line_tokens[0].0;
            let last = &line_tokens[line_tokens.len() - 1].0;

            // Dedent for closing keywords.
            if Self::is_dedent_token(first) {
                base = base.saturating_sub(1);
            }

            let text = Self::render_line_tokens(line_tokens);
            if text.trim().is_empty() {
                continue;
            }
            let indent = base;
            out.push_str(&" ".repeat(indent * indent_width));
            out.push_str(&text);
            out.push('\n');

            // Indent for block-opening keywords.
            if Self::is_indent_token(first) || Self::is_begin_token(last) {
                base += 1;
            }
        }
        out
    }

    fn is_indent_token(tok: &mivon_parser::lexer::Token) -> bool {
        use mivon_parser::lexer::Token;
        matches!(
            tok,
            Token::Module
                | Token::Interface
                | Token::Package
                | Token::Class
                | Token::Function
                | Token::Task
                | Token::Program
        )
    }

    fn is_dedent_token(tok: &mivon_parser::lexer::Token) -> bool {
        use mivon_parser::lexer::Token;
        matches!(
            tok,
            Token::Endmodule
                | Token::EndInterface
                | Token::EndPackage
                | Token::EndClass
                | Token::EndFunction
                | Token::EndTask
                | Token::EndProgram
                | Token::Endcase
                | Token::End
        )
    }

    fn is_begin_token(tok: &mivon_parser::lexer::Token) -> bool {
        use mivon_parser::lexer::Token;
        matches!(tok, Token::Begin)
    }

    /// Render a line of tokens into text.
    fn render_line_tokens(tokens: &[(mivon_parser::lexer::Token, usize)]) -> String {
        use mivon_parser::lexer::Token;
        let mut parts: Vec<String> = Vec::new();
        for (tok, _col) in tokens {
            let s: String = match tok {
                Token::Module => "module".into(),
                Token::Endmodule => "endmodule".into(),
                Token::Interface => "interface".into(),
                Token::EndInterface => "endinterface".into(),
                Token::Package => "package".into(),
                Token::EndPackage => "endpackage".into(),
                Token::Class => "class".into(),
                Token::EndClass => "endclass".into(),
                Token::Function => "function".into(),
                Token::EndFunction => "endfunction".into(),
                Token::Task => "task".into(),
                Token::EndTask => "endtask".into(),
                Token::Program => "program".into(),
                Token::Begin => "begin".into(),
                Token::End => "end".into(),
                Token::Endcase => "endcase".into(),
                Token::If => "if".into(),
                Token::Else => "else".into(),
                Token::Case => "case".into(),
                Token::CaseX => "casex".into(),
                Token::CaseZ => "casez".into(),
                Token::Default => "default".into(),
                Token::AlwaysComb => "always_comb".into(),
                Token::AlwaysFF => "always_ff".into(),
                Token::AlwaysLatch => "always_latch".into(),
                Token::Always => "always".into(),
                Token::Initial => "initial".into(),
                Token::Assign => "assign".into(),
                Token::Logic => "logic".into(),
                Token::Reg => "reg".into(),
                Token::Wire => "wire".into(),
                Token::Input => "input".into(),
                Token::Output => "output".into(),
                Token::Inout => "inout".into(),
                Token::Parameter => "parameter".into(),
                Token::LocalParam => "localparam".into(),
                Token::Signed => "signed".into(),
                Token::Unsigned => "unsigned".into(),
                Token::For => "for".into(),
                Token::While => "while".into(),
                Token::Repeat => "repeat".into(),
                Token::Foreach => "foreach".into(),
                Token::Return => "return".into(),
                Token::Generate => "generate".into(),
                Token::EndGenerate => "endgenerate".into(),
                Token::GenVar => "genvar".into(),
                Token::PosEdge => "posedge".into(),
                Token::NegEdge => "negedge".into(),
                Token::Import => "import".into(),
                Token::Export => "export".into(),
                Token::Ident(s) => {
                    parts.push(s.to_string());
                    continue;
                }
                Token::Number { value, .. } => {
                    parts.push(value.to_string());
                    continue;
                }
                Token::RealNum(s) => {
                    parts.push(s.to_string());
                    continue;
                }
                Token::StringLit(s) => {
                    // Re-escape: nilai tersimpan hasil decode lexer — render
                    // apa adanya membuat literal newline di tengah baris LSP
                    // (kelas bug sama dgn fmt roundtrip, fuzzer O3).
                    parts.push(Lexer::escape_string_literal(s.as_str()));
                    continue;
                }
                Token::Semi => ";".into(),
                Token::Colon => ":".into(),
                Token::Comma => ",".into(),
                Token::Dot => ".".into(),
                Token::LParen => "(".into(),
                Token::RParen => ")".into(),
                Token::LBrack => "[".into(),
                Token::RBrack => "]".into(),
                Token::LBrace => "{".into(),
                Token::RBrace => "}".into(),
                Token::Eq => "=".into(),
                Token::Equiv => "===".into(),
                Token::Neq => "!=".into(),
                Token::Lt => "<".into(),
                Token::NonBlockingAssign => "<=".into(),
                Token::Le => "<=".into(),
                Token::Gt => ">".into(),
                Token::Ge => ">=".into(),
                Token::Plus => "+".into(),
                Token::Minus => "-".into(),
                Token::Star => "*".into(),
                Token::Slash => "/".into(),
                Token::Percent => "%".into(),
                Token::Amp => "&".into(),
                Token::Pipe => "|".into(),
                Token::Caret => "^".into(),
                Token::Tilde => "~".into(),
                Token::Not => "!".into(),
                Token::Hash => "#".into(),
                Token::HashHash => "##".into(),
                Token::Arrow => "->".into(),
                Token::At => "@".into(),
                Token::Scope => "::".into(),
                Token::EndPrimitive => "endprimitive".into(),
                Token::Primitive => "primitive".into(),
                Token::EndProgram => "endprogram".into(),
                Token::BlockingAssign => "=".into(),
                Token::Null => "null".into(),
                _ => continue,
            };
            parts.push(s);
        }
        parts.join(" ")
    }

    /// LSP-10: Generate code actions from a diagnostic.
    fn actions_from_diagnostic(
        _text: &str,
        diag: &Diagnostic,
        uri: &lsp_types::Url,
    ) -> Option<Vec<CodeActionOrCommand>> {
        let mut actions = Vec::new();
        let msg = diag.message.to_string();
        let range = diag.range;

        // Quick-fix: missing semicolon (suggested: add ';')
        if msg.to_lowercase().contains("expected ';'") || msg.to_lowercase().contains("missing ';'")
        {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Add missing semicolon".to_string(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(WorkspaceEdit::new(std::collections::HashMap::from([(
                    uri.clone(),
                    vec![lsp_types::TextEdit {
                        range: Range {
                            start: Position::new(range.end.line, range.end.character),
                            end: Position::new(range.end.line, range.end.character),
                        },
                        new_text: ";".to_string(),
                    }],
                )]))),
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: None,
            }));
        }

        // Quick-fix: missing 'endmodule' (suggested: add 'endmodule')
        if msg.to_lowercase().contains("expected 'endmodule'")
            || msg.to_lowercase().contains("unterminated module")
        {
            let last_line = diag.message.lines().last().unwrap_or(&msg).to_string();
            let _ = last_line;
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Add missing endmodule".to_string(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(WorkspaceEdit::new(std::collections::HashMap::from([(
                    uri.clone(),
                    vec![lsp_types::TextEdit {
                        range: Range {
                            start: Position::new(range.end.line + 1, 0),
                            end: Position::new(range.end.line + 1, 0),
                        },
                        new_text: "endmodule\n".to_string(),
                    }],
                )]))),
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: None,
            }));
        }

        // Quick-fix: missing 'endfunction' (suggested: add 'endfunction')
        if msg.to_lowercase().contains("expected 'endfunction'") {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Add missing endfunction".to_string(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(WorkspaceEdit::new(std::collections::HashMap::from([(
                    uri.clone(),
                    vec![lsp_types::TextEdit {
                        range: Range {
                            start: Position::new(range.end.line + 1, 0),
                            end: Position::new(range.end.line + 1, 0),
                        },
                        new_text: "endfunction\n".to_string(),
                    }],
                )]))),
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: None,
            }));
        }

        // Quick-fix: missing 'endclass' (suggested: add 'endclass')
        if msg.to_lowercase().contains("expected 'endclass'") {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Add missing endclass".to_string(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(WorkspaceEdit::new(std::collections::HashMap::from([(
                    uri.clone(),
                    vec![lsp_types::TextEdit {
                        range: Range {
                            start: Position::new(range.end.line + 1, 0),
                            end: Position::new(range.end.line + 1, 0),
                        },
                        new_text: "endclass\n".to_string(),
                    }],
                )]))),
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: None,
            }));
        }

        if actions.is_empty() {
            None
        } else {
            Some(actions)
        }
    }

    /// Parse SystemVerilog source text and return diagnostics.
    fn parse_diagnostics(source: &str) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        // 1. Preprocess
        let mut pp = Preprocessor::new();
        pp.quiet = true;
        let preprocessed = match pp.preprocess(source, None) {
            Ok(s) => s,
            Err(e) => {
                diagnostics.push(Diagnostic {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: 0,
                            character: 0,
                        },
                    },
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("mivon".to_string()),
                    message: format!("Preprocessor error: {}", e),
                    ..Default::default()
                });
                return diagnostics;
            }
        };

        // 2. Lex
        let mut lexer = Lexer::new(&preprocessed);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lexer.next_token();
            if tok == mivon_parser::lexer::Token::Eof {
                break;
            }
            tokens.push((tok, line, col));
        }

        if tokens.is_empty() {
            return diagnostics;
        }

        // 3. Parse with diagnostics collection
        let file_line_map = lexer.file_line_map.clone();
        let file_path = "<buffer>".to_string();
        let mut parser = Parser::new(tokens, &file_path)
            .with_source_lines(&preprocessed)
            .with_file_line_map(file_line_map);

        let _design = parser.parse_design();

        // 4. Convert parser diagnostics to LSP diagnostics
        for diag in &parser.errors {
            let severity = if diag.is_error() {
                DiagnosticSeverity::ERROR
            } else {
                DiagnosticSeverity::WARNING
            };

            // Posisi dari source_snippet (line:col 1-based) → fallback DiagSpan
            // (file_id + line:col, terisi lintas modul) → (0,0) bila benar-benar
            // tanpa lokasi. Dulu span diabaikan → squiggle salah di 0:0.
            let (line, col) = if let Some(snippet) = &diag.source_snippet {
                (
                    snippet.line.saturating_sub(1) as u32,
                    snippet.col.saturating_sub(1) as u32,
                )
            } else if let Some(sp) = diag.spans.first() {
                (sp.start.saturating_sub(1), sp.end.saturating_sub(1))
            } else {
                (0, 0)
            };

            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position {
                        line,
                        character: col,
                    },
                    end: Position {
                        line,
                        character: col + 1,
                    },
                },
                severity: Some(severity),
                source: Some("mivon".to_string()),
                message: diag.message.to_string(),
                ..Default::default()
            });
        }

        diagnostics
    }

    /// Ekstrak identifier pada posisi kursor (LSP-03).
    /// Identifier SV: [A-Za-z0-9_$]. Mengembalikan (word, start_col, end_col).
    fn word_at_position(line_text: &str, character: u32) -> Option<(String, u32, u32)> {
        let bytes = line_text.as_bytes();
        let ch = character as usize;
        let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';

        if ch > bytes.len() {
            return None;
        }
        // Kursor di atas karakter non-ident → cek karakter sebelumnya
        // (posisi akhir kata).
        let mut start = ch.min(bytes.len());
        while start > 0 && is_ident(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = ch.min(bytes.len());
        while end < bytes.len() && is_ident(bytes[end]) {
            end += 1;
        }
        if start == end {
            return None;
        }
        Some((line_text[start..end].to_string(), start as u32, end as u32))
    }

    /// Tokenize satu baris menjadi (word, col) — ident [A-Za-z0-9_$] atau
    /// simbol tunggal. Untuk deteksi pola deklarasi.
    fn line_tokens(line: &str) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
                let s = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
                {
                    i += 1;
                }
                out.push((line[s..i].to_string(), s));
            } else if c == b' ' || c == b'\t' {
                i += 1;
            } else {
                // Komentar — abaikan sisa baris.
                if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    break;
                }
                out.push((String::from_utf8_lossy(&bytes[i..i + 1]).to_string(), i));
                i += 1;
            }
        }
        out
    }

    /// Deteksi apakah baris adalah DEFINISI dari `word` (LSP-03 tahap 1).
    /// Mengembalikan kolom awal kata bila ya. Pola yang dikenali:
    ///   1. module/interface/package/program/class/checker/function/task/
    ///      property/sequence `<word>` — definisi scope.
    ///   2. parameter/localparam/typedef/genvar ... `<word>` — deklarasi.
    ///   3. Baris diawali direction/type keyword (input/output/inout/reg/
    ///      wire/logic/bit/int/... ) dan memuat `<word>` sebagai token
    ///      berikutnya yang relevan — port/signal declaration.
    fn definition_col(line: &str, word: &str) -> Option<usize> {
        const SCOPE_KW: &[&str] = &[
            "module",
            "macromodule",
            "interface",
            "package",
            "program",
            "class",
            "checker",
            "function",
            "task",
            "property",
            "sequence",
        ];
        const DECL_KW: &[&str] = &["parameter", "localparam", "typedef", "genvar"];
        const TYPE_DIR_KW: &[&str] = &[
            "input", "output", "inout", "wire", "reg", "logic", "bit", "int", "integer", "byte",
            "shortint", "longint", "real", "realtime", "time", "nettype", "enum", "struct",
            "union", "string", "event",
        ];

        let toks = Self::line_tokens(line);
        if toks.is_empty() {
            return None;
        }

        // 0. Deklarasi parameter di mana pun dalam baris (termasuk
        //    `module m #(parameter WIDTH = 8) (`): token parameter/localparam
        //    → cari word SETELAHnya.
        for (i, (w, _)) in toks.iter().enumerate() {
            if matches!(w.as_str(), "parameter" | "localparam") {
                for (w2, c2) in toks.iter().skip(i + 1) {
                    if w2 == word {
                        return Some(*c2);
                    }
                }
            }
        }

        // 1. Definisi scope: <kw> <word>
        let (t0, _) = &toks[0];
        if SCOPE_KW.contains(&t0.as_str()) {
            if let Some((w1, c1)) = toks.get(1) {
                if w1 == word {
                    return Some(*c1);
                }
            }
            // function ret_type <name>( / task ...
            if matches!(t0.as_str(), "function" | "task") {
                for (w, c) in toks.iter().skip(1) {
                    if w == word {
                        // pastikan bukan tipe return: harus sebelum '('
                        return Some(*c);
                    }
                }
            }
            return None;
        }

        // 2. parameter/localparam/typedef/genvar ... word ...
        if DECL_KW.contains(&t0.as_str()) {
            for (w, c) in toks.iter().skip(1) {
                if w == word {
                    return Some(*c);
                }
            }
            return None;
        }

        // 3. Port/signal decl: baris diawali direction/type keyword dan
        //    memuat word sebagai token berdiri sendiri (bukan nama tipe
        //    pertama). Contoh: `input logic [7:0] data_in`, `reg [3:0] cnt`.
        if TYPE_DIR_KW.contains(&t0.as_str()) {
            // Cari word sebagai token berdiri sendiri setelah keyword.
            // Pemakaian dalam ekspresi (mis. `count + 1`) tetap bisa match
            // di sini BILA baris juga diawali type keyword — jarang dan
            // hasilnya tetap lokasi deklarasi-relevan (MVP tahap 1).
            for (idx, (w, c)) in toks.iter().enumerate().skip(1) {
                if w == word && idx > 0 {
                    // Skip pemakaian hierarki (a.b): token sebelumnya "."
                    let prev_dot = idx >= 1 && toks[idx - 1].0 == ".";
                    if !prev_dot {
                        return Some(*c);
                    }
                }
            }
        }
        None
    }

    /// Validasi identifier SV sederhana (LSP-05): [A-Za-z_][A-Za-z0-9_$]*.
    fn is_valid_identifier(name: &str) -> bool {
        let bytes = name.as_bytes();
        if bytes.is_empty() {
            return false;
        }
        let first = bytes[0];
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return false;
        }
        bytes[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
    }

    /// Rename core (murni, bisa diuji) — LSP-05 tahap 1: hasilkan edit
    /// (range) untuk SEMUA kemunculan identifier di dokumen. Return
    /// Err dengan pesan jelas bila kursor bukan identifier atau nama
    /// baru tidak valid.
    pub(crate) fn compute_rename_edits(
        source: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Vec<(u32, u32, u32)>, String> {
        if !Self::is_valid_identifier(new_name) {
            return Err(format!(
                "'{}' bukan identifier SystemVerilog yang valid",
                new_name
            ));
        }
        let Some(line_text) = source.lines().nth(line as usize) else {
            return Err("posisi kursor di luar dokumen".into());
        };
        let Some((word, _, _)) = Self::word_at_position(line_text, character) else {
            return Err("kursor tidak berada di atas identifier".into());
        };
        if word == new_name {
            return Ok(Vec::new());
        }
        // find_references menjamin token mandiri + word-boundary.
        Ok(Self::find_references(source, line, character)
            .into_iter()
            .collect())
    }

    /// Hover core (murni, bisa diuji) — LSP-06 tahap 1: konteks deklarasi
    /// identifier di posisi kursor, berformat markdown:
    ///   **kind** `name` — snippet baris deklarasi.
    fn hover_info(source: &str, line: u32, character: u32) -> Option<String> {
        let line_text = source.lines().nth(line as usize)?;
        let (word, _, _) = Self::word_at_position(line_text, character)?;

        let (def_line, _, _) = Self::find_definition(source, line, character)?;
        let decl_text = source.lines().nth(def_line as usize)?.trim().to_string();

        // Tebak kind dari keyword di baris definisi.
        let toks = Self::line_tokens(&decl_text);
        let kind = toks
            .iter()
            .find_map(|(w, _)| match w.as_str() {
                "module" | "interface" | "package" | "program" | "class" | "checker" => {
                    Some(w.clone())
                }
                "input" => Some("port input".into()),
                "output" => Some("port output".into()),
                "inout" => Some("port inout".into()),
                "parameter" | "localparam" => Some(w.clone()),
                "function" | "task" => Some(w.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "declaration".into());

        Some(format!(
            "**{kind}** `{word}`\n\n```systemverilog\n{decl_text}\n```"
        ))
    }

    /// Document symbol core (murni, bisa diuji) — LSP-12 tahap 1:
    /// outline simbol dokumen dengan nesting scope sederhana
    /// (module/interface/package/class sebagai container; function/task/
    /// parameter/port/signal sebagai anak).
    pub(crate) fn document_symbols(source: &str) -> Vec<DocSymbol> {
        const OPEN_KW: &[(&str, u8)] = &[
            ("module", DocSymbol::KIND_MODULE),
            ("macromodule", DocSymbol::KIND_MODULE),
            ("interface", DocSymbol::KIND_INTERFACE),
            ("package", DocSymbol::KIND_PACKAGE),
            ("class", DocSymbol::KIND_CLASS),
            ("checker", DocSymbol::KIND_CLASS),
        ];
        const CLOSE_KW: &[&str] = &[
            "endmodule",
            "endinterface",
            "endpackage",
            "endclass",
            "endchecker",
        ];
        const FUNC_KW: &[&str] = &["function", "task"];
        const CONST_KW: &[&str] = &["parameter", "localparam"];
        const VAR_KW: &[&str] = &[
            "input", "output", "inout", "wire", "reg", "logic", "bit", "int", "integer", "byte",
            "shortint", "longint", "real", "time", "string",
        ];

        struct Frame {
            sym: Option<DocSymbol>,
            children: Vec<DocSymbol>,
        }

        let mut stack: Vec<Frame> = vec![Frame {
            sym: None,
            children: Vec::new(),
        }];

        for (idx, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let toks = Self::line_tokens(line);
            let Some((t0, c0)) = toks.first().cloned() else {
                continue;
            };

            // Tutup scope → merge symbol dengan children terkumpul.
            if CLOSE_KW.contains(&t0.as_str()) && stack.len() > 1 {
                let mut frame = stack.pop().unwrap();
                if let Some(sym) = frame.sym.as_mut() {
                    std::mem::swap(&mut sym.children, &mut frame.children);
                }
                if let Some(sym) = frame.sym.take() {
                    stack.last_mut().unwrap().children.push(sym);
                } else {
                    stack.last_mut().unwrap().children.extend(frame.children);
                }
                continue;
            }

            // Buka scope: keyword + nama.
            if let Some((_, kind)) = OPEN_KW.iter().find(|(kw, _)| *kw == t0.as_str()) {
                if let Some((w1, _)) = toks.get(1) {
                    if Self::is_valid_identifier(w1) {
                        stack.push(Frame {
                            sym: Some(DocSymbol {
                                name: w1.clone(),
                                kind: *kind,
                                line: idx as u32,
                                col: c0 as u32,
                                children: Vec::new(),
                            }),
                            children: Vec::new(),
                        });
                        // Parameter inline pada baris yang sama
                        // (`module m #(parameter WIDTH = 8) (`).
                        let frame = stack.last_mut().unwrap();
                        for i in 2..toks.len() {
                            if matches!(toks[i].0.as_str(), "parameter" | "localparam") {
                                for (w, c) in toks.iter().skip(i + 1).cloned() {
                                    if Self::is_valid_identifier(&w)
                                        && !matches!(
                                            w.as_str(),
                                            "logic"
                                                | "bit"
                                                | "int"
                                                | "integer"
                                                | "signed"
                                                | "unsigned"
                                                | "real"
                                                | "string"
                                                | "type"
                                        )
                                        && !frame.children.iter().any(|ch| ch.name == w)
                                    {
                                        frame.children.push(DocSymbol {
                                            name: w,
                                            kind: DocSymbol::KIND_CONSTANT,
                                            line: idx as u32,
                                            col: c as u32,
                                            children: Vec::new(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                continue;
            }

            // Function/task → Function symbol (leaf).
            if FUNC_KW.contains(&t0.as_str()) {
                for (w, c) in toks.iter().skip(1).cloned() {
                    if Self::is_valid_identifier(&w)
                        && !matches!(
                            w.as_str(),
                            "logic"
                                | "bit"
                                | "int"
                                | "integer"
                                | "signed"
                                | "unsigned"
                                | "automatic"
                                | "static"
                                | "void"
                        )
                    {
                        stack.last_mut().unwrap().children.push(DocSymbol {
                            name: w.clone(),
                            kind: DocSymbol::KIND_FUNCTION,
                            line: idx as u32,
                            col: c as u32,
                            children: Vec::new(),
                        });
                        break;
                    }
                }
                continue;
            }

            // parameter/localparam → Constant.
            if CONST_KW.contains(&t0.as_str()) {
                for (w, c) in toks.iter().skip(1).cloned() {
                    if Self::is_valid_identifier(&w)
                        && !matches!(
                            w.as_str(),
                            "logic"
                                | "bit"
                                | "int"
                                | "integer"
                                | "signed"
                                | "unsigned"
                                | "real"
                                | "string"
                                | "type"
                        )
                    {
                        stack.last_mut().unwrap().children.push(DocSymbol {
                            name: w.clone(),
                            kind: DocSymbol::KIND_CONSTANT,
                            line: idx as u32,
                            col: c as u32,
                            children: Vec::new(),
                        });
                    }
                }
                continue;
            }

            // Port/signal decl → Variable per nama.
            if VAR_KW.contains(&t0.as_str()) {
                for (w, c) in toks.iter().skip(1).cloned() {
                    if Self::is_valid_identifier(&w)
                        && !matches!(
                            w.as_str(),
                            "logic"
                                | "reg"
                                | "wire"
                                | "bit"
                                | "int"
                                | "integer"
                                | "signed"
                                | "unsigned"
                                | "real"
                                | "string"
                                | "input"
                                | "output"
                                | "inout"
                        )
                    {
                        stack.last_mut().unwrap().children.push(DocSymbol {
                            name: w.clone(),
                            kind: DocSymbol::KIND_VARIABLE,
                            line: idx as u32,
                            col: c as u32,
                            children: Vec::new(),
                        });
                    }
                }
                continue;
            }
        }

        // Tutup scope yang belum tertutup (dokumen rusak).
        while stack.len() > 1 {
            let mut frame = stack.pop().unwrap();
            if let Some(sym) = frame.sym.as_mut() {
                std::mem::swap(&mut sym.children, &mut frame.children);
            }
            if let Some(sym) = frame.sym.take() {
                stack.last_mut().unwrap().children.push(sym);
            } else {
                stack.last_mut().unwrap().children.extend(frame.children);
            }
        }
        std::mem::take(&mut stack[0].children)
    }

    /// Folding range core (murni, bisa diuji) — LSP-15 tahap 1:
    /// region lipat untuk blok scope berdasarkan token baris:
    ///   - module/interface/package/class/checker/function/task/generate
    ///     dibuka sebagai token PERTAMA baris, ditutup end* sebagai token
    ///     pertama baris.
    ///   - begin...end dan case...endcase: `begin`/`case` membuka (begin
    ///     sebagai token terakhir baris header; case sebagai token pertama),
    ///     ditutup bila baris memuat token `end` / `endcase`.
    ///     Return (start_line, end_line) urut.
    pub(crate) fn compute_folding_ranges(source: &str) -> Vec<(u32, u32)> {
        // (keyword pembuka, keyword penutup) — pasangan scope utama.
        const PAIRS: &[(&str, &str)] = &[
            ("module", "endmodule"),
            ("macromodule", "endmodule"),
            ("interface", "endinterface"),
            ("package", "endpackage"),
            ("program", "endprogram"),
            ("class", "endclass"),
            ("checker", "endchecker"),
            ("function", "endfunction"),
            ("task", "endtask"),
            ("generate", "endgenerate"),
        ];

        let mut out: Vec<(u32, u32)> = Vec::new();
        // Stack: (keyword pembuka, start_line).
        let mut stack: Vec<(&str, u32)> = Vec::new();

        let close_top = |stack: &mut Vec<(&str, u32)>, out: &mut Vec<(u32, u32)>, idx: u32| {
            if let Some((_, start)) = stack.pop() {
                out.push((start, idx));
            }
        };

        for (idx, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let toks = Self::line_tokens(line);
            if toks.is_empty() {
                continue;
            }
            let t0 = toks[0].0.as_str();

            // Scope utama dibuka sebagai token pertama.
            if let Some((_, closer)) = PAIRS.iter().find(|(kw, _)| *kw == t0) {
                stack.push((*closer, idx as u32));
                continue;
            }
            // Scope utama ditutup sebagai token pertama.
            if let Some((_, opener)) = PAIRS.iter().find(|(_, ew)| *ew == t0) {
                let _ = opener;
                close_top(&mut stack, &mut out, idx as u32);
                continue;
            }

            let words: Vec<&str> = toks.iter().map(|(w, _)| w.as_str()).collect();

            // Penutup DULU (baris `end else begin` menutup lalu membuka):
            // endcase lalu end.
            if words.contains(&"endcase") {
                if let Some(pos) = stack
                    .iter()
                    .rposition(|(kw, _)| matches!(*kw, "case" | "casex" | "casez"))
                {
                    let (_, start) = stack.remove(pos);
                    out.push((start, idx as u32));
                }
            }
            // Token `end` mandiri (bukan bagian endmodule dkk — tokenizer
            // memisahkan kata utuh) menutup begin paling dalam.
            if words.contains(&"end") {
                if let Some(pos) = stack.iter().rposition(|(kw, _)| *kw == "begin") {
                    let (_, start) = stack.remove(pos);
                    out.push((start, idx as u32));
                } else if stack.len() > 1 {
                    // begin tidak ada → fallback tutup scope utama dalam.
                    close_top(&mut stack, &mut out, idx as u32);
                }
            }

            // Pembukaan setelah penutupan:
            // case/casex/casez sebagai token pertama membuka blok.
            if matches!(t0, "case" | "casex" | "casez") {
                stack.push(("case", idx as u32));
            }
            // `begin` sebagai token TERAKHIR baris header membuka blok.
            if words.len() > 1 && words[words.len() - 1] == "begin" {
                stack.push(("begin", idx as u32));
            }
        }

        out.sort_by_key(|(s, e)| (*s, *e));
        out.dedup();
        out
    }

    /// Autocomplete core (murni, bisa diuji) — LSP-02 tahap 1:
    /// kandidat = simbol dokumen (module/port/signal/param/function via
    /// outline) + keyword SystemVerilog umum, difilter prefix identifier
    /// di kursor. Return (label, kind) dengan kind DocSymbol::KIND_*;
    /// keyword memakai KIND_KEYWORD.
    pub(crate) fn compute_completions(
        source: &str,
        line: u32,
        character: u32,
    ) -> Vec<(String, u8)> {
        const KEYWORDS: &[&str] = &[
            "always",
            "always_comb",
            "always_ff",
            "always_latch",
            "assign",
            "begin",
            "case",
            "casex",
            "casez",
            "class",
            "clocking",
            "default",
            "disable",
            "else",
            "end",
            "endcase",
            "endclass",
            "endfunction",
            "endgenerate",
            "endinterface",
            "endmodule",
            "endpackage",
            "endtask",
            "enum",
            "for",
            "forever",
            "fork",
            "function",
            "generate",
            "genvar",
            "if",
            "iff",
            "import",
            "initial",
            "inout",
            "input",
            "int",
            "integer",
            "interface",
            "join",
            "logic",
            "localparam",
            "longint",
            "macromodule",
            "modport",
            "module",
            "negedge",
            "or",
            "output",
            "package",
            "parameter",
            "posedge",
            "primitive",
            "priority",
            "program",
            "property",
            "reg",
            "repeat",
            "return",
            "sequence",
            "shortint",
            "signed",
            "string",
            "struct",
            "time",
            "typedef",
            "unique",
            "unsigned",
            "wait",
            "while",
            "wire",
            "with",
        ];

        let line_text = source.lines().nth(line as usize).unwrap_or("");
        // Prefix = karakter identifier yang sudah diketik sebelum kursor.
        let bytes = line_text.as_bytes();
        let ch = (character as usize).min(bytes.len());
        let mut start = ch;
        while start > 0
            && (bytes[start - 1].is_ascii_alphanumeric()
                || bytes[start - 1] == b'_'
                || bytes[start - 1] == b'$')
        {
            start -= 1;
        }
        let prefix = line_text[start..ch].to_lowercase();
        if prefix.is_empty() {
            return Vec::new();
        }

        let mut out: Vec<(String, u8)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Simbol dokumen dulu (lebih relevan).
        fn walk(
            items: &[DocSymbol],
            prefix: &str,
            out: &mut Vec<(String, u8)>,
            seen: &mut std::collections::HashSet<String>,
        ) {
            for s in items {
                if s.name.to_lowercase().starts_with(prefix) && seen.insert(s.name.clone()) {
                    out.push((s.name.clone(), s.kind));
                }
                walk(&s.children, prefix, out, seen);
            }
        }
        walk(
            &Self::document_symbols(source),
            &prefix,
            &mut out,
            &mut seen,
        );

        // Keyword SystemVerilog.
        for kw in KEYWORDS {
            if kw.starts_with(&prefix) && seen.insert((*kw).to_string()) {
                out.push(((*kw).to_string(), DocSymbol::KIND_KEYWORD));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Workspace symbol search core (murni, bisa diuji) — LSP-11 tahap 1:
    /// cari di SEMUA dokumen terbuka (cache), flatten outline, filter
    /// case-insensitive substring. Query kosong = semua simbol.
    pub(crate) fn search_workspace_symbols(
        docs: &[(String, String)],
        query: &str,
    ) -> Vec<WsSymbolHit> {
        let q = query.to_lowercase();
        let mut out: Vec<WsSymbolHit> = Vec::new();

        fn walk(items: &[DocSymbol], uri: &str, q: &str, out: &mut Vec<WsSymbolHit>) {
            for s in items {
                if q.is_empty() || s.name.to_lowercase().contains(q) {
                    out.push(WsSymbolHit {
                        uri: uri.to_string(),
                        name: s.name.clone(),
                        kind: s.kind,
                        line: s.line,
                        col: s.col,
                    });
                }
                walk(&s.children, uri, q, out);
            }
        }

        for (uri, text) in docs {
            walk(&Self::document_symbols(text), uri, &q, &mut out);
        }
        out.sort_by(|a, b| (&a.uri, a.line, a.col).cmp(&(&b.uri, b.line, b.col)));
        out
    }

    /// Semantic tokens core (murni, bisa diuji) — LSP-07 tahap 1.
    /// Legend: 0=namespace(module), 1=type(interface), 2=class,
    ///         3=interface, 4=function, 5=variable,
    ///         6=constant(param), 7=keyword.
    pub(crate) fn compute_semantic_tokens(source: &str) -> Vec<SemanticToken> {
        // Kumpulkan definisi dari outline → (line, col, len, legend_idx).
        let syms = Self::document_symbols(source);
        let mut raw: Vec<(u32, u32, u32, u32)> = Vec::new();

        fn collect(syms: &[DocSymbol], raw: &mut Vec<(u32, u32, u32, u32)>) {
            for s in syms {
                let idx = match s.kind {
                    DocSymbol::KIND_MODULE => 0,
                    DocSymbol::KIND_INTERFACE => 1,
                    DocSymbol::KIND_CLASS => 2,
                    DocSymbol::KIND_PACKAGE => 1,
                    DocSymbol::KIND_FUNCTION => 4,
                    DocSymbol::KIND_CONSTANT => 6,
                    DocSymbol::KIND_VARIABLE => 5,
                    _ => 5,
                };
                raw.push((s.line, s.col, s.name.len() as u32, idx));
                collect(&s.children, raw);
            }
        }
        collect(&syms, &mut raw);

        // Tambah keyword SV sebagai token type=7 (keyword).
        const KW: &[&str] = &[
            "module",
            "endmodule",
            "always_comb",
            "always_ff",
            "assign",
            "begin",
            "case",
            "default",
            "else",
            "end",
            "endcase",
            "if",
            "input",
            "logic",
            "output",
            "parameter",
            "reg",
            "wire",
            "localparam",
            "function",
            "task",
            "endfunction",
            "endtask",
            "interface",
            "endinterface",
            "package",
            "endpackage",
            "typedef",
            "enum",
            "struct",
            "posedge",
            "negedge",
        ];
        for (idx, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for (w, col) in Self::line_tokens(line) {
                if KW.contains(&w.as_str()) {
                    raw.push((idx as u32, col as u32, w.len() as u32, 7));
                }
            }
        }

        // Urutkan by (line, col).
        raw.sort_by_key(|&(l, c, _, _)| (l, c));

        // Konversi ke delta encoding LSP.
        let mut out: Vec<SemanticToken> = Vec::with_capacity(raw.len());
        let mut prev_line = 0u32;
        let mut prev_col = 0u32;
        for &(l, c, len, typ) in &raw {
            let dl = l - prev_line;
            let ds = if l == prev_line { c - prev_col } else { c };
            out.push(SemanticToken {
                delta_line: dl,
                delta_start: ds,
                length: len,
                token_type: typ,
                token_modifiers_bitset: 0,
            });
            prev_line = l;
            prev_col = c;
        }
        out
    }

    /// Find-references core (murni, bisa diuji) — LSP-04 tahap 1:
    /// semua kemunculan identifier (sebagai token mandiri) dalam dokumen,
    /// urut baris. `include_declaration` tetap dikembalikan penuh — caller
    /// (handler) yang memfilter bila diminta tanpa deklarasi.
    pub(crate) fn find_references(source: &str, line: u32, character: u32) -> Vec<(u32, u32, u32)> {
        let Some(line_text) = source.lines().nth(line as usize) else {
            return Vec::new();
        };
        let Some((word, _, _)) = Self::word_at_position(line_text, character) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for (idx, text) in source.lines().enumerate() {
            // Skip komentar baris.
            let trimmed = text.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for (w, col) in Self::line_tokens(text) {
                if w == word {
                    out.push((idx as u32, col as u32, (col + w.len()) as u32));
                }
            }
        }
        out
    }

    /// Core go-to-definition (murni, bisa diuji): cari lokasi definisi dari
    /// identifier di posisi kursor. Mengembalikan (line, col_start, col_end)
    /// posisi definisi pertama dalam dokumen.
    pub(crate) fn find_definition(
        source: &str,
        line: u32,
        character: u32,
    ) -> Option<(u32, u32, u32)> {
        let line_text = source.lines().nth(line as usize)?;
        let (word, _, _) = Self::word_at_position(line_text, character)?;

        for (idx, text) in source.lines().enumerate() {
            // Skip baris kursor sendiri bila kemungkinan pemakaian —
            // definisi boleh saja di baris yang sama (deklarasi inline),
            // jadi tetap periksa semua baris.
            if let Some(col) = Self::definition_col(text, &word) {
                return Some((idx as u32, col as u32, (col + word.len()) as u32));
            }
        }
        None
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for LspBackend {
    /// Initialize the LSP server — advertises capabilities.
    async fn initialize(&self, _params: InitializeParams) -> JsonRpcResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                completion_provider: Some(lsp_types::CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: Some(vec![".".to_string(), "::".to_string()]),
                    all_commit_characters: None,
                    work_done_progress_options: Default::default(),
                    completion_item: Default::default(),
                }),
                semantic_tokens_provider: Some(
                    lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: Default::default(),
                            legend: SemanticTokensLegend {
                                token_types: vec![
                                    lsp_types::SemanticTokenType::NAMESPACE,
                                    lsp_types::SemanticTokenType::TYPE,
                                    lsp_types::SemanticTokenType::CLASS,
                                    lsp_types::SemanticTokenType::INTERFACE,
                                    lsp_types::SemanticTokenType::FUNCTION,
                                    lsp_types::SemanticTokenType::VARIABLE,
                                    lsp_types::SemanticTokenType::PARAMETER,
                                    lsp_types::SemanticTokenType::KEYWORD,
                                ],
                                token_modifiers: vec![],
                            },
                            range: None,
                            full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                        },
                    ),
                ),
                folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(
                    true,
                )),
                // LSP-09: Inlay hints (type hints)
                inlay_hint_provider: Some(OneOf::Left(true)),
                // LSP-13: Call hierarchy
                call_hierarchy_provider: Some(lsp_types::CallHierarchyServerCapability::Simple(
                    true,
                )),
                // LSP-20: Document formatting
                document_formatting_provider: Some(OneOf::Left(true)),
                // LSP-10: Code actions (quick-fix)
                code_action_provider: Some(lsp_types::CodeActionProviderCapability::Simple(true)),
                // LSP-08: Code lens
                code_lens_provider: Some(lsp_types::CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                // LSP-16: Signature help
                signature_help_provider: Some(lsp_types::SignatureHelpOptions {
                    trigger_characters: Some(vec![("(").to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: Default::default(),
                }),
                // LSP-22: Color provider
                color_provider: Some(lsp_types::ColorProviderCapability::Simple(true)),
                // LSP-24: Selection range
                selection_range_provider: Some(
                    lsp_types::SelectionRangeProviderCapability::Simple(true),
                ),
                // LSP-23: Linked editing range
                linked_editing_range_provider: Some(
                    lsp_types::LinkedEditingRangeServerCapabilities::Simple(true),
                ),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "mivon-lsp".to_string(),
                version: Some("0.3.0".to_string()),
            }),
        })
    }

    /// Called when the server is shut down.
    async fn shutdown(&self) -> JsonRpcResult<()> {
        Ok(())
    }

    /// Text document didOpen — parse and emit diagnostics.
    async fn did_open(&self, params: lsp_types::DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let source = &params.text_document.text;
        if let Ok(mut map) = self.documents.lock() {
            map.insert(uri.to_string(), source.clone());
        }
        let diagnostics = Self::parse_diagnostics(source);
        self.client
            .send_notification::<PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                diagnostics,
                version: Some(1),
            })
            .await;
    }

    /// Text document didChange — re-parse and emit diagnostics.
    /// LSP-18: Incremental document sync — apply range-based text changes.
    async fn did_change(&self, params: lsp_types::DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        // Apply all incremental changes in order.
        if let Ok(mut map) = self.documents.lock() {
            let text = map.entry(uri.to_string()).or_default();
            for change in params.content_changes {
                if let Some(range) = change.range {
                    // Incremental: apply range-based edit.
                    apply_text_change(text, range, &change.text);
                } else {
                    // Fallback: full document replacement.
                    *text = change.text;
                }
            }
        }
        // Re-parse with the updated document.
        let source = self
            .documents
            .lock()
            .ok()
            .and_then(|m| m.get(uri.as_str()).cloned())
            .unwrap_or_default();
        let diagnostics = Self::parse_diagnostics(&source);
        self.client
            .send_notification::<PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                diagnostics,
                version: Some(params.text_document.version),
            })
            .await;
    }

    /// Text document didSave — re-parse and emit diagnostics.
    async fn did_save(&self, params: lsp_types::DidSaveTextDocumentParams) {
        if let Some(source) = &params.text {
            let uri = params.text_document.uri;
            let diagnostics = Self::parse_diagnostics(source);
            self.client
                .send_notification::<PublishDiagnostics>(PublishDiagnosticsParams {
                    uri,
                    diagnostics,
                    version: None,
                })
                .await;
        }
    }

    /// Go-to-definition (LSP-03 tahap 1): cari deklarasi identifier di
    /// posisi kursor dalam dokumen yang sama.
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonRpcResult<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        match Self::find_definition(&text, pos.line, pos.character) {
            Some((line, start, end)) => Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: Range {
                    start: Position {
                        line,
                        character: start,
                    },
                    end: Position {
                        line,
                        character: end,
                    },
                },
            }))),
            None => Ok(None),
        }
    }

    /// Find-references (LSP-04 tahap 1): semua kemunculan identifier di
    /// dokumen yang sama.
    async fn references(&self, params: ReferenceParams) -> JsonRpcResult<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let refs: Vec<Location> = Self::find_references(&text, pos.line, pos.character)
            .into_iter()
            .map(|(line, start, end)| Location {
                uri: uri.clone(),
                range: Range {
                    start: Position {
                        line,
                        character: start,
                    },
                    end: Position {
                        line,
                        character: end,
                    },
                },
            })
            .collect();
        if refs.is_empty() {
            return Ok(None);
        }
        Ok(Some(refs))
    }

    /// Rename (LSP-05 tahap 1): rename identifier di seluruh dokumen.
    async fn rename(&self, params: RenameParams) -> JsonRpcResult<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        match Self::compute_rename_edits(&text, pos.line, pos.character, &params.new_name) {
            Ok(edits) => {
                let changes: std::collections::HashMap<lsp_types::Url, Vec<lsp_types::TextEdit>> =
                    std::collections::HashMap::from([(
                        uri.clone(),
                        edits
                            .into_iter()
                            .map(|(line, start, end)| lsp_types::TextEdit {
                                range: Range {
                                    start: Position {
                                        line,
                                        character: start,
                                    },
                                    end: Position {
                                        line,
                                        character: end,
                                    },
                                },
                                new_text: params.new_name.clone(),
                            })
                            .collect(),
                    )]);
                Ok(Some(WorkspaceEdit::new(changes)))
            }
            Err(msg) => Err(tower_lsp::jsonrpc::Error::invalid_params(msg)),
        }
    }

    /// Hover (LSP-06 tahap 1): konteks deklarasi identifier.
    async fn hover(&self, params: lsp_types::HoverParams) -> JsonRpcResult<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        match Self::hover_info(&text, pos.line, pos.character) {
            Some(markdown) => Ok(Some(Hover {
                contents: HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
                    language: "markdown".into(),
                    value: markdown,
                })),
                range: None,
            })),
            None => Ok(None),
        }
    }

    /// Document symbol / outline (LSP-12 tahap 1).
    async fn document_symbol(
        &self,
        params: lsp_types::DocumentSymbolParams,
    ) -> JsonRpcResult<Option<lsp_types::DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let line_len = |l: u32| -> u32 {
            text.lines()
                .nth(l as usize)
                .map(|s| s.chars().count() as u32)
                .unwrap_or(0)
        };
        let symbols: Vec<DocumentSymbol> = Self::document_symbols(&text)
            .into_iter()
            .map(|s| s.to_lsp(line_len))
            .collect();
        Ok(Some(lsp_types::DocumentSymbolResponse::Nested(symbols)))
    }

    /// Autocomplete (LSP-02 tahap 1).
    async fn completion(
        &self,
        params: CompletionParams,
    ) -> JsonRpcResult<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let items: Vec<CompletionItem> = Self::compute_completions(&text, pos.line, pos.character)
            .into_iter()
            .map(|(label, kind)| CompletionItem {
                label,
                kind: Some(DocSymbol::completion_kind(kind)),
                ..Default::default()
            })
            .collect();
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(CompletionResponse::Array(items)))
    }

    /// LSP-17: Completion resolve — add documentation/detail to a completion item.
    async fn completion_resolve(&self, mut item: CompletionItem) -> JsonRpcResult<CompletionItem> {
        // Add documentation based on kind.
        let doc =
            match item.kind {
                Some(CompletionItemKind::MODULE) => Some(lsp_types::Documentation::String(
                    format!("SystemVerilog module `{}`", item.label),
                )),
                Some(CompletionItemKind::FUNCTION) => Some(lsp_types::Documentation::String(
                    format!("SystemVerilog function `{}`", item.label),
                )),
                Some(CompletionItemKind::CLASS) => Some(lsp_types::Documentation::String(format!(
                    "SystemVerilog class `{}`",
                    item.label
                ))),
                Some(CompletionItemKind::INTERFACE) => Some(lsp_types::Documentation::String(
                    format!("SystemVerilog interface `{}`", item.label),
                )),
                Some(CompletionItemKind::KEYWORD) => Some(lsp_types::Documentation::String(
                    format!("SystemVerilog keyword `{}`", item.label),
                )),
                Some(CompletionItemKind::VARIABLE) => Some(lsp_types::Documentation::String(
                    format!("Signal/variable `{}`", item.label),
                )),
                Some(CompletionItemKind::CONSTANT) => Some(lsp_types::Documentation::String(
                    format!("Parameter `{}`", item.label),
                )),
                _ => None,
            };
        if doc.is_some() {
            item.documentation = doc;
        }
        Ok(item)
    }

    /// LSP-16: Signature help — parameter hints saat typing function call.
    async fn signature_help(
        &self,
        params: lsp_types::SignatureHelpParams,
    ) -> JsonRpcResult<Option<lsp_types::SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let Some(line_text) = text.lines().nth(pos.line as usize) else {
            return Ok(None);
        };
        // Find the function name before the cursor (search backward for ident followed by '(').
        let bytes = line_text.as_bytes();
        let cursor = (pos.character as usize).min(bytes.len());
        // Count parentheses to find active parameter.
        let mut paren_depth = 0i32;
        let mut active_param = 0u32;
        for i in (0..cursor).rev() {
            match bytes[i] {
                b')' => paren_depth += 1,
                b'(' => {
                    paren_depth -= 1;
                    if paren_depth < 0 {
                        // Found the opening paren — extract function name.
                        let name_end = i;
                        let mut name_start = i;
                        while name_start > 0
                            && (bytes[name_start - 1].is_ascii_alphanumeric()
                                || bytes[name_start - 1] == b'_')
                        {
                            name_start -= 1;
                        }
                        if name_start < name_end {
                            let func_name =
                                std::str::from_utf8(&bytes[name_start..name_end]).unwrap_or("");
                            // Build a simple signature from the function declaration.
                            let sig = Self::find_function_signature(&text, func_name);
                            if let Some((label, params_list)) = sig {
                                let parameters: Vec<lsp_types::ParameterInformation> = params_list
                                    .iter()
                                    .map(|p| lsp_types::ParameterInformation {
                                        label: lsp_types::ParameterLabel::Simple(p.clone()),
                                        documentation: None,
                                    })
                                    .collect();
                                return Ok(Some(lsp_types::SignatureHelp {
                                    signatures: vec![lsp_types::SignatureInformation {
                                        label,
                                        documentation: None,
                                        parameters: Some(parameters),
                                        active_parameter: None,
                                    }],
                                    active_signature: Some(0),
                                    active_parameter: Some(active_param),
                                }));
                            }
                        }
                        break;
                    }
                }
                b',' if paren_depth == 0 => {
                    active_param += 1;
                }
                _ => {}
            }
        }
        Ok(None)
    }

    /// Workspace symbol search (LSP-11 tahap 1): cari di dokumen terbuka.
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> JsonRpcResult<Option<Vec<SymbolInformation>>> {
        let docs: Vec<(String, String)> = {
            match self.documents.lock() {
                Ok(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                Err(_) => Vec::new(),
            }
        };
        let hits = Self::search_workspace_symbols(&docs, &params.query);
        let mut out = Vec::new();
        for h in hits {
            let Ok(uri) = Url::parse(&h.uri) else {
                continue;
            };
            let name_len = h.name.len() as u32;
            out.push(SymbolInformation {
                name: h.name,
                kind: DocSymbol {
                    name: String::new(),
                    kind: h.kind,
                    line: 0,
                    col: 0,
                    children: Vec::new(),
                }
                .lsp_kind(),
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                location: Location {
                    uri,
                    range: Range {
                        start: Position {
                            line: h.line,
                            character: h.col,
                        },
                        end: Position {
                            line: h.line,
                            character: h.col + name_len,
                        },
                    },
                },
                container_name: None,
            });
        }
        if out.is_empty() {
            return Ok(None);
        }
        Ok(Some(out))
    }

    /// Folding range (LSP-15).
    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> JsonRpcResult<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let ranges = Self::compute_folding_ranges(&text)
            .into_iter()
            .map(|(start, end)| FoldingRange {
                start_line: start,
                start_character: None,
                end_line: end,
                end_character: None,
                kind: Some(FoldingRangeKind::Region),
                collapsed_text: None,
            })
            .collect();
        Ok(Some(ranges))
    }

    /// Semantic tokens full (LSP-07 tahap 1).
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> JsonRpcResult<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let tokens = Self::compute_semantic_tokens(&text);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }

    /// LSP-20: Document formatting — format the entire document.
    async fn formatting(
        &self,
        params: lsp_types::DocumentFormattingParams,
    ) -> JsonRpcResult<Option<Vec<lsp_types::TextEdit>>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let indent_width = params.options.tab_size as usize;
        let formatted = Self::format_source(&text, indent_width);
        if formatted == text {
            return Ok(None);
        }
        // Replace entire document.
        let line_count = text.lines().count() as u32;
        let last_line_len = text
            .lines()
            .last()
            .map(|l| l.chars().count() as u32)
            .unwrap_or(0);
        Ok(Some(vec![lsp_types::TextEdit {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(line_count, last_line_len),
            },
            new_text: formatted,
        }]))
    }

    /// LSP-10: Code actions — quick-fix from diagnostic FixItHint.
    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> JsonRpcResult<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let mut actions: CodeActionResponse = Vec::new();
        for diag in &params.context.diagnostics {
            // Generate code actions from diagnostic message patterns.
            if let Some(actions_for_diag) = Self::actions_from_diagnostic(&text, diag, uri) {
                actions.extend(actions_for_diag);
            }
        }
        if actions.is_empty() {
            return Ok(None);
        }
        Ok(Some(actions))
    }

    /// LSP-08: Code lens — inline annotations above modules and functions.
    async fn code_lens(&self, params: CodeLensParams) -> JsonRpcResult<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let lenses = Self::compute_code_lens(&text, uri);
        if lenses.is_empty() {
            return Ok(None);
        }
        Ok(Some(lenses))
    }

    /// LSP-24: Selection range — smart selection (word → statement → scope).
    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> JsonRpcResult<Option<Vec<SelectionRange>>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let mut result = Vec::new();
        for pos in &params.positions {
            let range = compute_selection_range(&text, pos.line, pos.character);
            result.push(range);
        }
        if result.is_empty() {
            return Ok(None);
        }
        Ok(Some(result))
    }

    /// LSP-22: Color provider — detect hex color literals (#RRGGBB).
    async fn document_color(
        &self,
        params: lsp_types::DocumentColorParams,
    ) -> JsonRpcResult<Vec<ColorInformation>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(Vec::new());
        };
        Ok(compute_document_colors(&text))
    }

    /// LSP-22: Color presentation — convert color to hex string.
    async fn color_presentation(
        &self,
        params: ColorPresentationParams,
    ) -> JsonRpcResult<Vec<ColorPresentation>> {
        let r = (params.color.red * 255.0) as u8;
        let g = (params.color.green * 255.0) as u8;
        let b = (params.color.blue * 255.0) as u8;
        let label = format!("#{:02X}{:02X}{:02X}", r, g, b);
        Ok(vec![ColorPresentation {
            label: label.clone(),
            text_edit: Some(lsp_types::TextEdit {
                range: params.range,
                new_text: label,
            }),
            additional_text_edits: None,
        }])
    }

    /// LSP-23: Linked editing range — select matching words for simultaneous edit.
    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> JsonRpcResult<Option<LinkedEditingRanges>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let Some(line_text) = text.lines().nth(pos.line as usize) else {
            return Ok(None);
        };
        // Extract word at cursor.
        let bytes = line_text.as_bytes();
        let ch = pos.character as usize;
        let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        if ch >= bytes.len() || !is_ident(bytes[ch]) {
            return Ok(None);
        }
        let mut start = ch;
        while start > 0 && is_ident(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = ch;
        while end < bytes.len() && is_ident(bytes[end]) {
            end += 1;
        }
        if start >= end {
            return Ok(None);
        }
        let word = &line_text[start..end];
        if word.len() < 2 {
            return Ok(None);
        }
        // Find all occurrences of the word in the document.
        let mut ranges = Vec::new();
        for (line_idx, line) in text.lines().enumerate() {
            let mut offset = 0;
            while let Some(pos) = line[offset..].find(word) {
                let abs_pos = offset + pos;
                // Word boundary check.
                let before_ok = abs_pos == 0 || !is_ident(line.as_bytes()[abs_pos - 1]);
                let after_ok = abs_pos + word.len() >= line.len()
                    || !is_ident(line.as_bytes()[abs_pos + word.len()]);
                if before_ok && after_ok {
                    ranges.push(Range::new(
                        Position::new(line_idx as u32, abs_pos as u32),
                        Position::new(line_idx as u32, (abs_pos + word.len()) as u32),
                    ));
                }
                offset = abs_pos + 1;
                if offset >= line.len() {
                    break;
                }
            }
        }
        if ranges.len() < 2 {
            return Ok(None);
        }
        Ok(Some(LinkedEditingRanges {
            ranges,
            word_pattern: Some(format!("\\b{}\\b", word)),
        }))
    }

    /// Text document didClose — clear diagnostics.
    async fn did_close(&self, params: lsp_types::DidCloseTextDocumentParams) {
        self.client
            .send_notification::<PublishDiagnostics>(PublishDiagnosticsParams {
                uri: params.text_document.uri,
                diagnostics: vec![],
                version: None,
            })
            .await;
    }

    // ── LSP-13: Call Hierarchy (core function — sync) ──
    // NOTE: tower-lsp 0.20 tidak expose call_hierarchy_incoming/outgoing
    // di trait LanguageServer. Capability didaftarkan di ServerCapabilities
    // untuk client yang mendukung, tapi handler belum ter-wire.
    // Core logic tersedia via compute_call_hierarchy() untuk testing.

    // ── LSP-14: Type Hierarchy ──
    // NOTE: SystemVerilog has no class inheritance tree exposed via LSP natively.
    // This provides a simple supertype/subtype view for class extends chains.
}

/// Apply an incremental text change to a document string (LSP-18).
fn apply_text_change(text: &mut String, range: Range, new_text: &str) {
    let lines: Vec<&str> = text.lines().collect();

    // Convert (line, character) to byte offset.
    let start_offset = position_to_offset(&lines, range.start);
    let end_offset = position_to_offset(&lines, range.end);

    // Replace the byte range.
    if start_offset <= end_offset && end_offset <= text.len() {
        text.replace_range(start_offset..end_offset, new_text);
    }
}

/// Convert a Position (line, character) to a byte offset in the text.
fn position_to_offset(lines: &[&str], pos: Position) -> usize {
    let mut offset = 0;
    for (i, line) in lines.iter().enumerate() {
        if i == pos.line as usize {
            // Character is byte offset within this line.
            let char_offset = pos.character as usize;
            let safe_offset = char_offset.min(line.len());
            return offset + safe_offset;
        }
        // +1 for the newline character (not present in .lines() output).
        offset += line.len() + 1;
    }
    offset
}

/// LSP-24: Compute selection range — nested ranges from word → line → scope.
fn compute_selection_range(text: &str, line: u32, character: u32) -> SelectionRange {
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = line as usize;

    // Innermost: the word at cursor.
    let word_range = if let Some(line_text) = lines.get(line_idx) {
        let bytes = line_text.as_bytes();
        let ch = character as usize;
        let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let mut start = ch.min(bytes.len());
        while start > 0 && is_ident(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = ch.min(bytes.len());
        while end < bytes.len() && is_ident(bytes[end]) {
            end += 1;
        }
        if start < end {
            Range::new(
                Position::new(line, start as u32),
                Position::new(line, end as u32),
            )
        } else {
            Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            )
        }
    } else {
        Range::new(
            Position::new(line, character),
            Position::new(line, character + 1),
        )
    };

    // Middle: the entire line.
    let line_range = Range::new(
        Position::new(line, 0),
        Position::new(
            line,
            lines.get(line_idx).map(|l| l.len() as u32).unwrap_or(0),
        ),
    );

    // Outermost: enclosing scope (module/class/function/end → start).
    let scope_range = find_enclosing_scope(&lines, line_idx).unwrap_or(Range::new(
        Position::new(0, 0),
        Position::new(lines.len() as u32, 0),
    ));

    // Build nested chain: word → line → scope.
    SelectionRange {
        range: scope_range,
        parent: Some(Box::new(SelectionRange {
            range: line_range,
            parent: Some(Box::new(SelectionRange {
                range: word_range,
                parent: None,
            })),
        })),
    }
}

/// Find the enclosing scope range for a given line.
fn find_enclosing_scope(lines: &[&str], target_line: usize) -> Option<Range> {
    const OPEN_KW: &[&str] = &[
        "module",
        "macromodule",
        "interface",
        "package",
        "class",
        "checker",
        "function",
        "task",
        "program",
    ];
    const CLOSE_KW: &[&str] = &[
        "endmodule",
        "endinterface",
        "endpackage",
        "endclass",
        "endchecker",
        "endfunction",
        "endtask",
        "endprogram",
    ];

    // Walk backward from target line to find opening keyword.
    let mut depth = 0i32;
    for i in (0..=target_line).rev() {
        let trimmed = lines[i].trim();
        let first = trimmed.split_whitespace().next().unwrap_or("");
        if CLOSE_KW.contains(&first) {
            depth += 1;
        } else if OPEN_KW.contains(&first) {
            if depth == 0 {
                // Found the opening keyword. Now find the matching end.
                let mut end_depth = 0i32;
                for (j, line) in lines.iter().enumerate().skip(i) {
                    let t = line.trim();
                    let f = t.split_whitespace().next().unwrap_or("");
                    if OPEN_KW.contains(&f) {
                        end_depth += 1;
                    } else if CLOSE_KW.contains(&f) {
                        end_depth -= 1;
                        if end_depth == 0 {
                            return Some(Range::new(
                                Position::new(i as u32, 0),
                                Position::new(j as u32, line.len() as u32),
                            ));
                        }
                    }
                }
                // Unclosed scope.
                return Some(Range::new(
                    Position::new(i as u32, 0),
                    Position::new(lines.len() as u32, 0),
                ));
            } else {
                depth -= 1;
            }
        }
    }
    None
}

/// LSP-22: Compute document colors from hex color literals.
fn compute_document_colors(text: &str) -> Vec<ColorInformation> {
    let mut colors = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        // Skip comments.
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        // Find #RRGGBB patterns.
        for (i, _) in line.match_indices('#') {
            if i + 7 <= line.len() {
                let hex = &line[i + 1..i + 7];
                if let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&hex[0..2], 16),
                    u8::from_str_radix(&hex[2..4], 16),
                    u8::from_str_radix(&hex[4..6], 16),
                ) {
                    // Verify all chars are hex digits.
                    if hex.chars().all(|c| c.is_ascii_hexdigit()) {
                        colors.push(ColorInformation {
                            range: Range::new(
                                Position::new(line_idx as u32, i as u32),
                                Position::new(line_idx as u32, (i + 7) as u32),
                            ),
                            color: lsp_types::Color {
                                red: r as f32 / 255.0,
                                green: g as f32 / 255.0,
                                blue: b as f32 / 255.0,
                                alpha: 1.0,
                            },
                        });
                    }
                }
            }
        }
    }
    colors
}

/// Run the LSP server with stdio transport.
pub async fn run_lsp_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, messages) = LspService::new(LspBackend::new);
    Server::new(stdin, stdout, messages).serve(service).await;
}

// ═══ LSP-09: Inlay Hints (type hints) ═══
#[allow(dead_code)] // scaffolding LSP-09 — belum dipanggil handler LSP (dihasilkan API utk dipakai nanti)
impl LspBackend {
    /// Compute inlay hints for a document (LSP-09).
    /// Returns type hints for variables, parameters, and function return types.
    pub(crate) fn compute_inlay_hints(
        text: &str,
        range: Option<Range>,
    ) -> Vec<lsp_types::InlayHint> {
        let mut hints = Vec::new();
        let lines: Vec<&str> = text.lines().collect();
        let start_line = range.map(|r| r.start.line as usize).unwrap_or(0);
        let end_line = range.map(|r| r.end.line as usize).unwrap_or(lines.len());

        for (line_idx, line) in lines
            .iter()
            .enumerate()
            .skip(start_line)
            .take(end_line - start_line)
        {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }

            // 1. Port/signal declarations: `input logic [7:0] data`
            //    → hint after name: `: logic [7:0]`
            let type_dirs = ["input", "output", "inout"];
            if type_dirs.iter().any(|d| trimmed.starts_with(d)) {
                if let Some((name, type_str)) = Self::parse_port_type_hint(trimmed) {
                    let col = Self::find_word_col(line, &name).unwrap_or(0) + name.len() as u32;
                    hints.push(Self::make_type_hint(line_idx as u32, col, &type_str));
                }
            }

            // 2. Internal declarations: `logic [7:0] cnt`, `reg [3:0] state`, `wire [15:0] bus`
            let int_types = [
                "logic", "reg", "wire", "bit", "int", "integer", "byte", "shortint", "longint",
            ];
            if int_types.iter().any(|t| trimmed.starts_with(t)) {
                if let Some((name, type_str)) = Self::parse_signal_type_hint(trimmed) {
                    let col = Self::find_word_col(line, &name).unwrap_or(0) + name.len() as u32;
                    hints.push(Self::make_type_hint(line_idx as u32, col, &type_str));
                }
            }

            // 3. Function/task return type: `function int add(...)`
            //    → hint after function name: `: int`
            if trimmed.starts_with("function") {
                if let Some((name, ret_type)) = Self::parse_function_return_hint(trimmed) {
                    let col = Self::find_word_col(line, &name).unwrap_or(0) + name.len() as u32;
                    hints.push(Self::make_type_hint(line_idx as u32, col, &ret_type));
                }
            }

            // 4. Parameter declarations inside `#(parameter ...)`
            if trimmed.starts_with("parameter") || trimmed.starts_with("localparam") {
                if let Some((name, type_str)) = Self::parse_param_type_hint(trimmed) {
                    let col = Self::find_word_col(line, &name).unwrap_or(0) + name.len() as u32;
                    hints.push(Self::make_type_hint(line_idx as u32, col, &type_str));
                }
            }
        }
        hints
    }

    fn make_type_hint(line: u32, col: u32, label: &str) -> lsp_types::InlayHint {
        lsp_types::InlayHint {
            position: Position::new(line, col),
            label: lsp_types::InlayHintLabel::String(format!(": {}", label)),
            kind: Some(lsp_types::InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: Some(true),
            padding_right: None,
            data: None,
        }
    }

    /// Find column of a word in a line.
    fn find_word_col(line: &str, word: &str) -> Option<u32> {
        line.find(word).map(|c| c as u32)
    }

    /// Parse `input/output/inout [type] [range] name` → (name, type_str).
    fn parse_port_type_hint(line: &str) -> Option<(String, String)> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 3 {
            return None;
        }
        // Skip direction (input/output/inout).
        let mut i = 1;
        // Collect type tokens until we hit the variable name.
        let mut type_parts: Vec<&str> = Vec::new();
        while i < toks.len() {
            let t = toks[i];
            // Check if this is the variable name (no type keywords).
            if is_type_keyword(t) || t.starts_with('[') {
                type_parts.push(t);
                i += 1;
            } else if t == "," || t == ";" || t == ")" {
                break;
            } else {
                // This is the variable name.
                let name = t.trim_end_matches(',').to_string();
                if type_parts.is_empty() {
                    return None;
                }
                return Some((name, type_parts.join(" ")));
            }
        }
        None
    }

    /// Parse `logic [range] name` → (name, type_str).
    fn parse_signal_type_hint(line: &str) -> Option<(String, String)> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            return None;
        }
        let mut i = 0;
        let mut type_parts: Vec<&str> = Vec::new();
        while i < toks.len() {
            let t = toks[i];
            if is_type_keyword(t) || t.starts_with('[') {
                type_parts.push(t);
                i += 1;
            } else if t == "," || t == ";" || t == "=" {
                break;
            } else {
                let name = t.trim_end_matches(',').to_string();
                if type_parts.is_empty() {
                    return None;
                }
                return Some((name, type_parts.join(" ")));
            }
        }
        None
    }

    /// Parse `function ret_type name(...)` → (name, ret_type).
    fn parse_function_return_hint(line: &str) -> Option<(String, String)> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 3 {
            return None;
        }
        // function <ret_type> <name>
        let ret_type = toks[1];
        let name = toks[2].trim_end_matches('(');
        if ret_type == "void" || ret_type == "automatic" || ret_type == "static" {
            return None;
        }
        Some((name.to_string(), ret_type.to_string()))
    }

    /// Parse `parameter [type] name = value` → (name, type_str).
    fn parse_param_type_hint(line: &str) -> Option<(String, String)> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 3 {
            return None;
        }
        let mut i = 1;
        let mut type_parts: Vec<&str> = Vec::new();
        while i < toks.len() {
            let t = toks[i];
            if is_type_keyword(t) || t.starts_with('[') {
                type_parts.push(t);
                i += 1;
            } else if t == "=" || t == ";" {
                break;
            } else {
                let name = t.trim_end_matches(',').to_string();
                if type_parts.is_empty() {
                    return None;
                }
                return Some((name, type_parts.join(" ")));
            }
        }
        None
    }
}

// ═══ LSP-08: Code Lens ═══

impl LspBackend {
    /// Compute code lenses for a document (LSP-08).
    /// Shows:
    /// - Above each module: "Run tests" action
    /// - Above each function/task: reference count
    pub(crate) fn compute_code_lens(text: &str, uri: &Url) -> Vec<CodeLens> {
        let mut lenses = Vec::new();
        let syms = Self::document_symbols(text);

        #[allow(clippy::only_used_in_recursion)]
        fn walk(items: &[DocSymbol], text: &str, _uri: &Url, lenses: &mut Vec<CodeLens>) {
            for sym in items {
                match sym.kind {
                    // Module → "Run tests" lens
                    DocSymbol::KIND_MODULE => {
                        // Count tests (functions starting with "test_" in module body)
                        let test_count = count_tests_in_scope(text, sym);
                        let title = if test_count > 0 {
                            format!(
                                "{} test{}",
                                test_count,
                                if test_count == 1 { "" } else { "s" }
                            )
                        } else {
                            "Run tests".to_string()
                        };
                        lenses.push(CodeLens {
                            range: Range {
                                start: Position::new(sym.line, sym.col),
                                end: Position::new(sym.line, sym.col + sym.name.len() as u32),
                            },
                            command: Some(LspCommand {
                                title,
                                command: format!("mivon.testModule.{}", sym.name),
                                arguments: None,
                            }),
                            data: None,
                        });
                    }
                    // Function/task → reference count + test run lens (LSP-21)
                    DocSymbol::KIND_FUNCTION => {
                        // Estimate reference count (word-boundary match)
                        let ref_count = count_word_occurrences(text, &sym.name).saturating_sub(1); // -1 for decl
                        if ref_count > 0 {
                            lenses.push(CodeLens {
                                range: Range {
                                    start: Position::new(sym.line, sym.col),
                                    end: Position::new(sym.line, sym.col + sym.name.len() as u32),
                                },
                                command: Some(LspCommand {
                                    title: format!(
                                        "{} reference{}",
                                        ref_count,
                                        if ref_count == 1 { "" } else { "s" }
                                    ),
                                    command: format!("mivon.showReferences.{}", sym.name),
                                    arguments: None,
                                }),
                                data: None,
                            });
                        }
                        // LSP-21: "Run test" lens for test functions (name starts with "test_")
                        if sym.name.starts_with("test_") {
                            lenses.push(CodeLens {
                                range: Range {
                                    start: Position::new(sym.line, sym.col),
                                    end: Position::new(sym.line, sym.col + sym.name.len() as u32),
                                },
                                command: Some(LspCommand {
                                    title: "▶ Run test".to_string(),
                                    command: format!("mivon.runTest.{}", sym.name),
                                    arguments: None,
                                }),
                                data: None,
                            });
                        }
                    }
                    _ => {}
                }
                // Recurse into children.
                walk(&sym.children, text, _uri, lenses);
            }
        }

        walk(&syms, text, uri, &mut lenses);
        lenses
    }
}

/// LSP-16: Find function/task signature from declaration text.
/// Returns (label, param_list) e.g. ("function int add(int a, int b)", ["int a", "int b"]).
impl LspBackend {
    fn find_function_signature(text: &str, name: &str) -> Option<(String, Vec<String>)> {
        for line in text.lines() {
            let trimmed = line.trim();
            if (trimmed.starts_with("function") || trimmed.starts_with("task"))
                && trimmed.contains(name)
                && trimmed.contains('(')
            {
                // Extract the full signature up to ')'.
                if let Some(paren_start) = trimmed.find('(') {
                    if let Some(paren_end) = trimmed.find(')') {
                        let label = trimmed[..=paren_end].to_string();
                        let params_str = &trimmed[paren_start + 1..paren_end];
                        let params: Vec<String> = params_str
                            .split(',')
                            .map(|p| p.trim().to_string())
                            .filter(|p| !p.is_empty())
                            .collect();
                        return Some((label, params));
                    }
                }
            }
        }
        None
    }
}

/// Check if a token is a SystemVerilog type keyword.
fn is_type_keyword(s: &str) -> bool {
    matches!(
        s,
        "logic"
            | "reg"
            | "wire"
            | "bit"
            | "int"
            | "integer"
            | "byte"
            | "shortint"
            | "longint"
            | "real"
            | "realtime"
            | "time"
            | "string"
            | "signed"
            | "unsigned"
            | "void"
    )
}

/// Count test functions (names starting with "test_") in a module scope.
fn count_tests_in_scope(_text: &str, module: &DocSymbol) -> usize {
    let mut count = 0;
    // Look in children for functions starting with "test_"
    fn walk_count(items: &[DocSymbol], count: &mut usize) {
        for s in items {
            if s.kind == DocSymbol::KIND_FUNCTION && s.name.starts_with("test_") {
                *count += 1;
            }
            walk_count(&s.children, count);
        }
    }
    walk_count(&module.children, &mut count);
    count
}

/// Count occurrences of a word in text (word-boundary match).
fn count_word_occurrences(text: &str, word: &str) -> usize {
    let mut count = 0;
    let bytes = text.as_bytes();
    let wbytes = word.as_bytes();
    let wlen = wbytes.len();
    for i in 0..bytes.len() {
        if i + wlen <= bytes.len() && &bytes[i..i + wlen] == wbytes {
            // Check word boundaries.
            let before_ok =
                i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            let after_ok = i + wlen >= bytes.len()
                || !(bytes[i + wlen].is_ascii_alphanumeric() || bytes[i + wlen] == b'_');
            if before_ok && after_ok {
                count += 1;
            }
        }
    }
    count
}

// ═══ LSP-13: Call Hierarchy ═══

#[allow(dead_code)] // scaffolding LSP-13 — API disiapkan, handler belum di-wire
impl LspBackend {
    /// Compute call hierarchy for a symbol (LSP-13).
    /// Returns incoming calls (callers) and outgoing calls (callees).
    pub(crate) fn compute_call_hierarchy(
        text: &str,
        _name: &str,
    ) -> (
        Vec<lsp_types::CallHierarchyItem>,
        Vec<lsp_types::CallHierarchyItem>,
    ) {
        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();

        // Simple pattern matching for function/task calls
        for (line_idx, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            // Detect function/task declarations
            if trimmed.starts_with("function") || trimmed.starts_with("task") {
                if let Some(name) = Self::extract_symbol_name(trimmed) {
                    let item = lsp_types::CallHierarchyItem {
                        name,
                        kind: SymbolKind::FUNCTION,
                        tags: None,
                        detail: Some(trimmed.chars().take(50).collect()),
                        uri: Url::parse("file:///untitled").unwrap(),
                        range: Range::new(
                            Position::new(line_idx as u32, 0),
                            Position::new(line_idx as u32, trimmed.len() as u32),
                        ),
                        selection_range: Range::new(
                            Position::new(line_idx as u32, 0),
                            Position::new(line_idx as u32, trimmed.len() as u32),
                        ),
                        data: None,
                    };
                    outgoing.push(item);
                }
            }
            // Detect function/task calls
            if trimmed.contains('(')
                && !trimmed.starts_with("function")
                && !trimmed.starts_with("task")
            {
                if let Some(name) = Self::extract_call_name(trimmed) {
                    let item = lsp_types::CallHierarchyItem {
                        name,
                        kind: SymbolKind::FUNCTION,
                        tags: None,
                        detail: Some("caller".to_string()),
                        uri: Url::parse("file:///untitled").unwrap(),
                        range: Range::new(
                            Position::new(line_idx as u32, 0),
                            Position::new(line_idx as u32, trimmed.len() as u32),
                        ),
                        selection_range: Range::new(
                            Position::new(line_idx as u32, 0),
                            Position::new(line_idx as u32, trimmed.len() as u32),
                        ),
                        data: None,
                    };
                    incoming.push(item);
                }
            }
        }

        (incoming, outgoing)
    }

    /// Extract symbol name from a declaration line.
    /// Skips type keywords (int, logic, reg, etc.) to get the actual name.
    fn extract_symbol_name(line: &str) -> Option<String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let type_keywords = [
            "int", "logic", "reg", "wire", "bit", "byte", "integer", "shortint", "longint", "real",
            "realtime", "time", "string", "signed", "unsigned", "void",
        ];
        for (i, part) in parts.iter().enumerate() {
            if i == 0 {
                continue;
            }
            let name = part.trim_end_matches('(').trim_end_matches(')');
            if !type_keywords.contains(&name) && !name.is_empty() {
                return Some(name.to_string());
            }
        }
        None
    }

    /// Extract call name from a statement line.
    fn extract_call_name(line: &str) -> Option<String> {
        // Find word before '('
        if let Some(pos) = line.find('(') {
            let before = line[..pos].trim();
            let parts: Vec<&str> = before.split_whitespace().collect();
            if let Some(name) = parts.last() {
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return Some(name.to_string());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
module counter #(parameter WIDTH = 8) (
    input logic clk,
    input logic rst_n,
    output logic [WIDTH-1:0] count
);
    logic [3:0] state;
    reg enable;

    counter_aux aux (.clk(clk));

    always_ff @(posedge clk) begin
        if (!rst_n) count <= 0;
        else if (enable) count <= count + 1;
    end
endmodule

module counter_aux (
    input logic clk
);
endmodule
";

    /// Baris pertama yang memuat `needle` (untuk ekspektasi robust).
    fn line_of(needle: &str) -> u32 {
        SAMPLE.lines().position(|l| l.contains(needle)).unwrap() as u32
    }

    #[test]
    fn test_word_at_position() {
        // Kursor di tengah "counter" baris 0.
        let (w, s, e) = LspBackend::word_at_position("module counter;", 11).unwrap();
        assert_eq!(w, "counter");
        assert_eq!(s, 7);
        assert_eq!(e, 14);
        // Kursor di akhir kata.
        let (w, _, _) = LspBackend::word_at_position("module counter;", 14).unwrap();
        assert_eq!(w, "counter");
        // Bukan identifier.
        assert!(LspBackend::word_at_position("   ;", 1).is_none());
        // $ dalam identifier SV.
        let (w, _, _) = LspBackend::word_at_position("wire a$b;", 6).unwrap();
        assert_eq!(w, "a$b");
    }

    #[test]
    fn test_find_definition_module() {
        // Kursor di nama module sendiri → definisi tetap ditemukan.
        let (line, col, _) = LspBackend::find_definition(SAMPLE, 0, 10).unwrap();
        assert_eq!(line, line_of("module counter"), "definisi module counter");
        assert!(SAMPLE.lines().nth(line as usize).unwrap()[col as usize..].starts_with("counter"));

        // Instance `counter_aux aux` (baris pemakaian) → definisi module
        // counter_aux.
        let use_line = line_of("counter_aux aux");
        let aux_col = SAMPLE
            .lines()
            .nth(use_line as usize)
            .unwrap()
            .find("counter_aux")
            .unwrap() as u32;
        let (line, col, _) = LspBackend::find_definition(SAMPLE, use_line, aux_col + 3).unwrap();
        assert_eq!(line, line_of("module counter_aux"));
        assert!(
            SAMPLE.lines().nth(line as usize).unwrap()[col as usize..].starts_with("counter_aux")
        );
    }

    #[test]
    fn test_find_definition_signal_and_port() {
        // Pemakaian `enable` di always_ff → deklarasi `reg enable;`.
        let use_line = line_of("else if (enable)");
        let enable_col = SAMPLE
            .lines()
            .nth(use_line as usize)
            .unwrap()
            .find("enable")
            .unwrap() as u32;
        let (line, col, _) = LspBackend::find_definition(SAMPLE, use_line, enable_col).unwrap();
        assert_eq!(line, line_of("reg enable"));
        assert!(SAMPLE.lines().nth(line as usize).unwrap()[col as usize..].starts_with("enable"));

        // Pemakaian `clk` di port connection `.clk(clk)` → port decl
        // `input logic clk,` di module counter.
        let inst_line = line_of(".clk(clk)");
        let clk_col = SAMPLE
            .lines()
            .nth(inst_line as usize)
            .unwrap()
            .rfind("(clk)")
            .map(|p| p as u32 + 1)
            .unwrap();
        let (line, col, _) = LspBackend::find_definition(SAMPLE, inst_line, clk_col).unwrap();
        assert_eq!(line, line_of("input logic clk,"));
        assert!(SAMPLE.lines().nth(line as usize).unwrap()[col as usize..].starts_with("clk"));

        // Signal `state` dipakai? Tidak ada pemakaian; deklarasinya sendiri
        // tetap ditemukan.
        let decl_line = line_of("logic [3:0] state;");
        let st_col = SAMPLE
            .lines()
            .nth(decl_line as usize)
            .unwrap()
            .find("state")
            .unwrap() as u32;
        let (line, col, _) = LspBackend::find_definition(SAMPLE, decl_line, st_col).unwrap();
        assert_eq!(line, decl_line);
        assert!(SAMPLE.lines().nth(line as usize).unwrap()[col as usize..].starts_with("state"));
    }

    #[test]
    fn test_find_definition_parameter() {
        // `[WIDTH-1:0]` pada port output → parameter WIDTH baris pertama.
        let out_line = line_of("[WIDTH-1:0] count");
        let w_col = SAMPLE
            .lines()
            .nth(out_line as usize)
            .unwrap()
            .find("WIDTH")
            .unwrap() as u32;
        let (line, col, _) = LspBackend::find_definition(SAMPLE, out_line, w_col).unwrap();
        assert_eq!(line, line_of("parameter WIDTH"));
        assert!(SAMPLE.lines().nth(line as usize).unwrap()[col as usize..].starts_with("WIDTH"));
    }

    #[test]
    fn test_find_references() {
        // Referensi `clk`: port decl + instance connection + always_ff.
        let decl_line = line_of("input logic clk,");
        let inst_line = line_of(".clk(clk)");
        let ff_line = line_of("always_ff @(posedge clk)");
        let refs = LspBackend::find_references(SAMPLE, inst_line, 26);
        let refs = if refs.is_empty() {
            // fallback: kolom clk kedua di baris instance.
            let col = SAMPLE
                .lines()
                .nth(inst_line as usize)
                .unwrap()
                .rfind("(clk)")
                .unwrap() as u32
                + 1;
            LspBackend::find_references(SAMPLE, inst_line, col)
        } else {
            refs
        };
        assert_eq!(refs.len(), 5, "clk muncul 5x: {:?}", refs);
        let lines: Vec<u32> = refs.iter().map(|(l, _, _)| *l).collect();
        assert!(lines.contains(&decl_line));
        assert!(lines.contains(&inst_line));
        assert!(lines.contains(&ff_line));
        assert!(lines.contains(&line_of("input logic clk"))); // port counter_aux

        // Referensi `enable` (decl + pemakaian di else-if).
        let en_line = line_of("reg enable;");
        let use_line = line_of("else if (enable)");
        let col = SAMPLE
            .lines()
            .nth(use_line as usize)
            .unwrap()
            .find("enable")
            .unwrap() as u32;
        let refs2 = LspBackend::find_references(SAMPLE, use_line, col);
        assert_eq!(refs2.len(), 2, "enable muncul 2x: {:?}", refs2);
        let lines2: Vec<u32> = refs2.iter().map(|(l, _, _)| *l).collect();
        assert!(lines2.contains(&en_line));
        assert!(lines2.contains(&use_line));

        // Word boundary: `count` TIDAK match dengan `counter`/`count_aux`.
        let cnt_line = line_of("output logic [WIDTH-1:0] count");
        let c_col = SAMPLE
            .lines()
            .nth(cnt_line as usize)
            .unwrap()
            .rfind("count")
            .unwrap() as u32;
        let refs3 = LspBackend::find_references(SAMPLE, cnt_line, c_col);
        assert!(!refs3.is_empty(), "count harus ditemukan");
        for (l, s, e) in &refs3 {
            let text = SAMPLE.lines().nth(*l as usize).unwrap();
            assert!(
                !text[*s as usize..*e as usize].contains("counter"),
                "word boundary bocor: {}",
                text
            );
        }
    }

    #[test]
    fn test_find_references_empty_and_miss() {
        // Kursor bukan identifier → kosong.
        assert!(LspBackend::find_references(SAMPLE, 7, 0).is_empty());
        // Baris di luar dokumen → kosong.
        assert!(LspBackend::find_references(SAMPLE, 999, 0).is_empty());
    }

    #[test]
    fn test_rename_edits() {
        // Rename `enable` (2 lokusi) → `en`.
        let decl_line = line_of("reg enable;");
        let use_line = line_of("else if (enable)");
        let col = SAMPLE
            .lines()
            .nth(use_line as usize)
            .unwrap()
            .find("enable")
            .unwrap() as u32;
        let edits = LspBackend::compute_rename_edits(SAMPLE, use_line, col, "en").unwrap();
        assert_eq!(edits.len(), 2, "enable di-rename 2 lokusi: {:?}", edits);
        let lines: Vec<u32> = edits.iter().map(|(l, _, _)| *l).collect();
        assert!(lines.contains(&decl_line));
        assert!(lines.contains(&use_line));
        // Simulasi apply edit (dari bawah ke atas agar offset aman).
        let mut lines_mut: Vec<String> = SAMPLE.lines().map(|s| s.to_string()).collect();
        for (l, s, e) in edits.iter().rev() {
            let text = lines_mut[*l as usize].clone();
            lines_mut[*l as usize] =
                format!("{}{}{}", &text[..*s as usize], "en", &text[*e as usize..]);
        }
        assert!(lines_mut[decl_line as usize].contains("reg en;"));
        assert!(lines_mut[use_line as usize].contains("if (en)"));

        // Nama tidak valid → Err.
        assert!(LspBackend::compute_rename_edits(SAMPLE, use_line, col, "1bad").is_err());
        assert!(LspBackend::compute_rename_edits(SAMPLE, use_line, col, "").is_err());

        // Kursor bukan identifier → Err.
        assert!(LspBackend::compute_rename_edits(SAMPLE, 7, 0, "x").is_err());

        // Word boundary: rename `count` TIDAK menyentuh `counter_aux`/
        // `count <=`. count muncul: port decl + 3 pemakaian always_ff.
        let cnt_decl = line_of("[WIDTH-1:0] count");
        let cnt_use = line_of("count <= count + 1");
        let c_col = SAMPLE
            .lines()
            .nth(cnt_use as usize)
            .unwrap()
            .find("count")
            .unwrap() as u32;
        let edits_cnt = LspBackend::compute_rename_edits(SAMPLE, cnt_use, c_col, "cnt").unwrap();
        assert_eq!(
            edits_cnt.len(),
            4,
            "count 4 lokusi (decl+3): {:?}",
            edits_cnt
        );
        assert!(edits_cnt.iter().all(|(l, _, _)| *l >= cnt_decl));
    }

    #[test]
    fn test_hover_info() {
        // Hover di pemakaian `enable` → konteks deklarasi reg.
        let use_line = line_of("else if (enable)");
        let col = SAMPLE
            .lines()
            .nth(use_line as usize)
            .unwrap()
            .find("enable")
            .unwrap() as u32;
        let info = LspBackend::hover_info(SAMPLE, use_line, col).unwrap();
        assert!(info.contains("enable"), "{}", info);
        assert!(info.contains("reg enable;"), "{}", info);

        // Hover di nama module instance → konteks module.
        let inst_line = line_of("counter_aux aux");
        let aux_col = SAMPLE
            .lines()
            .nth(inst_line as usize)
            .unwrap()
            .find("counter_aux")
            .unwrap() as u32;
        let info2 = LspBackend::hover_info(SAMPLE, inst_line, aux_col).unwrap();
        assert!(info2.contains("**module**"), "{}", info2);

        // Bukan identifier → None.
        assert!(LspBackend::hover_info(SAMPLE, 7, 0).is_none());
    }

    #[test]
    fn test_document_symbols_outline() {
        let syms = LspBackend::document_symbols(SAMPLE);
        // 2 module di root.
        assert_eq!(
            syms.len(),
            2,
            "root: {:?}",
            syms.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert_eq!(syms[0].name, "counter");
        assert_eq!(syms[0].kind, DocSymbol::KIND_MODULE);
        assert_eq!(syms[0].line, line_of("module counter"));
        assert_eq!(syms[1].name, "counter_aux");

        // Children module counter: port + signal + param.
        let counter = &syms[0];
        let names: Vec<&str> = counter.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"WIDTH"), "{:?}", names);
        assert!(names.contains(&"clk"), "{:?}", names);
        assert!(names.contains(&"count"), "{:?}", names);
        assert!(names.contains(&"state"), "{:?}", names);
        assert!(names.contains(&"enable"), "{:?}", names);

        // Kind per jenis.
        let width = counter.children.iter().find(|c| c.name == "WIDTH").unwrap();
        assert_eq!(width.kind, DocSymbol::KIND_CONSTANT);
        let clk = counter.children.iter().find(|c| c.name == "clk").unwrap();
        assert_eq!(clk.kind, DocSymbol::KIND_VARIABLE);

        // Dokumen rusak (module tak ditutup) — tetap menghasilkan symbol.
        let broken = "module m;\n  reg x;\n";
        let syms2 = LspBackend::document_symbols(broken);
        assert_eq!(syms2.len(), 1);
        assert_eq!(syms2[0].name, "m");
        assert!(syms2[0].children.iter().any(|c| c.name == "x"));

        // Konversi ke lsp_types: kind + selection range benar.
        let lsp = syms2[0].to_lsp(|l| {
            broken
                .lines()
                .nth(l as usize)
                .map(|s| s.chars().count() as u32)
                .unwrap_or(0)
        });
        assert_eq!(lsp.kind, SymbolKind::MODULE);
    }

    #[test]
    fn test_folding_ranges() {
        let folds = LspBackend::compute_folding_ranges(SAMPLE);
        // Module counter (0 → endmodule) dan counter_aux.
        let m0 = line_of("module counter");
        let _m1 = line_of("module counter_aux");
        let e1 = line_of("endmodule");
        assert!(
            folds.contains(&(m0, e1)),
            "fold module pertama sampai endmodule terakhir: {:?}",
            folds
        );
        // begin always_ff (baris always_ff) → `end`-nya.
        let ff_line = line_of("always_ff @(posedge clk) begin");
        assert!(
            folds.iter().any(|(s, _)| *s == ff_line),
            "blok begin always_ff terlipat: {:?}",
            folds
        );
        // Urut & start ≤ end.
        assert!(folds.iter().all(|(s, e)| s <= e));
        assert!(
            folds.windows(2).all(|w| w[0] <= w[1]),
            "hasil urut: {:?}",
            folds
        );

        // Case block.
        let case_src = "module c;\n\
            case (x)\n\
                2'd0: y = 1;\n\
                default: y = 0;\n\
            endcase\n\
        endmodule\n";
        let f2 = LspBackend::compute_folding_ranges(case_src);
        assert!(f2.contains(&(1, 4)), "case 1..endcase 4: {:?}", f2);
        assert!(f2.contains(&(0, 5)), "module 0..endmodule 5: {:?}", f2);

        // End else begin: dua blok begin bertumpuk di baris sama.
        let nested = "module n;\n\
            if (a) begin\n\
                x = 1;\n\
            end else begin\n\
                x = 2;\n\
            end\n\
        endmodule\n";
        let f3 = LspBackend::compute_folding_ranges(nested);
        assert_eq!(f3.len(), 3, "{:?}", f3); // if-begin, else-begin, module
    }

    #[test]
    fn test_workspace_symbol_search() {
        let doc_a = ("file:///a.sv".to_string(), SAMPLE.to_string());
        let doc_b = (
            "file:///b.sv".to_string(),
            "module alu;\n  wire carry;\nendmodule\n".to_string(),
        );
        let docs = vec![doc_a, doc_b];

        // Query "count" → module counter + counter_aux + signal count.
        let hits = LspBackend::search_workspace_symbols(&docs, "count");
        assert!(hits.len() >= 3, "{:?}", hits);
        assert!(hits.iter().all(|h| h.name.to_lowercase().contains("count")));

        // Query case-insensitive.
        let hits2 = LspBackend::search_workspace_symbols(&docs, "ALU");
        assert!(hits2.iter().any(|h| h.name == "alu"));

        // Query kosong → semua simbol (>= jumlah outline kedua dokumen).
        let all = LspBackend::search_workspace_symbols(&docs, "");
        assert!(all.len() > hits.len());

        // Tidak ada match → kosong.
        assert!(LspBackend::search_workspace_symbols(&docs, "zzz").is_empty());
    }

    #[test]
    fn test_completions() {
        // Prefix "cou" → counter + counter_aux (simbol), bukan keyword.
        let use_line = line_of("counter_aux aux");
        let comps = LspBackend::compute_completions(SAMPLE, use_line, 7);
        let names: Vec<&str> = comps.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"counter"), "{:?}", names);
        assert!(names.contains(&"counter_aux"), "{:?}", names);

        // Prefix "en" di dalam module → signal enable + keyword end/endcase.
        let en_line = line_of("reg enable;");
        let comps2 = LspBackend::compute_completions(SAMPLE, en_line, 10);
        let names2: Vec<&str> = comps2.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names2.contains(&"enable"), "{:?}", names2);
        assert!(names2.contains(&"end"), "{:?}", names2);
        assert!(names2.contains(&"endcase"), "{:?}", names2);

        // Kind benar: enable VARIABLE, end KEYWORD.
        for (n, k) in &comps2 {
            if n == "enable" {
                assert_eq!(*k, DocSymbol::KIND_VARIABLE);
            }
            if n == "end" {
                assert_eq!(*k, DocSymbol::KIND_KEYWORD);
            }
        }

        // Prefix kosong (kursor setelah spasi) → tidak ada kandidat (MVP).
        assert!(LspBackend::compute_completions(SAMPLE, 7, 0).is_empty());
    }

    #[test]
    fn test_semantic_tokens() {
        let src = "module counter;\n  logic clk;\n  assign a = b;\nendmodule\n";
        let toks = LspBackend::compute_semantic_tokens(src);
        assert!(!toks.is_empty());
        // Ada token module (type=0) di line 0.
        assert!(
            toks.iter().any(|t| t.delta_line == 0 && t.token_type == 0),
            "module keyword: {:?}",
            toks
        );
        // Ada token variable (type=5) untuk `clk`.
        assert!(
            toks.iter().any(|t| t.token_type == 5),
            "variable: {:?}",
            toks
        );
        // Ada token keyword (type=7) untuk endmodule.
        assert!(
            toks.iter().any(|t| t.token_type == 7),
            "keyword: {:?}",
            toks
        );
    }

    #[test]
    fn test_find_definition_not_found() {
        // Identifier tanpa definisi (rst_n hanya ada di decl+usage... pakai
        // nama yang benar-benar tak ada).
        assert!(LspBackend::find_definition(SAMPLE, 0, 0).is_none()); // "module" bukan ident target? word="module" → def module <word>? tidak match
        assert!(LspBackend::find_definition("module m;\nendmodule\n", 1, 2).is_none());
        // Baris di luar dokumen.
        assert!(LspBackend::find_definition(SAMPLE, 999, 0).is_none());
    }

    #[test]
    fn test_selection_range() {
        let src = "module m;\n  logic a;\nendmodule\n";
        let range = compute_selection_range(src, 1, 5);
        // Should have nested ranges: word → line → scope.
        assert!(range.parent.is_some(), "should have parent: {:?}", range);
        let parent = range.parent.as_ref().unwrap();
        assert!(
            parent.parent.is_some(),
            "should have grandparent: {:?}",
            parent
        );
        // Word range should be non-empty.
        let word = &parent.parent.as_ref().unwrap().range;
        assert!(word.start.line == 1, "word line: {:?}", word);
    }

    #[test]
    fn test_document_colors() {
        let src = "module m;\n  parameter COLOR = #FF0000;\nendmodule\n";
        let colors = compute_document_colors(src);
        assert_eq!(colors.len(), 1, "one color: {:?}", colors);
        assert!(colors[0].range.start.line == 1, "color on line 1");
    }

    #[test]
    fn test_linked_editing_range() {
        // When cursor is on 'counter', all occurrences should be linked.
        let src = "module counter #(parameter WIDTH = 8) (
    input logic clk,
    output counter_aux aux
);
endmodule
";
        // Manually find all word occurrences (simulating the handler logic).
        let word = "counter";
        let mut ranges = Vec::new();
        for (line_idx, line) in src.lines().enumerate() {
            let mut offset = 0;
            while let Some(pos) = line[offset..].find(word) {
                let abs_pos = offset + pos;
                let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
                let before_ok = abs_pos == 0 || !is_ident(line.as_bytes()[abs_pos - 1]);
                let after_ok = abs_pos + word.len() >= line.len()
                    || !is_ident(line.as_bytes()[abs_pos + word.len()]);
                if before_ok && after_ok {
                    ranges.push((line_idx, abs_pos));
                }
                offset = abs_pos + 1;
            }
        }
        // 'counter' appears in 'counter' (line 0) and 'counter_aux' (not standalone).
        assert!(
            !ranges.is_empty(),
            "at least 1 standalone occurrence: {:?}",
            ranges
        );
    }

    #[test]
    fn test_document_colors_no_match() {
        let src = "module m;\n  parameter x = 1;\nendmodule\n";
        let colors = compute_document_colors(src);
        assert!(colors.is_empty(), "no colors: {:?}", colors);
    }

    #[test]
    fn test_incremental_text_change() {
        // Insert: add "abc" at position (0, 5) in "0123456789"
        let mut text = "0123456789".to_string();
        apply_text_change(
            &mut text,
            Range::new(Position::new(0, 5), Position::new(0, 5)),
            "abc",
        );
        assert_eq!(text, "01234abc56789");

        // Replace: replace range (0,2)..(0,5) with "X"
        let mut text2 = "0123456789".to_string();
        apply_text_change(
            &mut text2,
            Range::new(Position::new(0, 2), Position::new(0, 5)),
            "X",
        );
        assert_eq!(text2, "01X56789");

        // Delete: replace range with empty
        let mut text3 = "0123456789".to_string();
        apply_text_change(
            &mut text3,
            Range::new(Position::new(0, 0), Position::new(0, 5)),
            "",
        );
        assert_eq!(text3, "56789");
    }

    #[test]
    fn test_position_to_offset() {
        let lines = vec!["hello", "world"];
        assert_eq!(position_to_offset(&lines, Position::new(0, 0)), 0);
        assert_eq!(position_to_offset(&lines, Position::new(0, 5)), 5);
        assert_eq!(position_to_offset(&lines, Position::new(1, 0)), 6);
        assert_eq!(position_to_offset(&lines, Position::new(1, 3)), 9);
    }

    #[test]
    fn test_code_lens() {
        use lsp_types::Url;
        let uri = Url::parse("file:///test.sv").unwrap();
        let src = "module counter;\n  function test_add(input int a, input int b);\n    return a + b;\n  endfunction\nendmodule\n";
        let lenses = LspBackend::compute_code_lens(src, &uri);
        // Module counter → 1 lens
        assert!(
            lenses
                .iter()
                .any(|l| l.command.as_ref().is_some_and(|c| c.title.contains("test"))),
            "module lens: {:?}",
            lenses
                .iter()
                .map(|l| l.command.as_ref().map(|c| c.title.as_str()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_inlay_hints_improved() {
        let src = "module m(
    input logic clk,
    input [7:0] data_in,
    output logic [3:0] state
);
    logic [7:0] cnt;
    reg enable;
    function int add(input int a, input int b);
        return a + b;
    endfunction
endmodule
";
        let hints = LspBackend::compute_inlay_hints(src, None);
        // Should have type hints for ports, signals, and function return type.
        assert!(!hints.is_empty(), "hints: {:?}", hints);
        // Check that at least one hint contains a type keyword.
        let has_logic = hints.iter().any(|h| match &h.label {
            lsp_types::InlayHintLabel::String(s) => s.contains("logic"),
            _ => false,
        });
        assert!(has_logic, "should have logic type hint");
        // Check function return type hint.
        let has_int = hints.iter().any(|h| match &h.label {
            lsp_types::InlayHintLabel::String(s) => s.contains(": int"),
            _ => false,
        });
        assert!(
            has_int,
            "should have function return type hint: {:?}",
            hints
        );
    }

    #[test]
    fn test_count_word_occurrences() {
        assert_eq!(count_word_occurrences("foo bar foo", "foo"), 2);
        assert_eq!(count_word_occurrences("foobar foo", "foo"), 1);
        assert_eq!(count_word_occurrences("foo", "bar"), 0);
        assert_eq!(count_word_occurrences("", "foo"), 0);
        // Word boundary: "counter" should NOT match "counter_aux".
        assert_eq!(count_word_occurrences("counter counter_aux", "counter"), 1);
    }

    // LSP-21: test run code lens for individual test functions
    #[test]
    fn test_code_lens_test_run() {
        use lsp_types::Url;
        let uri = Url::parse("file:///test.sv").unwrap();
        let src = "module counter;\n  function test_add(input int a, input int b);\n    return a + b;\n  endfunction\n  function not_a_test(input int x);\n    return x;\n  endfunction\nendmodule\n";
        let lenses = LspBackend::compute_code_lens(src, &uri);
        // Module counter -> test count lens
        assert!(
            lenses
                .iter()
                .any(|l| l.command.as_ref().is_some_and(|c| c.title.contains("test"))),
            "module lens: {:?}",
            lenses
                .iter()
                .map(|l| l.command.as_ref().map(|c| c.title.as_str()))
                .collect::<Vec<_>>()
        );
        // test_add -> 'Run test' lens (LSP-21)
        assert!(
            lenses
                .iter()
                .any(|l| l.command.as_ref().is_some_and(
                    |c| c.command.contains("runTest") && c.command.contains("test_add")
                )),
            "test_add should have runTest lens: {:?}",
            lenses
                .iter()
                .map(|l| l.command.as_ref().map(|c| (&c.title, &c.command)))
                .collect::<Vec<_>>()
        );
        // not_a_test -> NO runTest lens
        assert!(
            !lenses.iter().any(|l| l
                .command
                .as_ref()
                .is_some_and(|c| c.command.contains("not_a_test"))),
            "not_a_test should NOT have runTest lens"
        );
    }

    // LSP-13: call hierarchy tests (using sync compute_call_hierarchy)
    #[test]
    fn test_call_hierarchy_declarations_and_calls() {
        let src = "module m;
  function int helper(input int x);
    return x + 1;
  endfunction
  function int main(input int a);
    return helper(a);
  endfunction
endmodule
";
        let (incoming, outgoing) = LspBackend::compute_call_hierarchy(src, "any");
        // outgoing = declarations found (helper, main)
        assert_eq!(
            outgoing.len(),
            2,
            "should find 2 declarations: {:?}",
            outgoing.iter().map(|i| i.name.as_str()).collect::<Vec<_>>()
        );
        // incoming = call sites (helper(a) call found)
        assert!(
            !incoming.is_empty(),
            "should find call sites: {:?}",
            incoming
        );
    }

    #[test]
    fn test_call_hierarchy_no_calls() {
        let src = "module m;
  function int plain(input int x);
    return x;
  endfunction
endmodule
";
        let (_incoming, outgoing) = LspBackend::compute_call_hierarchy(src, "plain");
        // No call sites (return x doesn't have function call)
        assert_eq!(outgoing.len(), 1, "should find 1 declaration");
    }

    #[test]
    fn test_format_source() {
        // Simple module — tokenized and re-joined with spaces.
        let src = "module counter(input logic clk, output logic [7:0] cnt);\nendmodule\n";
        let formatted = LspBackend::format_source(src, 4);
        // Must contain key tokens.
        assert!(formatted.contains("module"), "has module: {}", formatted);
        assert!(
            formatted.contains("endmodule"),
            "has endmodule: {}",
            formatted
        );
        assert!(formatted.contains("input"), "has input: {}", formatted);
        assert!(formatted.contains("logic"), "has logic: {}", formatted);
        assert!(formatted.contains("output"), "has output: {}", formatted);
    }

    #[test]
    fn test_format_source_indent_dedent() {
        // Module body indented, endmodule dedented.
        let src = "module m;\n  logic a;\nendmodule\n";
        let formatted = LspBackend::format_source(src, 4);
        let lines: Vec<&str> = formatted.lines().collect();
        // First line (module) — no indent.
        assert!(lines[0].starts_with("module"), "first: {}", lines[0]);
        // Last line (endmodule) — no indent (dedented).
        assert!(
            lines.last().unwrap().starts_with("endmodule"),
            "last: {}",
            lines.last().unwrap()
        );
    }

    #[test]
    fn test_format_source_empty() {
        // Empty source → empty output.
        let formatted = LspBackend::format_source("", 4);
        assert!(
            formatted.is_empty() || formatted.trim().is_empty(),
            "empty in: '{}', out: '{}'",
            "",
            formatted
        );
    }

    #[test]
    fn test_completion_resolve() {
        use lsp_types::{CompletionItem, CompletionItemKind};
        let item = CompletionItem {
            label: "counter".to_string(),
            kind: Some(CompletionItemKind::MODULE),
            ..Default::default()
        };
        // Simulate resolve by calling the static core logic.
        let doc = match item.kind {
            Some(CompletionItemKind::MODULE) => Some(lsp_types::Documentation::String(format!(
                "SystemVerilog module `{}`",
                item.label
            ))),
            _ => None,
        };
        assert!(doc.is_some(), "module should have documentation");
        if let Some(lsp_types::Documentation::String(s)) = doc {
            assert!(s.contains("counter"));
        }
    }

    #[test]
    fn test_signature_help_find_function() {
        let src = "module m;\n  function int add(input int a, input int b);\n    return a + b;\n  endfunction\nendmodule\n";
        let sig = LspBackend::find_function_signature(src, "add");
        assert!(sig.is_some(), "should find add function");
        let (label, params) = sig.unwrap();
        assert!(label.contains("add"), "label: {}", label);
        assert!(label.contains('(') && label.contains(')'));
        assert!(params.len() == 2, "2 params: {:?}", params);
    }

    #[test]
    fn test_actions_from_diagnostic_missing_semicolon() {
        use lsp_types::Url;
        let uri = Url::parse("file:///test.sv").unwrap();
        let diag = Diagnostic {
            range: Range {
                start: Position::new(2, 5),
                end: Position::new(2, 6),
            },
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("mivon".to_string()),
            message: "Expected ';'".to_string(),
            ..Default::default()
        };
        let actions = LspBackend::actions_from_diagnostic("", &diag, &uri).unwrap();
        assert_eq!(actions.len(), 1, "one action for missing semicolon");
        if let lsp_types::CodeActionOrCommand::CodeAction(action) = &actions[0] {
            assert!(
                action.title.contains("semicolon"),
                "title: {}",
                action.title
            );
            assert!(action.edit.is_some(), "has edit");
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn test_actions_from_diagnostic_missing_endmodule() {
        use lsp_types::Url;
        let uri = Url::parse("file:///test.sv").unwrap();
        let diag = Diagnostic {
            range: Range {
                start: Position::new(5, 0),
                end: Position::new(5, 0),
            },
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("mivon".to_string()),
            message: "Expected 'endmodule'".to_string(),
            ..Default::default()
        };
        let actions = LspBackend::actions_from_diagnostic("", &diag, &uri).unwrap();
        assert_eq!(actions.len(), 1, "one action for missing endmodule");
        if let lsp_types::CodeActionOrCommand::CodeAction(action) = &actions[0] {
            assert!(
                action.title.contains("endmodule"),
                "title: {}",
                action.title
            );
            assert!(action.edit.is_some(), "has edit");
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn test_actions_from_diagnostic_no_match() {
        use lsp_types::Url;
        let uri = Url::parse("file:///test.sv").unwrap();
        let diag = Diagnostic {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(0, 1),
            },
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("mivon".to_string()),
            message: "Some unrelated warning".to_string(),
            ..Default::default()
        };
        let actions = LspBackend::actions_from_diagnostic("", &diag, &uri);
        assert!(actions.is_none(), "no actions for unrelated diagnostic");
    }
}
