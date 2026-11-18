//! Concrete Syntax Tree (CST) foundation — stage 1 of PARSER-15
//! (CST untuk LSP — High, open).
//!
//! Pohon lossless yang melestarikan SETIAP token dan trivia dari lexer,
//! cocok untuk re-parsing inkremental dan fitur IDE (go-to-definition,
//! hover, rename) tanpa re-lexing seluruh file.
//! Belum di-wire ke parser/LSP — stage 1 adalah tipe node/pohon yang sudah
//! diuji.

use crate::lexer::Token;

/// Satu node CST: satu token ditambah anaknya yang terurut.
#[derive(Debug, Clone, PartialEq)]
pub struct CstNode {
    pub token: Token,
    /// Trivia leading (spasi/komen) sebelum token ini, sebagai teks mentah.
    pub leading_trivia: String,
    pub children: Vec<CstNode>,
}

impl CstNode {
    pub fn token(token: Token) -> Self {
        Self { token, leading_trivia: String::new(), children: Vec::new() }
    }

    pub fn with_trivia(token: Token, leading_trivia: String) -> Self {
        Self { token, leading_trivia, children: Vec::new() }
    }

    pub fn add_child(mut self, child: CstNode) -> Self {
        self.children.push(child);
        self
    }
}

/// Pohon CST lengkap untuk satu berkas sumber, di-root pada token sintetik
/// berjenis File.
#[derive(Debug, Clone, PartialEq)]
pub struct CstTree {
    pub root: CstNode,
}

impl CstTree {
    pub fn root(token: Token) -> Self {
        Self { root: CstNode::token(token) }
    }

    /// Bangun pohon lossless dari daftar token: setiap token menjadi anak
    /// langsung dari root, urutan dan identitas token terjaga (fondasi LSP
    /// go-to-definition/hover — PARSER-15). Trivia dikosongi di sini dan
    /// diisi pada stage pelengkap trivia.
    pub fn from_tokens(tokens: &[Token]) -> Self {
        let mut root = CstNode::token(Token::File);
        for t in tokens {
            root = root.add_child(CstNode::token(t.clone()));
        }
        Self { root }
    }
}

impl CstNode {
    /// Walk pre-order mengumpulkan setiap token (termasuk anak rekursif).
    pub fn tokens(&self) -> Vec<Token> {
        let mut out = Vec::new();
        out.push(self.token.clone());
        for child in &self.children {
            out.extend(child.tokens());
        }
        out
    }

    /// Cari pertama node (pre-order) whose token cocok `pred`.
    pub fn find_by_token<F>(&self, pred: F) -> Option<&CstNode>
    where
        F: Fn(&Token) -> bool + Clone,
    {
        if pred(&self.token) {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find_by_token(pred.clone()) {
                return Some(found);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Token;
    use mivon_core::intern::Symbol;

    #[test]
    fn tree_preserves_insertion_order_and_structure() {
        // module m; always @(posedge clk) begin ... end endmodule
        let mut root = CstTree::root(Token::Module);
        let ident_m = CstNode::token(Token::Ident(Symbol::intern("m")));
        let semi = CstNode::token(Token::Semi);
        root.root = root.root.add_child(ident_m).add_child(semi);

        let toks = root.root.tokens();
        assert_eq!(toks.len(), 3);
        assert!(matches!(toks[0], Token::Module));
        assert!(matches!(toks[1], Token::Ident(_)));
        assert!(matches!(toks[2], Token::Semi));
    }

    #[test]
    fn leading_trivia_is_preserved() {
        let node = CstNode::with_trivia(Token::Ident(Symbol::intern("clk")), "  // comment\n".to_string());
        assert_eq!(node.leading_trivia, "  // comment\n");
        assert!(matches!(node.token, Token::Ident(_)));
    }

    #[test]
    fn tokens_depth_first_preorder() {
        // root -> [a, [b, c], d]
        let mut root = CstNode::token(Token::Module);
        let mut a = CstNode::token(Token::Ident(Symbol::intern("a")));
        let mut b = CstNode::token(Token::Ident(Symbol::intern("b")));
        let c = CstNode::token(Token::Ident(Symbol::intern("c")));
        let d = CstNode::token(Token::Ident(Symbol::intern("d")));
        b = b.add_child(c);
        a = a.add_child(b);
        root = root.add_child(a).add_child(d);

        let toks: Vec<_> = root.tokens().iter().map(|t| match t {
            Token::Ident(s) => s.as_str().to_string(),
            other => format!("{:?}", other),
        }).collect();
        assert_eq!(toks, vec!["Module", "a", "b", "c", "d"]);
    }

    #[test]
    fn tree_clone_and_eq() {
        let a = CstTree::root(Token::Module);
        let b = a.clone();
        assert!(a == b);
    }

    #[test]
    fn cst_from_tokens_round_trip() {
        let toks = vec![
            Token::Module,
            Token::Ident(Symbol::intern("top")),
            Token::Semi,
            Token::Endmodule,
        ];
        let tree = CstTree::from_tokens(&toks);
        let back: Vec<Token> = tree.root.tokens();
        // root sintetis (File) diikuti semua token asli dalam urutan sama.
        assert_eq!(back.len(), toks.len() + 1);
        assert!(matches!(back[0], Token::File));
        assert_eq!(&back[1..], toks.as_slice());
    }

    #[test]
    fn cst_find_by_token_found() {
        let toks = vec![
            Token::Module,
            Token::Ident(Symbol::intern("top")),
            Token::Semi,
        ];
        let tree = CstTree::from_tokens(&toks);
        let found = tree.root.find_by_token(|t| matches!(t, Token::Ident(s) if s == "top"));
        assert!(found.is_some());
        assert!(matches!(found.unwrap().token, Token::Ident(_)));
    }

    #[test]
    fn cst_find_by_token_not_found() {
        let toks = vec![Token::Module, Token::Semi];
        let tree = CstTree::from_tokens(&toks);
        let found = tree.root.find_by_token(|t| matches!(t, Token::Endmodule));
        assert!(found.is_none());
    }
}
