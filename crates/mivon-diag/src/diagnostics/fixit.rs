//! Fix-it hint — representasi edit kode konkret yang bisa diterapkan otomatis.

use serde::{Deserialize, Serialize};

/// Fix-it: saran perbaikan otomatis (rentang + teks pengganti) — tipe rich
/// entry (Kritik-12) untuk struktur terserialisasi (MICD `FileDiags`); file
/// memegang `FileDiags::path`, jadi rentang TIDAK membawa file. Pindah dari
/// micd 2026-10 (konsolidasi diag — satu sumber tipe); wire format tetap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixIt {
    pub range: super::span::TextSpan,
    pub replacement: String,
    pub description: String,
}

/// Code action: aksi bernama yang ditawarkan IDE (bisa memuat edit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeAction {
    pub title: String,
    pub edit: Option<FixIt>,
}

/// Fix-it hint — representasi edit kode konkret yang bisa diterapkan otomatis.
#[derive(Debug, Clone)]
pub struct FixItHint {
    /// File yang perlu diedit
    pub file: String,
    /// Baris awal (1-based)
    pub start_line: usize,
    /// Kolom awal (1-based)
    pub start_col: usize,
    /// Baris akhir (1-based)
    pub end_line: usize,
    /// Kolom akhir (1-based)
    pub end_col: usize,
    /// Teks pengganti (kosong = hapus)
    pub replacement: String,
    /// Deskripsi singkat fix-it
    pub description: String,
}

impl FixItHint {
    /// Buat fix-it untuk insert teks di posisi tertentu
    pub fn insert(
        file: impl Into<String>,
        line: usize,
        col: usize,
        text: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        FixItHint {
            file: file.into(),
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
            replacement: text.into(),
            description: description.into(),
        }
    }

    /// Buat fix-it untuk replace rentang teks
    pub fn replace(
        file: impl Into<String>,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
        replacement: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        FixItHint {
            file: file.into(),
            start_line,
            start_col,
            end_line,
            end_col,
            replacement: replacement.into(),
            description: description.into(),
        }
    }

    /// Buat fix-it untuk hapus rentang teks
    pub fn delete(
        file: impl Into<String>,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
        description: impl Into<String>,
    ) -> Self {
        FixItHint::replace(
            file,
            start_line,
            start_col,
            end_line,
            end_col,
            "",
            description,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::span::TextSpan;

    #[test]
    fn test_fixit_create() {
        let fixit = FixIt {
            range: TextSpan::point(5, 10),
            replacement: "let x = 1;".to_string(),
            description: "Replace with const".to_string(),
        };
        assert_eq!(fixit.range.start_line, 5);
        assert_eq!(fixit.range.start_col, 10);
        assert_eq!(fixit.range.end_line, 5);
        assert_eq!(fixit.range.end_col, 10);
        assert_eq!(fixit.replacement, "let x = 1;");
        assert_eq!(fixit.description, "Replace with const");
    }

    #[test]
    fn test_codeaction_with_edit() {
        let fixit = FixIt {
            range: TextSpan::point(1, 1),
            replacement: "use foo::bar;".to_string(),
            description: "Add import".to_string(),
        };
        let action = CodeAction {
            title: "Add missing import".to_string(),
            edit: Some(fixit),
        };
        assert_eq!(action.title, "Add missing import");
        assert!(action.edit.is_some());
        assert_eq!(action.edit.unwrap().replacement, "use foo::bar;");
    }

    #[test]
    fn test_codeaction_without_edit() {
        let action = CodeAction {
            title: "Refactor".to_string(),
            edit: None,
        };
        assert_eq!(action.title, "Refactor");
        assert!(action.edit.is_none());
    }

    #[test]
    fn test_fixithint_insert() {
        let hint = FixItHint::insert(
            "foo.sv",
            3,
            5,
            "logic clk;",
            "Add clock declaration".to_string(),
        );
        assert_eq!(hint.file, "foo.sv");
        assert_eq!(hint.start_line, 3);
        assert_eq!(hint.start_col, 5);
        assert_eq!(hint.end_line, 3);
        assert_eq!(hint.end_col, 5);
        assert_eq!(hint.replacement, "logic clk;");
        assert_eq!(hint.description, "Add clock declaration");
    }

    #[test]
    fn test_fixithint_replace() {
        let hint = FixItHint::replace(
            "bar.sv",
            10,
            1,
            10,
            20,
            "wire [7:0] data;",
            "Replace signal declaration".to_string(),
        );
        assert_eq!(hint.file, "bar.sv");
        assert_eq!(hint.start_line, 10);
        assert_eq!(hint.start_col, 1);
        assert_eq!(hint.end_line, 10);
        assert_eq!(hint.end_col, 20);
        assert_eq!(hint.replacement, "wire [7:0] data;");
        assert_eq!(hint.description, "Replace signal declaration");
    }

    #[test]
    fn test_fixithint_delete() {
        let hint = FixItHint::delete(
            "baz.sv",
            5,
            1,
            5,
            10,
            "Remove unused signal".to_string(),
        );
        assert_eq!(hint.file, "baz.sv");
        assert_eq!(hint.start_line, 5);
        assert_eq!(hint.start_col, 1);
        assert_eq!(hint.end_line, 5);
        assert_eq!(hint.end_col, 10);
        assert_eq!(hint.replacement, "");
        assert_eq!(hint.description, "Remove unused signal");
    }

    #[test]
    fn test_fixithint_delete_equiv_replace_empty() {
        // delete() harus setara dengan replace() dengan replacement kosong.
        let del = FixItHint::delete("x.sv", 1, 1, 1, 5, "delete".to_string());
        let repl = FixItHint::replace("x.sv", 1, 1, 1, 5, "", "delete".to_string());
        assert_eq!(del.file, repl.file);
        assert_eq!(del.start_line, repl.start_line);
        assert_eq!(del.start_col, repl.start_col);
        assert_eq!(del.end_line, repl.end_line);
        assert_eq!(del.end_col, repl.end_col);
        assert_eq!(del.replacement, repl.replacement);
        assert_eq!(del.description, repl.description);
    }
}
