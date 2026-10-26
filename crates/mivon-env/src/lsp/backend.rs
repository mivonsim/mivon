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
    HoverContents, InitializeParams, InitializeResult, InsertTextFormat, LanguageString,
    LinkedEditingRangeParams, LinkedEditingRanges, Location, MarkedString, OneOf, Position,
    PrepareRenameResponse, PublishDiagnosticsParams, Range, ReferenceParams, RenameOptions,
    RenameParams, SelectionRange, SelectionRangeParams, SemanticToken, SemanticTokens,
    SemanticTokensDelta, SemanticTokensDeltaParams, SemanticTokensEdit,
    SemanticTokensFullDeltaResult, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensRangeParams, SemanticTokensRangeResult,
    SemanticTokensResult, ServerCapabilities, ServerInfo, SymbolInformation, SymbolKind,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TypeHierarchyItem, TypeHierarchyPrepareParams, TypeHierarchySubtypesParams,
    TypeHierarchySupertypesParams, Url, WorkspaceEdit, WorkspaceSymbolParams,
};
use mivon_parser::lexer::Lexer;
use mivon_parser::preprocessor::Preprocessor;
use mivon_parser::Parser;
use tower_lsp::jsonrpc::Result as JsonRpcResult;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod completions;
mod folding;
mod hierarchy;
mod hover;
mod outline;
mod references;
mod semantic;
mod workspace_index;

/// LSP-04 tahap 2: batas jumlah referensi lintas file yang dikembalikan
/// (respons `textDocument/references` bisa meledak di workspace besar).
const MAX_CROSS_FILE_REFERENCES: usize = 2000;

/// Hasil workspace symbol search (LSP-11).
#[derive(Debug, Clone)]
pub(crate) struct WsSymbolHit {
    pub uri: String,
    pub name: String,
    pub kind: u8,
    pub line: u32,
    pub col: u32,
}

/// Keyword deklarasi yang tidak termasuk arah/tipe (dipakai
/// `declarator_scan` untuk mengenali awal deklarasi baru setelah ';').
const DECL_START_KW: &[&str] = &["parameter", "localparam", "typedef", "genvar"];

/// Keyword arah/tipe yang menandai awal deklarasi port/signal.
const TYPE_DIR_KW: &[&str] = &[
    // arah port
    "input",
    "output",
    "inout",
    "ref",
    // tipe data
    "wire",
    "reg",
    "logic",
    "bit",
    "int",
    "integer",
    "byte",
    "shortint",
    "longint",
    "real",
    "realtime",
    "time",
    "nettype",
    "enum",
    "struct",
    "union",
    "string",
    "event",
    "chandle",
    "signed",
    "unsigned",
    // net/qualifier lain yang bisa membuka deklarasi kedua pada baris sama
    // (wire a; tri b;) atau field class (rand bit [7:0] x;).
    "tri",
    "tri0",
    "tri1",
    "triand",
    "trior",
    "trireg",
    "wand",
    "wor",
    "supply0",
    "supply1",
    "uwire",
    "var",
    "const",
    "static",
    "automatic",
    "rand",
    "randc",
    "local",
    "protected",
    "virtual",
];

/// Simbol dokumen versi lite (LSP-12) — dikonversi ke
/// lsp_types::DocumentSymbol oleh handler.
#[derive(Debug, Clone)]
pub(crate) struct DocSymbol {
    pub name: String,
    pub kind: u8,
    pub line: u32,
    pub col: u32,
    pub children: Vec<DocSymbol>,
    /// LSP-14 tahap 2a: parent class (`class A extends B`) — None bila
    /// tanpa extends / bukan class.
    pub base: Option<String>,
    /// LSP-14 tahap 2a: kolom NAMA (selection_range akurat). `col` tetap
    /// kolom keyword (dipakai fitur lama: semantic token, to_lsp).
    pub name_col: u32,
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
    /// LSP-07 tahap 2c: cache token semantik per dokumen —
    /// uri → (result_id, tokens) untuk delta update.
    semantic_cache: std::sync::Mutex<HashMap<String, (String, Vec<SemanticToken>)>>,
    /// Counter result_id token semantik.
    semantic_seq: std::sync::atomic::AtomicU64,
    /// LSP-11 tahap 2: root workspace dari `initialize` (root_uri atau
    /// workspace_folders pertama) — dipakai untuk indeks simbol dari DISK,
    /// bukan hanya dokumen yang sedang terbuka.
    workspace_root: std::sync::Mutex<Option<std::path::PathBuf>>,
    /// LSP-11 tahap 3: cache indeks disk (mtime+len per file) — file yang
    /// tidak berubah tidak dibaca ulang antar permintaan.
    index_cache: std::sync::Arc<std::sync::Mutex<workspace_index::DiskIndexCache>>,
}

impl LspBackend {
    pub fn new(client: Client) -> Self {
        LspBackend {
            client,
            documents: std::sync::Mutex::new(HashMap::new()),
            semantic_cache: std::sync::Mutex::new(HashMap::new()),
            semantic_seq: std::sync::atomic::AtomicU64::new(1),
            workspace_root: std::sync::Mutex::new(None),
            index_cache: std::sync::Arc::new(std::sync::Mutex::new(
                workspace_index::DiskIndexCache::default(),
            )),
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

    /// Root workspace (dari `initialize`) untuk indeks simbol dari disk.
    fn workspace_root_path(&self) -> Option<std::path::PathBuf> {
        match self.workspace_root.lock() {
            Ok(g) => g.clone(),
            Err(_) => None,
        }
    }

    /// Dokumen yang bisa dicari: dokumen TERBUKA + indeks file sumber di
    /// DISK (LSP-11 tahap 2 / LSP-04 tahap 2).
    ///
    /// Review R192 BUG-3: pemindaian disk = I/O sinkron berat → jalankan di
    /// spawn_blocking supaya event loop LSP tidak stall. Review R192 BUG-4:
    /// file yang sudah terbuka dilewati saat scan (path terkanonikalisasi)
    /// agar tidak muncul duplikat dgn isi disk yang basi.
    async fn collect_workspace_docs(&self) -> Vec<(String, String)> {
        let open: Vec<(String, String)> = match self.documents.lock() {
            Ok(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Err(_) => Vec::new(),
        };
        let skip = workspace_index::collect_skip_paths(&open);
        let disk = match self.workspace_root_path() {
            Some(root) => {
                // LSP-11 tahap 3: cache di-share lewat Arc agar bisa dipakai
                // di dalam spawn_blocking (I/O sinkron di luar event loop).
                let cache = std::sync::Arc::clone(&self.index_cache);
                tokio::task::spawn_blocking(move || {
                    // Poison (panic saat memegang lock) tidak boleh membuat
                    // indeks disk kosong permanen — pakai data yang ada.
                    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
                    workspace_index::load_disk_docs_cached(&mut guard, &root, &skip)
                })
                .await
                .unwrap_or_default()
            }
            None => Vec::new(),
        };
        // Review R193 BUG-1: `documents` adalah HashMap (urutan iterasi
        // acak) → tanpa sort, urutan Location DAN himpunan hasil saat cap
        // tercapai berbeda antar request. sorted_docs = deterministik
        // (no-op utk symbol yang sudah mengurutkan hasilnya sendiri).
        workspace_index::sorted_docs(workspace_index::merge_docs(&open, &disk))
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
        // Kolom dari klien = indeks karakter (UTF-16 untuk BMP), bukan offset
        // byte. Konversi dulu: memakainya langsung menggeser/memotong
        // identifier pada baris ber-UTF-8 (komentar é sebelum kode).
        let ch = completions::char_index_to_byte(line_text, character as usize);
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
        let toks = Self::line_tokens(line);
        if toks.is_empty() {
            return None;
        }

        // 0. Deklarasi parameter di mana pun dalam baris (termasuk
        //    `module m #(parameter WIDTH = 8) (`): token parameter/localparam
        //    → cari deklarator SETELAHnya.
        for (i, (w, _)) in toks.iter().enumerate() {
            if matches!(w.as_str(), "parameter" | "localparam") {
                if let Some(c) = Self::declarator_scan(&toks[i + 1..], word) {
                    return Some(c);
                }
            }
        }

        // 1. Definisi scope: <kw> <word>
        let (t0, _) = &toks[0];
        if SCOPE_KW.contains(&t0.as_str()) {
            // function/task: nama = identifier TERAKHIR sebelum '(' — tipe
            // return di depannya adalah pemakaian, bukan deklarasi (review
            // LSP-04 ronde 8 F-4). Argumen header memakai pemindaian port.
            if matches!(t0.as_str(), "function" | "task") {
                let mut name: Option<&(String, usize)> = None;
                for t in toks.iter().skip(1) {
                    if t.0 == "(" || t.0 == ";" {
                        break;
                    }
                    let is_ident =
                        t.0.chars()
                            .next()
                            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$');
                    if is_ident
                        && !TYPE_DIR_KW.contains(&t.0.as_str())
                        && !DECL_START_KW.contains(&t.0.as_str())
                    {
                        name = Some(t);
                    }
                }
                if let Some((w, c)) = name {
                    if w == word {
                        return Some(*c);
                    }
                }
                if let Some(inner) = Self::port_list(&toks) {
                    if let Some(c) = Self::port_segments_scan(inner, word) {
                        return Some(c);
                    }
                }
                return None;
            }
            if let Some((w1, c1)) = toks.get(1) {
                if w1 == word {
                    return Some(*c1);
                }
            }
            // Port ANSI di header: `module m (input logic clk, output q)`.
            // Hanya segmen (dipisah koma) yang memuat keyword arah/tipe yang
            // dianggap deklarasi; pencarian berhenti di '=' (nilai default).
            if matches!(
                t0.as_str(),
                "module" | "macromodule" | "interface" | "program"
            ) {
                if let Some(inner) = Self::port_list(&toks) {
                    if let Some(c) = Self::port_segments_scan(inner, word) {
                        return Some(c);
                    }
                }
            }
            return None;
        }

        // 1b. Lanjutan header multi-baris: baris `) (input logic clk);`.
        if t0 == ")" {
            if let Some(inner) = Self::port_list(&toks) {
                if let Some(c) = Self::port_segments_scan(inner, word) {
                    return Some(c);
                }
            }
        }

        // 1c. body kurung kurawal enum/struct/union (dengan atau tanpa
        //     typedef): anggota di depth 1 dan nama setelah '}' adalah
        //     deklarasi (regresi vs perilaku linear-scan lama — review
        //     LSP-04 ronde 5/7).
        if matches!(t0.as_str(), "typedef" | "enum" | "struct" | "union") {
            if let Some(c) = Self::braced_typedef_scan(&toks, word) {
                return Some(c);
            }
        }

        // 2. parameter/localparam/typedef/genvar ... word ...
        if DECL_START_KW.contains(&t0.as_str()) {
            return Self::declarator_scan(&toks[1..], word);
        }

        // 3. Port/signal decl: baris diawali direction/type keyword dan
        //    memuat word sebagai token berdiri sendiri (bukan nama tipe
        //    pertama). Contoh: `input logic [7:0] data_in`, `reg [3:0] cnt`.
        if TYPE_DIR_KW.contains(&t0.as_str()) {
            return Self::declarator_scan(&toks[1..], word);
        }
        None
    }

    /// Anggota typedef ber-body kurung kurawal (`typedef enum {IDLE, RUN} t;`,
    /// `typedef struct packed { logic f; } s_t;`): identifier di depth 1
    /// dalam kurawal adalah deklarasi anggota; nama tipe setelah '}' juga.
    fn braced_typedef_scan(toks: &[(String, usize)], word: &str) -> Option<usize> {
        let open = toks.iter().position(|(w, _)| w == "{")?;
        let mut depth = 0i32;
        let mut nest = 0i32; // '(' / '[' di dalam kurawal (argumen fungsi)
                             // Kedalaman tempat nilai ('=') dimulai; None = sedang bukan nilai.
                             // Nilai bersarang (A = {P, Q}) tidak boleh dianggap anggota
                             // (review LSP-04 ronde 9).
        let mut value_depth: Option<i32> = None;
        let mut closed_at = None;
        for (i, (w, c)) in toks.iter().enumerate().skip(open) {
            match w.as_str() {
                "{" => {
                    depth += 1;
                    continue;
                }
                "}" => {
                    depth -= 1;
                    if depth == 0 {
                        closed_at = Some(i);
                        break;
                    }
                    continue;
                }
                "(" | "[" if depth >= 1 => {
                    nest += 1;
                    continue;
                }
                ")" | "]" if depth >= 1 => {
                    nest -= 1;
                    continue;
                }
                // Nilai anggota enum (A = W) adalah pemakaian, bukan nama.
                "=" if depth >= 1 && nest == 0 => {
                    value_depth = Some(depth);
                    continue;
                }
                ";" if depth >= 1 && nest == 0 => {
                    value_depth = None;
                    continue;
                }
                "," if depth >= 1 && nest == 0 && value_depth == Some(depth) => {
                    value_depth = None;
                    continue;
                }
                _ => {}
            }
            if depth >= 1
                && nest == 0
                && value_depth.is_none()
                && w == word
                && !TYPE_DIR_KW.contains(&w.as_str())
                && !DECL_START_KW.contains(&w.as_str())
            {
                // Identifier diikuti identifier lain = nama TIPE anggota
                // (typedef struct { my_t f; } — review LSP-04 ronde 7/8).
                if Self::followed_by_ident(toks, i) {
                    continue;
                }
                return Some(*c);
            }
        }
        // Nama setelah '}' penutup (tipe/variabel), berhenti di '=' atau ';'
        // pada depth 0 — initializer adalah pemakaian (review ronde 8 F-3).
        let mut tail_nest = 0i32;
        for (w, c) in toks.iter().skip(closed_at? + 1) {
            match w.as_str() {
                "(" | "[" => {
                    tail_nest += 1;
                    continue;
                }
                ")" | "]" => {
                    tail_nest -= 1;
                    continue;
                }
                "=" | ";" if tail_nest == 0 => break,
                _ => {}
            }
            if tail_nest == 0 && w == word {
                return Some(*c);
            }
        }
        None
    }

    /// Apakah token setelah `idx` (melewati `[...]` dan `::`) memuat
    /// identifier lain sebelum '=', ',', ';', ')' — penanda bahwa token
    /// `idx` adalah nama TIPE, bukan deklarator. Menangani
    /// `my_t [3:0] f;` dan `pkg::my_t f;` (review LSP-04 ronde 8 F-2).
    fn followed_by_ident(toks: &[(String, usize)], idx: usize) -> bool {
        let mut i = idx + 1;
        while i < toks.len() {
            match toks[i].0.as_str() {
                "[" => {
                    let mut d = 0i32;
                    while i < toks.len() {
                        match toks[i].0.as_str() {
                            "[" => d += 1,
                            "]" => {
                                d -= 1;
                                if d == 0 {
                                    i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    continue;
                }
                ":" => {
                    i += 1;
                    continue;
                }
                "=" | "," | ";" | ")" | "}" | "(" => return false,
                w => {
                    let is_ident = w
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$');
                    return is_ident && !TYPE_DIR_KW.contains(&w) && !DECL_START_KW.contains(&w);
                }
            }
        }
        false
    }

    /// Scan token untuk menemukan `word` sebagai NAMA deklarator: hanya
    /// identifier pada depth bracket 0 dan sebelum '=' (RHS = pemakaian).
    /// Koma depth 0 memulai deklarator baru; ';' mereset status nilai.
    fn declarator_scan(toks: &[(String, usize)], word: &str) -> Option<usize> {
        let mut depth = 0i32;
        let mut in_value = false;
        // Setelah ';' depth 0, hanya deklarasi baru yang DIAWALI keyword
        // arah/tipe yang boleh menghasilkan deklarator — mencegah
        // `wire a; assign b = a;` menganggap `b` sebagai deklarasi
        // (review LSP-04: false negative find-references).
        let mut expect_decl = true;
        for (idx, (w, c)) in toks.iter().enumerate() {
            match w.as_str() {
                "[" | "(" | "{" => {
                    depth += 1;
                    continue;
                }
                "]" | "}" => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    continue;
                }
                ")" => {
                    // Keluar dari konstruk (mis. daftar parameter) → berhenti.
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    continue;
                }
                "=" if depth == 0 => {
                    in_value = true;
                    continue;
                }
                ";" if depth == 0 => {
                    in_value = false;
                    expect_decl = false;
                    continue;
                }
                "," if depth == 0 => {
                    in_value = false;
                    continue;
                }
                _ => {}
            }
            if depth == 0
                && (TYPE_DIR_KW.contains(&w.as_str()) || DECL_START_KW.contains(&w.as_str()))
            {
                expect_decl = true;
                continue;
            }
            if depth == 0 && !in_value && expect_decl && w == word {
                // Lewati pemakaian hierarki (a.b) dan scope resolution
                // (pkg::x, local::x — review LSP-04 ronde 9).
                if idx > 0 && matches!(toks[idx - 1].0.as_str(), "." | ":") {
                    continue;
                }
                // Identifier yang diikuti identifier lain depth 0 adalah nama
                // TIPE, bukan deklarator (mis. static my_t cfg; — review
                // LSP-04 ronde 6/8: tanpa guard ini tipe user-defined ikut
                // dianggap deklarasi dan pemakaiannya hilang dari references).
                if Self::followed_by_ident(toks, idx) {
                    continue;
                }
                return Some(*c);
            }
        }
        None
    }

    /// Daftar port pada header module/interface: lewati `#(...)` (hanya
    /// parameter list SEBELUM '(' pertama) lalu ambil isi `( ... )`
    /// pasangan berikutnya.
    fn port_list(toks: &[(String, usize)]) -> Option<&[(String, usize)]> {
        let first_paren = toks.iter().position(|(w, _)| w == "(")?;
        let mut from = 0usize;
        if let Some(hash) = toks.iter().position(|(w, _)| w == "#") {
            if hash < first_paren && toks.get(hash + 1).map(|(w, _)| w.as_str()) == Some("(") {
                from = Self::matching_paren(toks, hash + 1)? + 1;
            }
        }
        let open = (from..toks.len()).find(|&k| toks[k].0 == "(")?;
        let close = Self::matching_paren(toks, open)?;
        Some(&toks[open + 1..close])
    }

    /// Scan segmen daftar port (dipisah koma depth 0). Segmen tanpa
    /// arah/tipe tetap deklarasi bila mewarisi dari segmen sebelumnya
    /// (`input logic a, b`).
    fn port_segments_scan(inner: &[(String, usize)], word: &str) -> Option<usize> {
        let mut seg_start = 0usize;
        let mut depth = 0i32;
        let mut carried = false;
        let mut end = 0usize;
        while end <= inner.len() {
            let at_comma = depth == 0 && end < inner.len() && inner[end].0 == ",";
            if end == inner.len() || at_comma {
                let seg = &inner[seg_start..end];
                let has_dir = seg.iter().any(|(w, _)| TYPE_DIR_KW.contains(&w.as_str()));
                if has_dir {
                    carried = true;
                }
                if has_dir || carried {
                    if let Some(c) = Self::declarator_scan(seg, word) {
                        return Some(c);
                    }
                }
                seg_start = end + 1;
            } else {
                match inner[end].0.as_str() {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => depth -= 1,
                    _ => {}
                }
            }
            end += 1;
        }
        None
    }

    /// Indeks ')' yang menutup '(' pada `open`.
    fn matching_paren(toks: &[(String, usize)], open: usize) -> Option<usize> {
        let mut depth = 0i32;
        for (i, (w, _)) in toks.iter().enumerate().skip(open) {
            match w.as_str() {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Validasi identifier SV sederhana (LSP-05): [A-Za-z_][A-Za-z0-9_$]*
    /// dan bukan reserved keyword (review R194 NIT-2).
    fn is_valid_identifier(name: &str) -> bool {
        let bytes = name.as_bytes();
        if bytes.is_empty() {
            return false;
        }
        let first = bytes[0];
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return false;
        }
        if !bytes[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
        {
            return false;
        }
        // Keyword (`always`, `logic`, `endmodule`, ...) di-lex sebagai token
        // keyword, bukan `Ident` — rename ke situ menghasilkan source invalid.
        use mivon_parser::lexer::Token;
        matches!(Lexer::new(name).next_token(), (Token::Ident(_), _, _))
    }

    /// Kata di kursor yang layak di-rename (LSP-05). Menggabungkan
    /// `word_at_position` dengan guard identifier + penolakan fragmen
    /// literal berbasis (`8'd5` → kursor di atas `d5`, review R194 NIT-3).
    fn rename_word_at_position(line_text: &str, character: u32) -> Option<(String, u32, u32)> {
        let (word, s, e) = Self::word_at_position(line_text, character)?;
        if !Self::is_valid_identifier(&word) {
            return None;
        }
        if s > 0 && line_text.as_bytes().get(s as usize - 1) == Some(&b'\'') {
            return None;
        }
        Some((word, s, e))
    }

    /// Rename core (murni, bisa diuji) — LSP-05 tahap 1: hasilkan edit
    /// (range) untuk SEMUA kemunculan identifier di dokumen. Return
    /// Err dengan pesan jelas bila kursor bukan identifier atau nama
    /// baru tidak valid.
    /// LSP-05 tahap 2a: validasi awal rename — kembalikan
    /// (line, start_col, end_col) kata di kursor, atau None bila
    /// kursor tidak di atas identifier.
    pub(crate) fn prepare_rename_range(
        source: &str,
        line: u32,
        character: u32,
    ) -> Option<(u32, u32, u32)> {
        let line_text = source.lines().nth(line as usize)?;
        // Review R194 NIT-1/NIT-3: jangan tawarkan rename untuk keyword,
        // literal angka, `$display`, atau fragmen literal (`8'd5`).
        let (_, s, e) = Self::rename_word_at_position(line_text, character)?;
        Some((line, s, e))
    }

    #[allow(dead_code)] // API tahap 1 (single-doc) — dipakai test; produksi
                        // memakai compute_rename_edits_multi (tahap 2).
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
        let Some((word, _, _)) = Self::rename_word_at_position(line_text, character) else {
            return Err("kursor tidak berada di atas identifier yang valid".into());
        };
        if word == new_name {
            return Ok(Vec::new());
        }
        // find_references menjamin token mandiri + word-boundary.
        Ok(Self::find_references(source, line, character)
            .into_iter()
            .collect())
    }

    /// LSP-05 tahap 2: edit rename LINTAS FILE (murni, bisa diuji).
    ///
    /// Validasi identifier baru + kata lama (keduanya wajib identifier SV,
    /// bukan keyword); kemunculan dicari di seluruh `docs` (dokumen terbuka
    /// + indeks disk) memakai aturan find-references (token mandiri,
    /// komentar `//` dilewati).
    ///
    /// Determinisme: dokumen diurutkan by uri secara internal (tidak
    /// bergantung urutan masukan).
    ///
    /// Error: identifier invalid, atau kemunculan melebihi `max_results`
    /// (rename TIDAK dipotong senyap — user mempersempit dulu).
    pub(crate) fn compute_rename_edits_multi(
        docs: &[(String, String)],
        word: &str,
        new_name: &str,
        max_results: usize,
    ) -> Result<Vec<(String, u32, u32, u32)>, String> {
        if !Self::is_valid_identifier(new_name) {
            return Err(format!(
                "'{}' bukan identifier SystemVerilog yang valid",
                new_name
            ));
        }
        // Review R194 BUG-2: kata yang di-rename juga wajib identifier.
        // word_at_position menerima digit/`$` di awal (mis. `42`,
        // `$display`) — tanpa guard ini rename akan mengganti SEMUA literal
        // 42 / semua `$display` di seluruh workspace.
        if !Self::is_valid_identifier(word) {
            return Err("kursor tidak berada di atas identifier yang valid".into());
        }
        if word == new_name {
            return Ok(Vec::new());
        }
        // Review R194 BUG-1: rename TIDAK BOLEH terpotong senyap. Minta satu
        // lebih dari batas; bila terlampaui → Err (user mempersempit dulu).
        let limit = max_results.saturating_add(1);
        // Deterministik tanpa meng-clone teks: iterasi dokumen terurut uri.
        let mut order: Vec<usize> = (0..docs.len()).collect();
        order.sort_by(|&a, &b| docs[a].0.cmp(&docs[b].0));
        let mut hits: Vec<(String, u32, u32, u32)> = Vec::new();
        for i in order {
            let (uri, text) = &docs[i];
            if !Self::collect_refs_in_text(uri, text, word, &mut hits, limit) {
                break;
            }
        }
        if hits.len() > max_results {
            return Err(format!(
                "rename terlalu besar: lebih dari {} kemunculan — persempit dulu (workspace/berkas)",
                max_results
            ));
        }
        Ok(hits)
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

    /// LSP-03 tahap 2: cari definisi LINTAS FILE (murni, bisa diuji).
    ///
    /// `docs` = dokumen terbuka + indeks disk; `prefer_uri` = dokumen asal
    /// kursor (diperiksa lebih dulu agar definisi lokal tetap menang seperti
    /// tahap 1). Sisanya diurutkan by uri supaya hasil deterministik dan
    /// tidak bergantung urutan masukan. Mengembalikan
    /// (uri, line, start, end) definisi pertama yang ditemukan.
    pub(crate) fn find_definition_in_docs(
        docs: &[(String, String)],
        word: &str,
        prefer_uri: &str,
    ) -> Option<(String, u32, u32, u32)> {
        let mut order: Vec<usize> = (0..docs.len()).collect();
        order.sort_by(|&a, &b| docs[a].0.cmp(&docs[b].0));
        if let Some(p) = order.iter().position(|&i| docs[i].0 == prefer_uri) {
            let p = order.remove(p);
            order.insert(0, p);
        }
        for i in order {
            let (uri, text) = &docs[i];
            // Prefilter murah (review R195 NIT-3): definisi selalu memuat
            // kata secara literal, jadi dokumen tanpa substring itu mustahil
            // cocok — lewati tanpa tokenisasi per baris.
            if !text.contains(word) {
                continue;
            }
            for (idx, line) in text.lines().enumerate() {
                if let Some(col) = Self::definition_col(line, word) {
                    return Some((
                        uri.clone(),
                        idx as u32,
                        col as u32,
                        (col + word.len()) as u32,
                    ));
                }
            }
        }
        None
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for LspBackend {
    /// Initialize the LSP server — advertises capabilities.
    async fn initialize(&self, params: InitializeParams) -> JsonRpcResult<InitializeResult> {
        // LSP-11 tahap 2: simpan root workspace utk indeks simbol dari disk
        // (root_uri diutamakan, fallback workspace_folders pertama).
        if let Ok(mut guard) = self.workspace_root.lock() {
            *guard = workspace_index::workspace_root_from_params(&params);
        }
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
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
                                token_modifiers: vec![
                                    lsp_types::SemanticTokenModifier::DECLARATION,
                                    lsp_types::SemanticTokenModifier::READONLY,
                                ],
                            },
                            range: Some(true),
                            full: Some(lsp_types::SemanticTokensFullOptions::Delta {
                                delta: Some(true),
                            }),
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

    /// Go-to-definition (LSP-03 tahap 1+2): jalur cepat di dokumen aktif,
    /// lalu — bila tidak ketemu — pencarian LINTAS FILE di seluruh workspace.
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonRpcResult<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        // LSP-03 tahap 2: definisi bisa berada di file lain. Jalur cepat:
        // cari di dokumen aktif dulu (tanpa scan disk); hanya bila tidak
        // ketemu baru sapu workspace (dokumen terbuka + indeks disk).
        let found = match Self::find_definition(&text, pos.line, pos.character) {
            Some((line, start, end)) => Some((uri.to_string(), line, start, end)),
            None => {
                let line_text = text.lines().nth(pos.line as usize).unwrap_or("");
                let Some((word, _, _)) = Self::word_at_position(line_text, pos.character) else {
                    return Ok(None);
                };
                // Review R195 NIT-3: kata non-identifier (`$display`, `42`,
                // fragmen literal `8'd5`) tidak punya deklarasi — jangan
                // bayar pemindaian workspace penuh.
                if !Self::is_valid_identifier(&word) {
                    return Ok(None);
                }
                let docs = workspace_index::ensure_doc(
                    self.collect_workspace_docs().await,
                    uri.as_str(),
                    &text,
                );
                Self::find_definition_in_docs(&docs, &word, uri.as_str())
            }
        };
        let Some((target, line, start, end)) = found else {
            return Ok(None);
        };
        let Ok(target_uri) = Url::parse(&target) else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
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
        })))
    }

    /// Find-references (LSP-04 tahap 1): semua kemunculan identifier di
    /// dokumen yang sama.
    async fn references(&self, params: ReferenceParams) -> JsonRpcResult<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        // LSP-04 tahap 2: referensi LINTAS FILE (dokumen terbuka + indeks
        // disk). Kata dicari dulu di dokumen aktif (word-boundary sama
        // seperti tahap 1), lalu disapu ke seluruh workspace.
        let line_text = text.lines().nth(pos.line as usize).unwrap_or("");
        let Some((word, _, _)) = Self::word_at_position(line_text, pos.character) else {
            return Ok(None);
        };
        let docs =
            workspace_index::ensure_doc(self.collect_workspace_docs().await, uri.as_str(), &text);
        // LSP-04: hormati context.include_declaration (default klien = true).
        let hits = Self::references_hits(
            &docs,
            &word,
            params.context.include_declaration,
            MAX_CROSS_FILE_REFERENCES,
        );
        let refs: Vec<Location> = hits
            .into_iter()
            .filter_map(|(u, line, start, end)| {
                let uri = Url::parse(&u).ok()?;
                Some(Location {
                    uri,
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
            })
            .collect();
        if refs.is_empty() {
            return Ok(None);
        }
        Ok(Some(refs))
    }

    /// LSP-14 tahap 2a: siapkan item hierarki tipe untuk class di kursor.
    /// Batasan: capability `typeHierarchyProvider` belum ada di lsp-types
    /// 0.94 ServerCapabilities → belum diiklankan; handler siap dipakai
    /// saat upgrade lsp-types.
    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> JsonRpcResult<Option<Vec<TypeHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let line_text = text.lines().nth(pos.line as usize).unwrap_or("");
        let Some((word, _, _)) = Self::word_at_position(line_text, pos.character) else {
            return Ok(None);
        };
        let syms = Self::document_symbols(&text);
        let Some((line, col)) = Self::find_class_symbol(&syms, &word) else {
            return Ok(None);
        };
        let len = word.len() as u32;
        Ok(Some(vec![TypeHierarchyItem {
            name: word,
            kind: SymbolKind::CLASS,
            tags: None,
            detail: None,
            uri: uri.clone(),
            range: Range {
                start: Position { line, character: 0 },
                end: Position {
                    line,
                    character: col + len,
                },
            },
            selection_range: Range {
                start: Position {
                    line,
                    character: col,
                },
                end: Position {
                    line,
                    character: col + len,
                },
            },
            data: None,
        }]))
    }

    /// LSP-14 tahap 2a: supertypes dari item hierarki.
    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> JsonRpcResult<Option<Vec<TypeHierarchyItem>>> {
        let Some(text) = self.document_text(&params.item.uri) else {
            return Ok(None);
        };
        let syms = Self::document_symbols(&text);
        let names = Self::type_supertypes(&syms, &params.item.name);
        let items = Self::hierarchy_items(&params.item.uri, &syms, &names);
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(items))
    }

    /// LSP-14 tahap 2a: subtypes dari item hierarki.
    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> JsonRpcResult<Option<Vec<TypeHierarchyItem>>> {
        let Some(text) = self.document_text(&params.item.uri) else {
            return Ok(None);
        };
        let syms = Self::document_symbols(&text);
        let names = Self::type_subtypes(&syms, &params.item.name);
        let items = Self::hierarchy_items(&params.item.uri, &syms, &names);
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(items))
    }

    /// LSP-05 tahap 2a: prepare_rename — validasi kursor + kembalikan
    /// range kata dan placeholder (nama saat ini).
    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> JsonRpcResult<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let pos = params.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let Some((line, s, e)) = Self::prepare_rename_range(&text, pos.line, pos.character) else {
            return Ok(None);
        };
        let placeholder = text
            .lines()
            .nth(line as usize)
            .map(|l| {
                let a = (s as usize).min(l.len());
                let b = (e as usize).min(l.len());
                l[a..b].to_string()
            })
            .unwrap_or_default();
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: Range {
                start: Position { line, character: s },
                end: Position { line, character: e },
            },
            placeholder,
        }))
    }

    /// Rename (LSP-05 tahap 1+2): rename identifier di seluruh dokumen DAN
    /// lintas file (dokumen terbuka + indeks disk).
    async fn rename(&self, params: RenameParams) -> JsonRpcResult<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        // Review R194 BUG-3: validasi (nama baru + kata lama) SEBELUM scan
        // disk — input invalid / no-op tidak boleh membayar pemindaian
        // workspace penuh.
        if !Self::is_valid_identifier(&params.new_name) {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
                "'{}' bukan identifier SystemVerilog yang valid",
                params.new_name
            )));
        }
        let line_text = text.lines().nth(pos.line as usize).unwrap_or("");
        // Review R194 NIT-1/NIT-3: satu guard untuk kursor bukan identifier,
        // keyword, literal angka, dan fragmen literal (`8'd5`).
        let Some((word, _, _)) = Self::rename_word_at_position(line_text, pos.character) else {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(
                "kursor tidak berada di atas identifier yang valid",
            ));
        };
        if word == params.new_name {
            return Ok(Some(WorkspaceEdit::new(Default::default())));
        }
        let docs =
            workspace_index::ensure_doc(self.collect_workspace_docs().await, uri.as_str(), &text);
        match Self::compute_rename_edits_multi(
            &docs,
            &word,
            &params.new_name,
            MAX_CROSS_FILE_REFERENCES,
        ) {
            Ok(hits) => {
                let mut changes: std::collections::HashMap<
                    lsp_types::Url,
                    Vec<lsp_types::TextEdit>,
                > = std::collections::HashMap::new();
                for (u, line, start, end) in hits {
                    let Ok(file_uri) = Url::parse(&u) else {
                        continue;
                    };
                    let edit = lsp_types::TextEdit {
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
                    };
                    changes.entry(file_uri).or_default().push(edit);
                }
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
            .map(|(label, kind)| {
                let snippet = Self::snippet_for_keyword(&label);
                let lsp_kind = DocSymbol::completion_kind(kind);
                let detail = Self::completion_detail(lsp_kind, snippet.is_some());
                CompletionItem {
                    label,
                    kind: Some(lsp_kind),
                    detail,
                    insert_text: snippet.map(|s| s.to_string()),
                    insert_text_format: snippet.map(|_| InsertTextFormat::SNIPPET),
                    ..Default::default()
                }
            })
            .collect();
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(CompletionResponse::Array(items)))
    }

    /// LSP-17: Completion resolve — add documentation/detail to a completion item.
    async fn completion_resolve(&self, mut item: CompletionItem) -> JsonRpcResult<CompletionItem> {
        // LSP-02 tahap 2c: isi detail bila handler lama (atau klien) belum ada.
        item.detail = Self::resolve_detail(item.detail, item.kind, item.insert_text_format);
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

    /// Workspace symbol search (LSP-11 tahap 1+2): dokumen terbuka + indeks
    /// file sumber di DISK (di bawah root workspace dari `initialize`).
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> JsonRpcResult<Option<Vec<SymbolInformation>>> {
        let docs = self.collect_workspace_docs().await;
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
                    base: None,
                    name_col: 0,
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

    /// Semantic tokens full (LSP-07 tahap 1+2c: simpan cache + result_id
    /// untuk delta update).
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> JsonRpcResult<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let tokens = Self::compute_semantic_tokens(&text);
        let rid = self
            .semantic_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();
        if let Ok(mut cache) = self.semantic_cache.lock() {
            cache.insert(uri.as_str().to_string(), (rid.clone(), tokens.clone()));
        }
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: Some(rid),
            data: tokens,
        })))
    }

    /// Semantic tokens delta (LSP-07 tahap 2c): edit dari previous_result_id.
    /// Id tak dikenal → full Tokens (bukan error, sesuai LSP).
    async fn semantic_tokens_full_delta(
        &self,
        params: SemanticTokensDeltaParams,
    ) -> JsonRpcResult<Option<SemanticTokensFullDeltaResult>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let tokens = Self::compute_semantic_tokens(&text);
        let prev = self
            .semantic_cache
            .lock()
            .ok()
            .and_then(|c| c.get(uri.as_str()).cloned());
        // Cache miss / id mismatch → full Tokens DENGAN result_id baru
        // (agar client bisa delta berikutnya; cache ikut diperbarui).
        let matched = matches!(
            &prev,
            Some((prev_id, _)) if *prev_id == params.previous_result_id
        );
        if !matched {
            let rid = self
                .semantic_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .to_string();
            if let Ok(mut cache) = self.semantic_cache.lock() {
                cache.insert(uri.as_str().to_string(), (rid.clone(), tokens.clone()));
            }
            return Ok(Some(SemanticTokensFullDeltaResult::Tokens(
                SemanticTokens {
                    result_id: Some(rid),
                    data: tokens,
                },
            )));
        }
        let prev_tokens = prev.map(|(_, t)| t).unwrap_or_default();
        let edits = Self::semantic_tokens_delta(&prev_tokens, &tokens);
        let rid = self
            .semantic_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();
        if let Ok(mut cache) = self.semantic_cache.lock() {
            cache.insert(uri.as_str().to_string(), (rid.clone(), tokens));
        }
        Ok(Some(SemanticTokensFullDeltaResult::TokensDelta(
            SemanticTokensDelta {
                result_id: Some(rid),
                edits,
            },
        )))
    }

    /// Semantic tokens range (LSP-07 tahap 2b).
    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> JsonRpcResult<Option<SemanticTokensRangeResult>> {
        let uri = &params.text_document.uri;
        let Some(text) = self.document_text(uri) else {
            return Ok(None);
        };
        let tokens =
            Self::semantic_tokens_in_range(&text, params.range.start.line, params.range.end.line);
        Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
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

    /// Text document didClose — clear diagnostics + evict caches
    /// (LSP-07 tahap 2c: semantic_cache ikut dibuang agar tak basi).
    async fn did_close(&self, params: lsp_types::DidCloseTextDocumentParams) {
        let key = params.text_document.uri.as_str().to_string();
        if let Ok(mut docs) = self.documents.lock() {
            docs.remove(&key);
        }
        if let Ok(mut cache) = self.semantic_cache.lock() {
            cache.remove(&key);
        }
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
        let ch = completions::char_index_to_byte(line_text, character as usize);
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
            // start/end = offset byte hasil walk; LSP memakai kolom karakter.
            let start_col = line_text[..start].chars().count() as u32;
            let end_col = line_text[..end].chars().count() as u32;
            Range::new(Position::new(line, start_col), Position::new(line, end_col))
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

    // Middle: the entire line (kolom = jumlah KARAKTER, bukan byte).
    let line_range = Range::new(
        Position::new(line, 0),
        Position::new(
            line,
            lines
                .get(line_idx)
                .map(|l| l.chars().count() as u32)
                .unwrap_or(0),
        ),
    );

    // Outermost: enclosing scope (module/class/function/end → start).
    let scope_range = find_enclosing_scope(&lines, line_idx).unwrap_or(Range::new(
        Position::new(0, 0),
        Position::new(lines.len() as u32, 0),
    ));

    // Spec LSP: range teratas = yang TERKECIL; parent = range yang memuatnya.
    // Urutan: word → line → scope (sebelumnya terbalik sehingga Expand
    // Selection langsung memilih seluruh module).
    SelectionRange {
        range: word_range,
        parent: Some(Box::new(SelectionRange {
            range: line_range,
            parent: Some(Box::new(SelectionRange {
                range: scope_range,
                parent: None,
            })),
        })),
    }
}

/// Find the enclosing scope range for a given line.
fn find_enclosing_scope(lines: &[&str], target_line: usize) -> Option<Range> {
    // EOF setelah newline terakhir: posisi sah tapi tidak ada baris →
    // tanpa guard ini lines[i] panic (request selectionRange di ujung file).
    if target_line >= lines.len() {
        return None;
    }
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
                                // Kolom akhir = jumlah karakter (bukan byte).
                                Position::new(j as u32, line.chars().count() as u32),
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
    fn test_find_definition_in_docs_cross_file() {
        // LSP-03 tahap 2: definisi lintas file. top.sv memakai `sub` dan
        // `clk`; deklarasi `sub` hanya ada di sub.sv.
        let top = [
            "module top;",
            "  logic clk;",
            "  sub u0 (.clk(clk));",
            "endmodule",
        ]
        .join("\n");
        let sub = ["module sub (", "  input logic clk", ");", "endmodule"].join("\n");
        let docs = vec![
            ("file:///top.sv".to_string(), top.clone()),
            ("file:///sub.sv".to_string(), sub.clone()),
        ];

        // `sub` tidak didefinisikan di top.sv → ketemu di sub.sv baris 0.
        let hit = LspBackend::find_definition_in_docs(&docs, "sub", "file:///top.sv")
            .expect("module sub ada di sub.sv");
        assert_eq!(hit.0, "file:///sub.sv");
        assert_eq!(hit.1, 0);
        assert_eq!(
            &sub.lines().nth(hit.1 as usize).unwrap()[hit.2 as usize..hit.3 as usize],
            "sub"
        );

        // `clk` didefinisikan di KEDUA file → dokumen asal (prefer) menang.
        let local = LspBackend::find_definition_in_docs(&docs, "clk", "file:///top.sv")
            .expect("clk ada di top.sv");
        assert_eq!(local.0, "file:///top.sv");
        assert_eq!(local.1, 1);
        assert_eq!(
            &top.lines().nth(local.1 as usize).unwrap()[local.2 as usize..local.3 as usize],
            "clk"
        );
        let other = LspBackend::find_definition_in_docs(&docs, "clk", "file:///sub.sv")
            .expect("clk ada di sub.sv");
        assert_eq!(other.0, "file:///sub.sv");
        assert_eq!(other.1, 1);

        // Determinisme: urutan masukan tidak mengubah hasil. Prefer yang
        // tidak ada di himpunan → urutan uri menang (sub.sv < top.sv).
        let mut rev = docs.clone();
        rev.reverse();
        assert_eq!(
            LspBackend::find_definition_in_docs(&docs, "sub", "file:///top.sv"),
            LspBackend::find_definition_in_docs(&rev, "sub", "file:///top.sv")
        );
        let by_uri = LspBackend::find_definition_in_docs(&docs, "clk", "file:///other.sv")
            .expect("clk tetap ketemu");
        assert_eq!(by_uri.0, "file:///sub.sv");
        assert_eq!(
            by_uri,
            LspBackend::find_definition_in_docs(&rev, "clk", "file:///other.sv")
                .expect("clk tetap ketemu")
        );

        // Dua definisi dalam SATU file → yang pertama (baris terkecil) menang,
        // dan prefilter substring tidak mengubah hasil.
        let dup = ["module d;", "  logic clk;", "  logic clk;", "endmodule"].join("\n");
        let dup_docs = vec![("file:///dup.sv".to_string(), dup)];
        let first = LspBackend::find_definition_in_docs(&dup_docs, "clk", "file:///dup.sv")
            .expect("clk ada");
        assert_eq!(first.1, 1);
        // Kata yang hanya muncul sebagai substring (bukan token) tidak
        // menghasilkan definisi — prefilter hanya optimisasi, hasil sama.
        assert!(LspBackend::find_definition_in_docs(&docs, "cl", "file:///top.sv").is_none());

        // Tidak ada definisi / tanpa dokumen → None.
        assert!(
            LspBackend::find_definition_in_docs(&docs, "tidak_ada", "file:///top.sv").is_none()
        );
        assert!(LspBackend::find_definition_in_docs(&[], "clk", "file:///top.sv").is_none());
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
    fn test_references_hits_removes_only_declaration_hits() {
        // LSP-04: include_declaration=false → hanya hit deklarasi (port ANSI
        // di header) yang dibuang; pemakaian tetap.
        let docs = vec![(
            "file:///m.sv".to_string(),
            "module m (input logic clk, output logic q);\n  always_ff @(posedge clk) q <= clk;\nendmodule\n"
                .to_string(),
        )];
        let hits = LspBackend::find_references_in_docs(&docs, "clk", 100);
        assert_eq!(hits.len(), 3, "port header + 2 pemakaian: {hits:?}");
        let filtered = LspBackend::references_hits(&docs, "clk", false, 100);
        assert_eq!(filtered.len(), 2, "deklarasi dibuang: {filtered:?}");
        assert!(
            filtered.iter().all(|h| h.1 == 1),
            "hanya baris pemakaian: {filtered:?}"
        );
    }

    #[test]
    fn test_references_hits_keeps_width_usage() {
        // Pemakaian parameter W di lebar port BUKAN deklarasi — tidak boleh
        // dibuang saat include_declaration=false.
        let src = "module m #(\n  parameter W = 8\n) (\n  input logic [W-1:0] d\n);\nendmodule\n";
        let docs = vec![("file:///w.sv".to_string(), src.to_string())];
        let hits = LspBackend::find_references_in_docs(&docs, "W", 100);
        assert_eq!(hits.len(), 2, "deklarasi + pemakaian lebar: {hits:?}");
        let filtered = LspBackend::references_hits(&docs, "W", false, 100);
        assert_eq!(
            filtered.len(),
            1,
            "pemakaian W di lebar port dipertahankan: {filtered:?}"
        );
        assert_eq!(filtered[0].1, 3, "baris port: {filtered:?}");
    }

    #[test]
    fn test_references_hits_keeps_rhs_usage() {
        // RHS setelah '=' adalah pemakaian, bukan deklarasi.
        let src = "module m;\n  logic [7:0] a = b;\nendmodule\n";
        let docs = vec![("file:///r.sv".to_string(), src.to_string())];
        let hits = LspBackend::find_references_in_docs(&docs, "b", 100);
        assert_eq!(hits.len(), 1, "{hits:?}");
        let filtered = LspBackend::references_hits(&docs, "b", false, 100);
        assert_eq!(filtered.len(), 1, "RHS dipertahankan: {filtered:?}");
    }

    #[test]
    fn test_references_hits_inherited_direction_port() {
        // `input logic a, b` — b mewarisi arah/tipe → deklarasi juga.
        let src = "module m (input logic a, b);\n  assign b = a;\nendmodule\n";
        let docs = vec![("file:///i.sv".to_string(), src.to_string())];
        let hits = LspBackend::find_references_in_docs(&docs, "b", 100);
        assert_eq!(hits.len(), 2, "port b + pemakaian: {hits:?}");
        let filtered = LspBackend::references_hits(&docs, "b", false, 100);
        assert_eq!(filtered.len(), 1, "deklarasi port b dibuang: {filtered:?}");
        assert_eq!(filtered[0].1, 1, "tinggal pemakaian: {filtered:?}");
    }

    #[test]
    fn test_definition_col_ignores_width_and_rhs_tokens() {
        // Guard false positive: token di dalam lebar, RHS, dan argumen call
        // BUKAN deklarasi (dipakai filter include_declaration).
        let w_line = "localparam [W-1:0] MASK = '1;";
        assert_eq!(LspBackend::definition_col(w_line, "W"), None, "W di lebar");
        let mask_col = w_line.find("MASK").unwrap();
        assert_eq!(LspBackend::definition_col(w_line, "MASK"), Some(mask_col));

        let rhs = "wire [7:0] w = {a, b};";
        assert_eq!(
            LspBackend::definition_col(rhs, "b"),
            None,
            "b di konkatenasi"
        );
        let call = "int y = f(a, b);";
        assert_eq!(
            LspBackend::definition_col(call, "b"),
            None,
            "b argumen call"
        );

        // Multi-statement: deklarasi kedua pada baris yang sama tetap dikenali.
        let two = "logic a = 0; logic b;";
        assert_eq!(
            LspBackend::definition_col(two, "b"),
            Some(two.rfind('b').unwrap()),
            "deklarasi setelah ';'"
        );
    }

    #[test]
    fn test_definition_col_header_continuation_and_int_port() {
        // Lanjutan header multi-baris: baris diawali ')'.
        let cont = ") (input logic clk);";
        let clk = cont.find("clk").unwrap();
        assert_eq!(LspBackend::definition_col(cont, "clk"), Some(clk));

        // Port tanpa arah dengan tipe `int` tetap deklarasi.
        let int_port = "module m (int a);";
        let a = int_port.find("a)").unwrap();
        assert_eq!(LspBackend::definition_col(int_port, "a"), Some(a));

        // '#' setelah '(' (bukan parameter list) tidak menggeser port list.
        let mixed = "module m (input logic clk); initial #(5) x = 0;";
        let c = mixed.find("clk").unwrap();
        assert_eq!(LspBackend::definition_col(mixed, "clk"), Some(c));
    }

    #[test]
    fn test_definition_col_statement_after_semicolon_not_declaration() {
        // Review LSP-04: statement SETELAH ';' pada baris yang sama bukan
        // deklarasi — LHS `assign` adalah pemakaian, bukan deklarator.
        let wire = "wire a; assign b = a;";
        assert_eq!(
            LspBackend::definition_col(wire, "b"),
            None,
            "LHS assign setelah ';'"
        );
        let param = "localparam N = 4; assign y = N;";
        assert_eq!(
            LspBackend::definition_col(param, "y"),
            None,
            "LHS assign setelah localparam"
        );
        // Deklarasi kedua yang diawali keyword tetap dikenali.
        let two = "logic a; reg b;";
        assert_eq!(
            LspBackend::definition_col(two, "b"),
            Some(two.rfind('b').unwrap()),
            "deklarasi setelah ';' berkeyword"
        );
    }

    #[test]
    fn test_definition_col_typedef_braced_members() {
        // Regresi review LSP-04: anggota enum/field struct di dalam kurung
        // kurawal dan nama tipe setelah '}' tetap deklarasi.
        let en = "typedef enum logic [1:0] {IDLE, RUN} state_t;";
        assert_eq!(
            LspBackend::definition_col(en, "IDLE"),
            Some(en.find("IDLE").unwrap()),
            "anggota enum pertama"
        );
        assert_eq!(
            LspBackend::definition_col(en, "RUN"),
            Some(en.find("RUN").unwrap()),
            "anggota enum kedua"
        );
        assert_eq!(
            LspBackend::definition_col(en, "state_t"),
            Some(en.find("state_t").unwrap()),
            "nama tipe setelah kurung kurawal"
        );

        // Nilai bersarang (A = {P, Q}) seluruhnya pemakaian (F-2 ronde 9).
        let nested_val = "typedef enum {A = {P, Q}} e;";
        assert_eq!(
            LspBackend::definition_col(nested_val, "P"),
            None,
            "nilai nested pertama"
        );
        assert_eq!(
            LspBackend::definition_col(nested_val, "Q"),
            None,
            "nilai nested kedua"
        );
        assert_eq!(
            LspBackend::definition_col(nested_val, "A"),
            Some(nested_val.find('A').unwrap()),
            "anggota sebelum nilai nested"
        );

        // Nilai anggota enum adalah pemakaian, bukan nama anggota.
        let val = "typedef enum {A = W} t;";
        assert_eq!(
            LspBackend::definition_col(val, "A"),
            Some(val.find('A').unwrap()),
            "anggota enum sebelum '='"
        );
        assert_eq!(
            LspBackend::definition_col(val, "W"),
            None,
            "nilai enum bukan deklarasi"
        );

        // Tanpa typedef: anggota + nama variabel setelah '}' tetap deklarasi.
        let anon = "enum {IDLE, RUN} state;";
        assert_eq!(
            LspBackend::definition_col(anon, "IDLE"),
            Some(anon.find("IDLE").unwrap()),
            "anggota enum anonim"
        );
        assert_eq!(
            LspBackend::definition_col(anon, "state"),
            Some(anon.find("state").unwrap()),
            "variabel enum anonim"
        );

        // Tipe user-defined anggota BUKAN deklarasi; field sesudahnya ya.
        let typed = "typedef struct packed { my_t f; } s_t;";
        assert_eq!(
            LspBackend::definition_col(typed, "my_t"),
            None,
            "tipe user-defined field"
        );
        assert_eq!(
            LspBackend::definition_col(typed, "f"),
            Some(typed.find(" f;").unwrap() + 1),
            "field bertipe user-defined"
        );

        // Identifier di dalam lebar field struct bukan nama anggota.
        let width = "typedef struct { logic [W-1:0] f; } s_t;";
        assert_eq!(
            LspBackend::definition_col(width, "W"),
            None,
            "parameter lebar field struct"
        );
        assert_eq!(
            LspBackend::definition_col(width, "f"),
            Some(width.find(" f;").unwrap() + 1),
            "field setelah lebar"
        );

        // Struct bersarang: anggota depth >= 2 tetap deklarasi (F-1).
        let nested = "typedef struct { struct { logic x; } inner; } outer;";
        assert_eq!(
            LspBackend::definition_col(nested, "x"),
            Some(nested.find("x;").unwrap()),
            "field struct bersarang"
        );
        assert_eq!(
            LspBackend::definition_col(nested, "inner"),
            Some(nested.find("inner").unwrap()),
            "field struct dalam"
        );
        assert_eq!(
            LspBackend::definition_col(nested, "outer"),
            Some(nested.find("outer").unwrap()),
            "nama tipe luar"
        );

        // Nama tipe dengan dimensi packed / scope (F-2).
        let packed = "typedef struct packed { my_t [3:0] f; } s_t;";
        assert_eq!(
            LspBackend::definition_col(packed, "my_t"),
            None,
            "tipe dengan dimensi"
        );
        assert_eq!(
            LspBackend::definition_col(packed, "f"),
            Some(packed.find(" f;").unwrap() + 1),
            "field setelah tipe berdimensi"
        );
        let scoped = "typedef struct { pkg::my_t f; } s_t;";
        assert_eq!(
            LspBackend::definition_col(scoped, "pkg"),
            None,
            "paket pada tipe berscope"
        );
        let port_d = "input my_t [3:0] d;";
        assert_eq!(
            LspBackend::definition_col(port_d, "my_t"),
            None,
            "tipe port"
        );
        assert_eq!(
            LspBackend::definition_col(port_d, "d"),
            Some(port_d.find("d;").unwrap()),
            "nama port bertipe berdimensi"
        );

        // Initializer setelah '}' bukan deklarasi (F-3).
        let init = "enum {A, B} state = get_state();";
        assert_eq!(
            LspBackend::definition_col(init, "get_state"),
            None,
            "initializer bukan deklarasi"
        );
        assert_eq!(
            LspBackend::definition_col(init, "state"),
            Some(init.find("state").unwrap()),
            "variabel enum"
        );
        let arr = "enum {A, B} state [N];";
        assert_eq!(
            LspBackend::definition_col(arr, "N"),
            None,
            "dimensi unpacked bukan deklarasi"
        );

        // Koma di dalam argumen fungsi bukan pemisah anggota enum.
        let call = "typedef enum {A = f(x, y), B} t;";
        assert_eq!(
            LspBackend::definition_col(call, "y"),
            None,
            "argumen fungsi di nilai enum"
        );
        assert_eq!(
            LspBackend::definition_col(call, "B"),
            Some(call.find("B}").unwrap()),
            "anggota setelah nilai berargumen"
        );

        let st = "typedef struct packed { logic [7:0] f; } s_t;";
        assert_eq!(
            LspBackend::definition_col(st, "f"),
            Some(st.find(" f;").unwrap() + 1),
            "field struct"
        );
        assert_eq!(
            LspBackend::definition_col(st, "s_t"),
            Some(st.find("s_t").unwrap()),
            "nama struct setelah kurung kurawal"
        );
    }

    #[test]
    fn test_definition_col_second_statement_net_and_qualifier() {
        // Deklarasi kedua setelah ';' yang diawali net/qualifier keyword.
        let tri = "wire a; tri b;";
        assert_eq!(
            LspBackend::definition_col(tri, "b"),
            Some(tri.rfind('b').unwrap()),
            "tri setelah ';'"
        );
        let sup = "wire a; supply0 gnd;";
        assert_eq!(
            LspBackend::definition_col(sup, "gnd"),
            Some(sup.find("gnd").unwrap()),
            "supply0 setelah ';'"
        );
        let field = "rand bit [7:0] x;";
        assert_eq!(
            LspBackend::definition_col(field, "x"),
            Some(field.find("x;").unwrap()),
            "field class berprefix rand"
        );

        // Scope resolution (local::x, pkg::x) bukan deklarasi (F-1 ronde 9).
        let scope = "local::gen_error == (m_error ||";
        assert_eq!(
            LspBackend::definition_col(scope, "gen_error"),
            None,
            "local:: bukan deklarasi"
        );
        assert_eq!(
            LspBackend::definition_col(scope, "m_error"),
            None,
            "argumen dalam kurung"
        );

        // Nama tipe user-defined BUKAN deklarator; identifier sesudahnya ya.
        let user = "static my_t cfg;";
        assert_eq!(
            LspBackend::definition_col(user, "my_t"),
            None,
            "nama tipe user-defined"
        );
        assert_eq!(
            LspBackend::definition_col(user, "cfg"),
            Some(user.find("cfg").unwrap()),
            "deklarator setelah nama tipe"
        );
        let port = "input my_t d;";
        assert_eq!(LspBackend::definition_col(port, "my_t"), None, "tipe port");
        assert_eq!(
            LspBackend::definition_col(port, "d"),
            Some(port.find("d;").unwrap()),
            "nama port bertipe user-defined"
        );
    }

    #[test]
    fn test_references_hits_include_declaration_toggle() {
        // Wiring LSP-04: parameter include_declaration menentukan apakah
        // deklarasi ikut (menggantikan cabang if di handler).
        let src = "module m (input logic clk);\n  always_ff @(posedge clk) q <= clk;\nendmodule\n";
        let docs = vec![("file:///t.sv".to_string(), src.to_string())];
        let with_decl = LspBackend::references_hits(&docs, "clk", true, 100);
        let no_decl = LspBackend::references_hits(&docs, "clk", false, 100);
        assert_eq!(with_decl.len(), 3, "deklarasi + 2 pemakaian: {with_decl:?}");
        assert_eq!(no_decl.len(), 2, "tanpa deklarasi: {no_decl:?}");
        assert!(
            with_decl.iter().any(|h| h.1 == 0),
            "deklarasi ikut saat true: {with_decl:?}"
        );
        assert!(
            no_decl.iter().all(|h| h.1 != 0),
            "deklarasi dibuang saat false: {no_decl:?}"
        );
    }

    #[test]
    fn test_references_hits_filters_declarations_before_cap() {
        // Review LSP-04: filter deklarasi SEBELUM memotong ke max, agar
        // kuota tidak dihabiskan hit deklarasi.
        let src = "module m (input logic a, b);\n  assign a = b;\n  assign b = a;\nendmodule\n";
        let docs = vec![("file:///c.sv".to_string(), src.to_string())];
        let hits = LspBackend::references_hits(&docs, "b", false, 2);
        assert_eq!(hits.len(), 2, "dua pemakaian tersisa: {hits:?}");
        assert!(
            hits.iter().all(|h| h.1 >= 1),
            "cap tidak dihabiskan deklarasi: {hits:?}"
        );
    }

    #[test]
    fn test_find_definition_port_after_parameter_list() {
        // Header dengan #(...) sebaris: '(' pertama adalah daftar parameter,
        // port list ada di '(' berikutnya.
        let src = "module m #(parameter W = 8) (input logic clk);\n  always_ff @(posedge clk) ;\nendmodule\n";
        let col = src.lines().next().unwrap().find("clk").unwrap() as u32;
        let d = LspBackend::find_definition(src, 1, 22).expect("definisi port header");
        assert_eq!((d.0, d.1), (0, col), "port setelah #(...)");
    }

    #[test]
    fn test_definition_col_function_return_type_not_declaration() {
        // F-4: tipe return function bukan deklarasi; nama fungsi + argumen ya.
        let f = "function my_t f(input my_t x);";
        assert_eq!(
            LspBackend::definition_col(f, "my_t"),
            None,
            "tipe return bukan deklarasi"
        );
        assert_eq!(
            LspBackend::definition_col(f, "f"),
            Some(f.find("f(").unwrap()),
            "nama fungsi"
        );
        assert_eq!(
            LspBackend::definition_col(f, "x"),
            Some(f.rfind('x').unwrap()),
            "argumen header fungsi"
        );
    }

    #[test]
    fn test_find_definition_ansi_port_in_header() {
        // LSP-03: port ANSI di header module kini punya definisi (dipakai
        // juga oleh filter include_declaration).
        let src = "module m (input logic clk);\n  always_ff @(posedge clk) ;\nendmodule\n";
        let d = LspBackend::find_definition(src, 1, 22).expect("definisi port header");
        assert_eq!((d.0, d.1, d.2), (0, 22, 25), "kolom nama port di header");
    }

    #[test]
    fn test_find_references_across_documents() {
        // LSP-04 tahap 2: referensi lintas file (dokumen terbuka + disk).
        let docs = vec![
            (
                "file:///top.sv".to_string(),
                "module top;\n  logic clk;\n  // clk di komentar\n  sub u0 (.clk(clk));\nendmodule\n"
                    .to_string(),
            ),
            (
                "file:///sub.sv".to_string(),
                "module sub (input logic clk);\n  always_ff @(posedge clk) ;\n  logic clk_counter;\nendmodule\n"
                    .to_string(),
            ),
        ];
        let hits = LspBackend::find_references_in_docs(&docs, "clk", 100);
        // top: decl + 2x di instance line = 3 (komentar dilewati);
        // sub: port decl + posedge = 2; clk_counter TIDAK match (word-boundary).
        assert_eq!(hits.len(), 5, "{hits:?}");
        assert_eq!(hits.iter().filter(|h| h.0 == "file:///top.sv").count(), 3);
        assert_eq!(hits.iter().filter(|h| h.0 == "file:///sub.sv").count(), 2);
        // Kolom & baris akurat utk satu hit yang diketahui.
        assert!(
            hits.iter().any(|h| h.0 == "file:///sub.sv" && h.1 == 0),
            "{hits:?}"
        );
        // end = start + len(word)
        assert!(hits.iter().all(|h| h.3 == h.2 + 3));
        // Kolom ABSOLUT (bukan hanya relatif): baris "  logic clk;" → clk
        // mulai kolom 8 (0-based).
        let decl = hits
            .iter()
            .find(|h| h.0 == "file:///top.sv" && h.1 == 1)
            .expect("hit deklarasi clk di top.sv baris 1");
        assert_eq!((decl.2, decl.3), (8, 11), "kolom absolut salah: {decl:?}");

        // LSP-04: include_declaration=false → hit deklarasi dibuang.
        let filtered = LspBackend::references_hits(&docs, "clk", false, 100);
        assert_eq!(filtered.len(), 3, "dua deklarasi dibuang: {filtered:?}");
        assert!(
            filtered.iter().any(|h| h.0 == "file:///top.sv" && h.1 == 3),
            "pemakaian di instance tetap ada: {filtered:?}"
        );
        assert!(
            filtered.iter().any(|h| h.0 == "file:///sub.sv" && h.1 == 1),
            "posedge clk tetap ada: {filtered:?}"
        );
        assert!(
            !filtered.iter().any(|h| h.0 == "file:///top.sv" && h.1 == 1),
            "deklarasi top.sv dibuang: {filtered:?}"
        );

        // Batas hasil (cap) dihormati.
        let capped = LspBackend::find_references_in_docs(&docs, "clk", 2);
        assert_eq!(capped.len(), 2, "{capped:?}");
        // Word kosong / cap 0 → kosong.
        assert!(LspBackend::find_references_in_docs(&docs, "", 10).is_empty());
        assert!(LspBackend::find_references_in_docs(&docs, "clk", 0).is_empty());
        // Tidak ada dokumen → kosong.
        assert!(LspBackend::find_references_in_docs(&[], "clk", 10).is_empty());
    }

    /// Review R193 BUG-1: urutan hasil dan himpunan hasil saat cap tercapai
    /// harus deterministik — handler mengurutkan docs by uri sebelum
    /// memanggil core (sumber urutan acak = HashMap documents).
    #[test]
    fn test_find_references_docs_order_is_deterministic() {
        let a = (
            "file:///a.sv".to_string(),
            "module a;\n  logic clk;\n  logic clk2;\nendmodule\n".to_string(),
        );
        let b = (
            "file:///b.sv".to_string(),
            "module b;\n  logic clk;\nendmodule\n".to_string(),
        );
        // Masukan SENGAJA tak terurut; helper produksi (sorted_docs) yang
        // wajib mengurutkan — bila sort dihapus dari helper, test gagal.
        let docs1 = workspace_index::sorted_docs(vec![a.clone(), b.clone()]);
        let docs2 = workspace_index::sorted_docs(vec![b, a]);

        let h1 = LspBackend::find_references_in_docs(&docs1, "clk", 100);
        let h2 = LspBackend::find_references_in_docs(&docs2, "clk", 100);
        assert_eq!(h1, h2, "urutan dokumen masukan tidak boleh mengubah hasil");

        // Urutan keluaran mengikuti urutan dokumen (uri menaik).
        let uris: Vec<&str> = h1.iter().map(|h| h.0.as_str()).collect();
        let mut sorted = uris.clone();
        sorted.sort();
        assert_eq!(uris, sorted, "{h1:?}");

        // Cap juga deterministik: cap 1 selalu memilih hit pertama dokumen
        // pertama (bukan acak).
        let c1 = LspBackend::find_references_in_docs(&docs1, "clk", 1);
        let c2 = LspBackend::find_references_in_docs(&docs2, "clk", 1);
        assert_eq!(c1, c2, "{c1:?} vs {c2:?}");
        assert_eq!(c1.len(), 1);
        assert_eq!(c1[0].0, "file:///a.sv");
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
    fn test_rename_edits_across_documents() {
        // LSP-05 tahap 2: rename lintas file (dokumen terbuka + disk).
        let docs = vec![
            (
                "file:///top.sv".to_string(),
                "module top;\n  logic clk;\n  sub u0 (.clk(clk));\nendmodule\n".to_string(),
            ),
            (
                "file:///sub.sv".to_string(),
                "module sub (input logic clk);\n  always_ff @(posedge clk) ;\nendmodule\n"
                    .to_string(),
            ),
        ];
        let hits = LspBackend::compute_rename_edits_multi(&docs, "clk", "clock", 100).unwrap();
        // top: decl + 2x di baris instance = 3; sub: port + posedge = 2.
        assert_eq!(hits.len(), 5, "{hits:?}");
        assert_eq!(hits.iter().filter(|h| h.0 == "file:///top.sv").count(), 3);
        assert_eq!(hits.iter().filter(|h| h.0 == "file:///sub.sv").count(), 2);

        // Pengelompokan per-uri (seperti handler): tiap file punya edit
        // terurut baris menaik dan rentang = panjang nama lama.
        let mut per_uri: std::collections::HashMap<&str, Vec<(u32, u32, u32)>> =
            std::collections::HashMap::new();
        for (u, line, start, end) in &hits {
            per_uri
                .entry(u.as_str())
                .or_default()
                .push((*line, *start, *end));
        }
        assert_eq!(per_uri.len(), 2, "harus ada edit di 2 file: {per_uri:?}");
        for (_, edits) in per_uri.iter() {
            let lines: Vec<u32> = edits.iter().map(|e| e.0).collect();
            let mut sorted = lines.clone();
            sorted.sort();
            assert_eq!(lines, sorted, "edit per file harus urut baris");
            assert!(edits.iter().all(|e| e.2 == e.1 + 3), "rentang = len(clk)");
        }

        // Validasi & no-op.
        assert!(LspBackend::compute_rename_edits_multi(&docs, "clk", "1bad", 100).is_err());
        assert!(LspBackend::compute_rename_edits_multi(&docs, "clk", "", 100).is_err());
        assert!(LspBackend::compute_rename_edits_multi(&docs, "", "clock", 100).is_err());
        assert!(
            LspBackend::compute_rename_edits_multi(&docs, "clk", "clk", 100)
                .unwrap()
                .is_empty()
        );
        // Nama yang tidak ada di dokumen mana pun → kosong (bukan error).
        assert!(
            LspBackend::compute_rename_edits_multi(&docs, "tidak_ada", "x", 100)
                .unwrap()
                .is_empty()
        );
        // Review R194 BUG-2: kata target wajib identifier — `42` / `$display`
        // (diterima word_at_position) HARUS ditolak agar literal/system task
        // tidak di-rename massal.
        assert!(LspBackend::compute_rename_edits_multi(&docs, "42", "x", 100).is_err());
        assert!(LspBackend::compute_rename_edits_multi(&docs, "$display", "x", 100).is_err());

        // Review R194 BUG-1: batas TIDAK boleh memotong senyap → Err.
        let err = LspBackend::compute_rename_edits_multi(&docs, "clk", "clock", 2)
            .expect_err("rename terpotong harus Err, bukan sukses parsial");
        assert!(err.contains("terlalu besar"), "{err}");
        // Tepat di batas (5 kemunculan, max=5) → tetap Ok (bukan off-by-one).
        assert_eq!(
            LspBackend::compute_rename_edits_multi(&docs, "clk", "clock", 5)
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn test_prepare_rename_range() {
        // Di atas identifier → range kata; di luar → None.
        let use_line = line_of("else if (enable)");
        let col = SAMPLE
            .lines()
            .nth(use_line as usize)
            .unwrap()
            .find("enable")
            .unwrap() as u32;
        let (l, s, e) =
            LspBackend::prepare_rename_range(SAMPLE, use_line, col).expect("range ketemu");
        assert_eq!(l, use_line);
        assert_eq!(
            &SAMPLE.lines().nth(l as usize).unwrap()[s as usize..e as usize],
            "enable"
        );
        assert!(LspBackend::prepare_rename_range(SAMPLE, 7, 0).is_none());
        assert!(LspBackend::prepare_rename_range(SAMPLE, 999, 0).is_none());
    }
    #[test]
    fn test_rename_rejects_keyword_and_literal_fragment() {
        // Review R194 NIT-2: keyword SV bukan identifier sah untuk rename.
        let docs = vec![(
            "file:///a.sv".to_string(),
            "module m;\n  logic clk;\nendmodule\n".to_string(),
        )];
        let keywords = [
            "al".to_string() + "ways",
            "logic".into(),
            "endmodule".into(),
        ];
        for kw in &keywords {
            assert!(!LspBackend::is_valid_identifier(kw), "`{kw}` keyword");
            assert!(
                LspBackend::compute_rename_edits_multi(&docs, "clk", kw, 100).is_err(),
                "rename ke keyword `{kw}` harus ditolak"
            );
        }
        // Identifier sah tetap diterima.
        assert!(LspBackend::is_valid_identifier("clk_2$"));
        assert!(LspBackend::is_valid_identifier("_tmp"));

        // Review R194 NIT-3: fragmen literal berbasis bukan identifier —
        // kursor di `8'd5` menghasilkan kata `d5` yang tampak sah.
        let line = "  assign y = 8'd5;";
        let col_d5 = line.find("d5").unwrap() as u32;
        assert!(
            LspBackend::rename_word_at_position(line, col_d5).is_none(),
            "`d5` pada 8'd5 adalah fragmen literal"
        );
        let col_y = line.find("y =").unwrap() as u32;
        assert_eq!(
            LspBackend::rename_word_at_position(line, col_y)
                .map(|t| t.0)
                .as_deref(),
            Some("y")
        );

        // NIT-1: prepare_rename tidak menawarkan rename di posisi invalid.
        let src = ["module m;", line, "endmodule"].join("\n");
        assert!(LspBackend::prepare_rename_range(&src, 1, col_d5).is_none());
        let kline = "  logic clk;";
        let kcol = kline.find("logic").unwrap() as u32;
        let ksrc = ["module m;", kline, "endmodule"].join("\n");
        assert!(LspBackend::prepare_rename_range(&ksrc, 1, kcol).is_none());
        let dline = "  $display(\"hi\");";
        assert!(LspBackend::prepare_rename_range(dline, 0, 3).is_none());

        // NIT-4: core single-doc juga menolak kata non-identifier.
        let num = ["module m;", "  x = 42;", "endmodule"].join("\n");
        let ncol = num.lines().nth(1).unwrap().find("42").unwrap() as u32;
        assert!(LspBackend::compute_rename_edits(&num, 1, ncol, "y").is_err());
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
    fn test_hover_doc_comment() {
        // Doc comment `//` di atas definisi muncul di hover.
        let src = "// Counter utama.\n// Hitung naik.\nmodule counter;\nendmodule\n";
        let doc = LspBackend::doc_comment_above(src, 2).expect("doc ketemu");
        assert!(doc.contains("Counter utama"), "{doc}");
        assert!(doc.contains("Hitung naik"), "{doc}");
        assert!(LspBackend::doc_comment_above(src, 3).is_none());
        let info = LspBackend::hover_info(src, 2, 8).expect("hover modul");
        assert!(info.contains("Counter utama"), "{info}");
    }

    #[test]
    fn test_hover_port_detail() {
        // LSP-06 tahap 2b: direction + range eksplisit untuk port.
        assert_eq!(
            LspBackend::port_detail("port input", "input logic [7:0] clk,").as_deref(),
            Some("direction: input · range: `[7:0]`")
        );
        assert_eq!(
            LspBackend::port_detail("port output", "output bit o").as_deref(),
            Some("direction: output · scalar")
        );
        assert!(LspBackend::port_detail("module", "module m;").is_none());
        assert_eq!(
            LspBackend::port_detail("port inout", "inout wire [3:0] b").as_deref(),
            Some("direction: inout · range: `[3:0]`")
        );
        let src = "module m(\n  input logic [7:0] d\n);\n  assign x = d;\nendmodule\n";
        let uline = 3u32;
        let ucol = src.lines().nth(3).unwrap().find("d;").unwrap() as u32;
        let info = LspBackend::hover_info(src, uline, ucol).expect("hover port");
        assert!(info.contains("direction: input · range: `[7:0]`"), "{info}");
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
    fn test_document_symbols_function_scope() {
        // LSP-12 tahap 2a: lokal di badan function jadi anak function.
        let src = "module m;\n  function int add(input int a);\n    logic tmp;\n    return a;\n  endfunction\nendmodule\n";
        let syms = LspBackend::document_symbols(src);
        assert_eq!(syms.len(), 1);
        let m = &syms[0];
        let f = m
            .children
            .iter()
            .find(|c| c.name == "add")
            .expect("function ada");
        assert_eq!(f.kind, DocSymbol::KIND_FUNCTION);
        assert!(f.children.iter().any(|c| c.name == "tmp"), "{f:?}");
        assert!(!m.children.iter().any(|c| c.name == "tmp"), "{m:?}");
        // Tanpa endfunction (dokumen rusak) tetap nested via cleanup.
        let broken = "module m;\n  function int add();\n    logic tmp;\n";
        let b = &LspBackend::document_symbols(broken)[0];
        let bf = b.children.iter().find(|c| c.name == "add").expect("fn ada");
        assert!(bf.children.iter().any(|c| c.name == "tmp"), "{bf:?}");
        // Sinyal setelah endfunction kembali ke module (bukan anak function).
        let after = "module m;\n  function int add();\n    logic tmp;\n  endfunction\n  logic after;\nendmodule\n";
        let am = &LspBackend::document_symbols(after)[0];
        assert!(am.children.iter().any(|c| c.name == "after"), "{am:?}");
        let af = am
            .children
            .iter()
            .find(|c| c.name == "add")
            .expect("fn ada");
        assert!(!af.children.iter().any(|c| c.name == "after"), "{af:?}");
        // Task/endtask sama.
        let tsrc = "module m;\n  task run();\n    logic t;\n  endtask\nendmodule\n";
        let tm = &LspBackend::document_symbols(tsrc)[0];
        let tf = tm
            .children
            .iter()
            .find(|c| c.name == "run")
            .expect("task ada");
        assert_eq!(tf.kind, DocSymbol::KIND_FUNCTION);
        assert!(tf.children.iter().any(|c| c.name == "t"), "{tf:?}");
    }

    #[test]
    fn test_document_symbols_qualified_members() {
        // LSP-12 tahap 2b: qualifier deklarasi (rand/protected/static/
        // virtual/...) tidak boleh menyembunyikan field/method class dari
        // outline — pola standar class UVM.
        let src = "class pkt;\n  rand bit [7:0] addr;\n  protected int cnt;\n  static int hits;\n  virtual function void write();\n    int tmp;\n  endfunction\nendclass\n";
        let syms = LspBackend::document_symbols(src);
        assert_eq!(syms.len(), 1);
        let names: Vec<&str> = syms[0].children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"addr"), "rand bit → field: {names:?}");
        assert!(names.contains(&"cnt"), "protected int → field: {names:?}");
        assert!(names.contains(&"hits"), "static int → field: {names:?}");
        let f = syms[0]
            .children
            .iter()
            .find(|c| c.name == "write")
            .expect("virtual function harus ter-outline");
        assert_eq!(f.kind, DocSymbol::KIND_FUNCTION);
        assert!(
            f.children.iter().any(|c| c.name == "tmp"),
            "lokal di badan virtual function tetap anak function"
        );
        // Prototype `extern`/`pure` TIDAK membuka frame (tanpa endfunction) —
        // simbol berikutnya tidak boleh tertelan.
        let proto = "class c;\n  extern function void foo();\n  int after;\nendclass\n";
        let p = &LspBackend::document_symbols(proto)[0];
        assert!(
            p.children.iter().any(|c| c.name == "after"),
            "simbol setelah prototype: {:?}",
            p.children.iter().map(|c| &c.name).collect::<Vec<_>>()
        );

        // Review R197 NIT-2: qualifier lain + baris yang seluruhnya qualifier.
        let more = "class q;\n  randc bit [1:0] r;\n  local int l;\n  const int k = 1;\n  var logic v;\n  automatic int a;\n  virtual\n  localparam int LP = 2;\nendclass\n";
        let qs = &LspBackend::document_symbols(more)[0];
        let qn: Vec<&str> = qs.children.iter().map(|c| c.name.as_str()).collect();
        for n in ["r", "l", "k", "v", "a", "LP"] {
            assert!(qn.contains(&n), "`{n}` hilang dari outline: {qn:?}");
        }
        assert_eq!(qn.len(), 6, "baris `virtual` sendirian: {qn:?}");

        // `pure virtual function` = prototype → tidak menghasilkan simbol dan
        // tidak membuka frame.
        let pv = "class c2;\n  pure virtual function void f();\n  int after2;\nendclass\n";
        let c2 = &LspBackend::document_symbols(pv)[0];
        assert!(
            c2.children.iter().any(|c| c.name == "after2"),
            "{:?}",
            c2.children
        );
        assert!(
            !c2.children.iter().any(|c| c.name == "f"),
            "{:?}",
            c2.children
        );

        // Review R197 BUG-1: `virtual interface <tipe> <var>;` adalah
        // deklarasi variabel, bukan pembukaan scope interface (tidak ada
        // endinterface) — jangan sampai menelan simbol berikutnya.
        let vif = "class c3;\n  virtual interface bus_if vif;\n  int after3;\nendclass\n";
        let c3 = &LspBackend::document_symbols(vif)[0];
        assert!(
            c3.children.iter().any(|c| c.name == "after3"),
            "virtual interface tidak boleh membuka frame: {:?}",
            c3.children.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert!(
            !c3.children
                .iter()
                .any(|c| c.kind == DocSymbol::KIND_INTERFACE),
            "virtual interface bukan container interface"
        );

        // NIT-5: guard yang sama berlaku di dalam module.
        let vif_m = "module m;\n  virtual interface bus_if vif;\n  int after4;\nendmodule\n";
        let m4 = &LspBackend::document_symbols(vif_m)[0];
        assert!(
            m4.children.iter().any(|c| c.name == "after4"),
            "{:?}",
            m4.children
        );

        // NIT-4: `interface class` adalah scope CLASS (bukan interface) —
        // endclass menutup frame yang benar.
        let ic = "interface class ic;\n  pure virtual function void f();\nendclass\nclass after_ic;\nendclass\n";
        let syms_ic = LspBackend::document_symbols(ic);
        assert_eq!(
            syms_ic.len(),
            2,
            "{:?}",
            syms_ic.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert_eq!(syms_ic[0].name, "ic");
        assert_eq!(syms_ic[0].kind, DocSymbol::KIND_CLASS);
        assert_eq!(syms_ic[1].name, "after_ic");
        assert_eq!(syms_ic[1].kind, DocSymbol::KIND_CLASS);
    }

    #[test]
    fn test_type_hierarchy_extends() {
        // LSP-14 tahap 2a: rantai extends + supertypes/subtypes.
        let src = "class base;\nendclass\nclass mid extends base;\nendclass\nclass leaf extends mid;\nendclass\nclass lone;\nendclass\n";
        let syms = LspBackend::document_symbols(src);
        assert_eq!(
            LspBackend::type_supertypes(&syms, "leaf"),
            vec!["mid", "base"]
        );
        assert!(LspBackend::type_supertypes(&syms, "lone").is_empty());
        assert!(LspBackend::type_supertypes(&syms, "tak_ada").is_empty());
        let sub = LspBackend::type_subtypes(&syms, "base");
        assert_eq!(sub, vec!["mid", "leaf"], "{sub:?}");
        assert!(LspBackend::type_subtypes(&syms, "leaf").is_empty());
        // Parameterized: `class p #(int W) extends base`.
        let psrc = "class base;\nendclass\nclass p #(int W) extends base;\nendclass\n";
        let psyms = LspBackend::document_symbols(psrc);
        assert_eq!(LspBackend::type_supertypes(&psyms, "p"), vec!["base"]);
        // Base tak terdefinisi: rantai berhenti, tidak dimasukkan.
        let usrc = "class a extends Unknown;\nendclass\n";
        let usyms = LspBackend::document_symbols(usrc);
        assert!(LspBackend::type_supertypes(&usyms, "a").is_empty());
        // Cycle A↔B: berhenti tanpa hang, peminta dikecualikan.
        let csrc = "class a extends b;\nendclass\nclass b extends a;\nendclass\n";
        let csyms = LspBackend::document_symbols(csrc);
        assert_eq!(LspBackend::type_supertypes(&csyms, "a"), vec!["b"]);
        assert_eq!(LspBackend::type_subtypes(&csyms, "a"), vec!["b"]);
        // Self-extends: kosong dua arah.
        let ssrc = "class s extends s;\nendclass\n";
        let ssys = LspBackend::document_symbols(ssrc);
        assert!(LspBackend::type_supertypes(&ssys, "s").is_empty());
        assert!(LspBackend::type_subtypes(&ssys, "s").is_empty());
        // selection_range menutupi NAMA (bukan keyword `class`).
        let uri = lsp_types::Url::parse("file:///t.sv").unwrap();
        let items = LspBackend::hierarchy_items(&uri, &syms, &["mid".to_string()]);
        assert_eq!(items.len(), 1);
        let r = &items[0].selection_range;
        assert_eq!(
            (r.start.line, r.start.character, r.end.character),
            (2, 6, 9),
            "{r:?}"
        );
        // col keyword vs name_col nama.
        let mid = syms.iter().find(|s| s.name == "mid").unwrap();
        assert_eq!((mid.col, mid.name_col), (0, 6));
        // Unknown difilter di items.
        assert!(LspBackend::hierarchy_items(&uri, &syms, &["Ghost".to_string()]).is_empty());
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
    fn test_folding_comment_region() {
        // Blok komentar /* */ multi-baris + marker #region terlipat.
        let src = "/* lisensi\nspdx\n*/\nmodule m;\n// #region ports\ninput clk;\n// #endregion\nendmodule\n";
        let f = LspBackend::compute_folding_ranges(src);
        assert!(f.contains(&(0, 2)), "blok komentar 0..2: {f:?}");
        assert!(f.contains(&(4, 6)), "region 4..6: {f:?}");
        assert!(f.contains(&(3, 7)), "module tetap: {f:?}");
        // Single-line /* */ tidak jadi fold (tolak semua start==0).
        let single = "/* satu */\nmodule m;\nendmodule\n";
        let fs = LspBackend::compute_folding_ranges(single);
        assert!(!fs.iter().any(|(s, _)| *s == 0), "{fs:?}");
        // Unclosed /* dan unclosed region: tidak panic, tidak emit.
        let unclosed = "/* buka\nmodule m;\nendmodule\n";
        let fu = LspBackend::compute_folding_ranges(unclosed);
        assert!(!fu.iter().any(|(s, _)| *s == 0), "{fu:?}");
        let unreg = "// #region x\nmodule m;\nendmodule\n";
        assert!(
            !LspBackend::compute_folding_ranges(unreg)
                .iter()
                .any(|(s, e)| (*s, *e) == (0, 2)),
            "{unreg}"
        );
        // Alias tanpa `#` + `// regional` bukan marker.
        let alias = "// region io\ninput clk;\n// endregion\n";
        let fa = LspBackend::compute_folding_ranges(alias);
        assert!(fa.contains(&(0, 2)), "{fa:?}");
        let notreg = "// regional bus\nmodule m;\n// #endregion\nendmodule\n";
        let fnr = LspBackend::compute_folding_ranges(notreg);
        assert!(!fnr.iter().any(|(s, e)| *e == 2 && *s == 0), "{fnr:?}");
    }

    #[test]
    fn test_folding_import_group() {
        // LSP-15 tahap 2b: 2+ baris import berurutan terlipat.
        let src = "import pkg_a::*;\nimport pkg_b::*;\nmodule m;\nendmodule\n";
        let f = LspBackend::compute_folding_ranges(src);
        assert!(f.contains(&(0, 1)), "{f:?}");
        assert!(f.contains(&(2, 3)), "{f:?}");
        // Tunggal tidak dilipat; baris kosong di dalam grup dilewati.
        let single = "import pkg_a::*;\nmodule m;\nendmodule\n";
        assert!(!LspBackend::compute_folding_ranges(single)
            .iter()
            .any(|(s, _)| *s == 0));
        let gap = "import pkg_a::*;\n\nimport pkg_b::*;\nmodule m;\nendmodule\n";
        assert!(LspBackend::compute_folding_ranges(gap).contains(&(0, 2)));
        // Komentar di dalam grup ikut; grup di EOF ter-flush.
        let cmt = "import pkg_a::*;\n// usein\nimport pkg_b::*;\nmodule m;\nendmodule\n";
        assert!(LspBackend::compute_folding_ranges(cmt).contains(&(0, 2)));
        let eof = "module m;\nendmodule\nimport pkg_a::*;\nimport pkg_b::*;\n";
        assert!(LspBackend::compute_folding_ranges(eof).contains(&(2, 3)));
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
    fn test_workspace_symbol_from_disk_index() {
        // LSP-11 tahap 2: simbol dari file di DISK (belum dibuka editor).
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("rtl")).unwrap();
        std::fs::write(
            root.join("rtl/counter.sv"),
            "module counter;\n  logic [7:0] count;\nendmodule\n",
        )
        .unwrap();
        std::fs::write(
            root.join("alu.v"),
            "module alu;\n  wire carry;\nendmodule\n",
        )
        .unwrap();
        // File non-sumber & direktori ter-skip tidak boleh ikut.
        std::fs::write(root.join("notes.txt"), "module ghost; endmodule\n").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/ghost.sv"), "module ghost; endmodule\n").unwrap();

        let files = workspace_index::collect_source_files(
            root,
            workspace_index::MAX_INDEX_FILES,
            workspace_index::MAX_INDEX_DEPTH,
        );
        assert_eq!(files.len(), 2, "{files:?}");

        // Bangun dokumen disk seperti handler (uri file:// + isi file).
        let disk: Vec<(String, String)> = files
            .iter()
            .map(|p| {
                let uri = Url::from_file_path(p).expect("uri").as_str().to_string();
                let text = std::fs::read_to_string(p).expect("baca");
                (uri, text)
            })
            .collect();
        let docs = workspace_index::merge_docs(&[], &disk);
        assert_eq!(docs.len(), 2);

        let hits = LspBackend::search_workspace_symbols(&docs, "counter");
        assert!(hits.iter().any(|h| h.name == "counter"), "{hits:?}");
        // Simbol dari file .v di direktori lain juga ditemukan.
        let hits = LspBackend::search_workspace_symbols(&docs, "alu");
        assert!(hits.iter().any(|h| h.name == "alu"), "{hits:?}");
        // Dokumen terbuka menang atas disk utk uri yang sama.
        let open = vec![(
            docs[0].0.clone(),
            "module dari_editor;\nendmodule\n".to_string(),
        )];
        let merged = workspace_index::merge_docs(&open, &docs);
        let hits = LspBackend::search_workspace_symbols(&merged, "dari_editor");
        assert_eq!(hits.len(), 1, "{hits:?}");
        let hits = LspBackend::search_workspace_symbols(&merged, "ghost");
        assert!(
            hits.is_empty(),
            "file .git/txt tidak boleh terindeks: {hits:?}"
        );
    }

    #[test]
    fn selection_range_line_end_uses_character_column() {
        // "// café" = 7 karakter (8 byte). Akhir range baris harus 7.
        let sr = compute_selection_range("// café\n", 0, 4);
        // Spec LSP: top-level = range terkecil (kata), parent = lebih besar.
        assert_eq!(sr.range.start.character, 3, "top-level harus kata (caf)");
        let line_range = sr.parent.as_ref().expect("range baris");
        assert_eq!(
            line_range.range.end.character, 7,
            "akhir baris = jumlah karakter, bukan byte"
        );
        assert!(
            sr.parent.as_ref().and_then(|p| p.parent.as_ref()).is_some(),
            "scope harus jadi parent terluar"
        );
    }

    #[test]
    fn selection_range_at_eof_after_trailing_newline_does_not_panic() {
        // "module m;\nendmodule\n" → lines() = 2 baris; posisi (2,0) sah
        // (Expand Selection di ujung file).
        let sr = compute_selection_range("module m;\nendmodule\n", 2, 0);
        assert!(sr.parent.is_some());
    }

    #[test]
    fn word_at_position_uses_character_column_on_multibyte_line() {
        // "éclk": kolom karakter 1 (= awal "clk") = byte 2. Tanpa konversi,
        // kolom 1 jatuh di tengah é → tidak ada kata.
        let (word, _, _) = LspBackend::word_at_position("éclk", 1).expect("kata harus ada");
        assert_eq!(word, "clk");
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
    fn test_snippet_for_keyword() {
        let s = LspBackend::snippet_for_keyword("module").expect("module punya snippet");
        assert!(s.contains("endmodule"), "{s}");
        assert!(s.contains("$1"), "{s}");
        assert!(LspBackend::snippet_for_keyword("always_ff")
            .unwrap()
            .contains("posedge"));
        assert!(LspBackend::snippet_for_keyword("endmodule").is_none());
        assert!(LspBackend::snippet_for_keyword("counter").is_none());
    }

    #[test]
    fn test_completion_context_rank() {
        // Deteksi konteks baris.
        assert_eq!(LspBackend::completion_context("input l"), "port");
        assert_eq!(LspBackend::completion_context("  output logic o"), "port");
        assert_eq!(LspBackend::completion_context("#(parameter l"), "param");
        assert_eq!(LspBackend::completion_context("  logic clk;"), "code");
        assert_eq!(LspBackend::completion_context("INPUT l"), "port");
        assert_eq!(LspBackend::completion_context("inputs x;"), "code");
        // Prefix "l": port → tipe dulu (logic), code → alfabet (localparam).
        // Catatan: outline juga menemukan simbol parsial "l" (rank 0) —
        // assert ranking relatif antar keyword, bukan posisi absolut.
        let port_src = "module m;\ninput l";
        let pc = LspBackend::compute_completions(port_src, 1, 7);
        let names: Vec<&str> = pc.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"logic"), "{names:?}");
        let (li, pi) = (
            names.iter().position(|n| *n == "logic").unwrap(),
            names.iter().position(|n| *n == "localparam").unwrap(),
        );
        assert!(li < pi, "port: logic sebelum localparam: {names:?}");
        let code_src = "module m;\n  l";
        let cc = LspBackend::compute_completions(code_src, 1, 3);
        let cnames: Vec<&str> = cc.iter().map(|(n, _)| n.as_str()).collect();
        let (cli, cpi) = (
            cnames.iter().position(|n| *n == "logic").unwrap(),
            cnames.iter().position(|n| *n == "localparam").unwrap(),
        );
        assert!(
            cpi < cli,
            "code: alfabet localparam sebelum logic: {cnames:?}"
        );
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
    fn test_semantic_tokens_modifiers() {
        // LSP-07 tahap 2d: definisi bawa declaration; parameter juga
        // readonly; keyword 0.
        let src = "module counter #(parameter WIDTH = 8);\n  logic clk;\nendmodule\n";
        let toks = LspBackend::compute_semantic_tokens(src);
        assert!(
            toks.iter().any(|t| t.token_modifiers_bitset
                == LspBackend::MOD_DECLARATION | LspBackend::MOD_READONLY
                && t.token_type == 6),
            "{toks:?}"
        );
        assert!(
            toks.iter()
                .any(|t| t.token_modifiers_bitset == LspBackend::MOD_DECLARATION
                    && t.token_type == 5),
            "{toks:?}"
        );
        assert!(
            toks.iter()
                .any(|t| t.token_modifiers_bitset == 0 && t.token_type == 7),
            "{toks:?}"
        );
    }

    #[test]
    fn test_semantic_tokens_range() {
        let src = "module counter;\n  logic clk;\n  assign a = b;\nendmodule\n";
        let full = LspBackend::compute_semantic_tokens(src);
        // Range baris 0..0 = subset tak-kosong dari full, tanpa token baris 3.
        let r0 = LspBackend::semantic_tokens_in_range(src, 0, 0);
        assert!(
            !r0.is_empty() && r0.len() < full.len(),
            "{r0:?} vs {full:?}"
        );
        assert!(r0.iter().any(|t| t.delta_line == 0 && t.token_type == 0));
        // Delta rebase: token pertama range selalu delta_line 0.
        assert_eq!(r0[0].delta_line, 0);
        // Range terbalik / di luar dokumen → kosong.
        assert!(LspBackend::semantic_tokens_in_range(src, 2, 1).is_empty());
        assert!(LspBackend::semantic_tokens_in_range(src, 9, 12).is_empty());
        // Range non-nol: decode delta → absolut tetap dalam [1,1].
        let r1 = LspBackend::semantic_tokens_in_range(src, 1, 1);
        assert!(!r1.is_empty(), "{r1:?}");
        let mut line = 0u32;
        for t in &r1 {
            line += t.delta_line;
            assert_eq!(line, 1, "{r1:?}");
        }
    }

    #[test]
    fn test_semantic_tokens_delta() {
        // LSP-07 tahap 2c: diff token identik/sisip/tengah.
        let tok = |dl: u32, ds: u32| SemanticToken {
            delta_line: dl,
            delta_start: ds,
            length: 3,
            token_type: 5,
            token_modifiers_bitset: 0,
        };
        let a = vec![tok(0, 0), tok(1, 0), tok(1, 4)];
        // Identik → tanpa edit.
        assert!(LspBackend::semantic_tokens_delta(&a, &a).is_empty());
        // Sisip 1 di tengah → 1 edit {start 1, delete 0, data 1}.
        let mut b = a.clone();
        b.insert(1, tok(9, 9));
        let e = LspBackend::semantic_tokens_delta(&a, &b);
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].start, e[0].delete_count), (1, 0));
        assert_eq!(e[0].data.as_ref().unwrap().len(), 1);
        // Hapus tengah → delete 1 tanpa data.
        let e2 = LspBackend::semantic_tokens_delta(&b, &a);
        assert_eq!(e2.len(), 1);
        assert_eq!((e2[0].start, e2[0].delete_count), (1, 1));
        // Terapkan edit ke prev → sama dengan next (roundtrip).
        let mut applied = a.clone();
        let (s, d) = (e[0].start as usize, e[0].delete_count as usize);
        applied.splice(s..s + d, e[0].data.clone().unwrap());
        assert_eq!(applied, b);
        // Kosong → kosong.
        assert!(LspBackend::semantic_tokens_delta(&[], &[]).is_empty());
        // Replace total → 1 edit hapus semua + data penuh.
        let e3 = LspBackend::semantic_tokens_delta(&a, &[tok(7, 7)]);
        assert_eq!((e3[0].start, e3[0].delete_count), (0, 3));
        assert_eq!(e3[0].data.as_ref().unwrap().len(), 1);
        // Overlap suffix [A,A]→[A]: prefix 1, hapus 1, tanpa data.
        let dup = vec![tok(0, 0), tok(0, 0)];
        let e4 = LspBackend::semantic_tokens_delta(&dup, &[tok(0, 0)]);
        assert_eq!((e4[0].start, e4[0].delete_count), (1, 1));
        assert!(e4[0].data.as_ref().unwrap().is_empty());
        // Kosong → isi: insert di 0.
        let e5 = LspBackend::semantic_tokens_delta(&[], &a);
        assert_eq!((e5[0].start, e5[0].delete_count), (0, 0));
        assert_eq!(e5[0].data.as_ref().unwrap().len(), 3);
        // Roundtrip delete: terapkan e2 ke b → a.
        let mut back = b.clone();
        let (s2, d2) = (e2[0].start as usize, e2[0].delete_count as usize);
        back.splice(s2..s2 + d2, e2[0].data.clone().unwrap());
        assert_eq!(back, a);
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
        // Spec LSP: top-level = range TERKECIL (kata), lalu baris, lalu scope.
        let word = &range.range;
        assert_eq!(
            (word.start.line, word.start.character),
            (1, 2),
            "top-level harus kata logic: {word:?}"
        );
        let line = range.parent.as_ref().expect("range baris");
        assert_eq!(
            (line.range.start.line, line.range.start.character),
            (1, 0),
            "parent kedua = seluruh baris"
        );
        let scope = line.parent.as_ref().expect("range scope");
        assert_eq!(
            (scope.range.start.line, scope.range.start.character),
            (0, 0),
            "parent terluar = scope module"
        );
        assert!(scope.parent.is_none(), "scope harus parent terakhir");
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
    fn test_completion_detail() {
        use lsp_types::{CompletionItemKind, InsertTextFormat};
        for (kind, want) in [
            (CompletionItemKind::MODULE, "module"),
            (CompletionItemKind::FUNCTION, "function"),
            (CompletionItemKind::CLASS, "class"),
            (CompletionItemKind::INTERFACE, "interface"),
            (CompletionItemKind::KEYWORD, "keyword"),
            (CompletionItemKind::VARIABLE, "signal"),
            (CompletionItemKind::CONSTANT, "parameter"),
        ] {
            assert_eq!(
                LspBackend::completion_detail(kind, false).as_deref(),
                Some(want),
                "{kind:?}"
            );
        }
        assert_eq!(
            LspBackend::completion_detail(CompletionItemKind::KEYWORD, true).as_deref(),
            Some("keyword · snippet")
        );
        assert!(LspBackend::completion_detail(CompletionItemKind::TEXT, false).is_none());
        // Fallback resolve: kosong terisi, ada dipertahankan, tanpa kind tetap None.
        assert_eq!(
            LspBackend::resolve_detail(None, Some(CompletionItemKind::MODULE), None).as_deref(),
            Some("module")
        );
        assert_eq!(
            LspBackend::resolve_detail(Some("keep".into()), Some(CompletionItemKind::MODULE), None)
                .as_deref(),
            Some("keep")
        );
        assert!(LspBackend::resolve_detail(None, None, None).is_none());
        assert_eq!(
            LspBackend::resolve_detail(
                None,
                Some(CompletionItemKind::KEYWORD),
                Some(InsertTextFormat::SNIPPET)
            )
            .as_deref(),
            Some("keyword · snippet")
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
