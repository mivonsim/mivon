//! Span — source location untuk AST nodes.
//!
//! Setiap node AST memiliki span yang menunjuk ke file sumber original.
//! Zero-cost: Copy semantics, 16 bytes.

use crate::intern::Symbol;

/// Byte offset dalam file sumber.
pub type Offset = u32;

/// Range dalam file sumber.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct TextRange {
    /// Byte offset mulai (inclusive).
    pub start: Offset,
    /// Byte offset akhir (exclusive).
    pub end: Offset,
}

impl TextRange {
    /// Create a new text range.
    pub fn new(start: Offset, end: Offset) -> Self {
        TextRange { start, end }
    }

    /// Length in bytes.
    pub fn len(self) -> Offset {
        self.end - self.start
    }

    /// Whether the range is empty.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Check if a position is inside this range.
    pub fn contains(self, pos: Offset) -> bool {
        pos >= self.start && pos < self.end
    }

    /// Merge two adjacent or overlapping ranges.
    pub fn merge(self, other: TextRange) -> TextRange {
        TextRange {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// Source location span — file + byte range.
///
/// # Examples
///
/// ```
/// use mivon_diag::intern::Span;
///
/// let span = Span::new("test.sv", 0, 10);
/// assert_eq!(span.start(), 0);
/// ```
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Span {
    /// File path (interned).
    pub file: Symbol,
    /// Byte range in file.
    pub range: TextRange,
    /// Line number (1-based, lazily computed).
    pub line: u32,
    /// Column number (1-based, lazily computed).
    pub col: u32,
}

impl Span {
    /// Create a new span.
    pub fn new(file: impl Into<Symbol>, start: Offset, end: Offset) -> Self {
        Span {
            file: file.into(),
            range: TextRange::new(start, end),
            line: 1,
            col: 1,
        }
    }

    /// Create a span with known line and column.
    pub fn new_with_pos(
        file: impl Into<Symbol>,
        start: Offset,
        end: Offset,
        line: u32,
        col: u32,
    ) -> Self {
        Span {
            file: file.into(),
            range: TextRange::new(start, end),
            line,
            col,
        }
    }

    /// The start byte offset.
    pub fn start(self) -> Offset {
        self.range.start
    }

    /// The end byte offset.
    pub fn end(self) -> Offset {
        self.range.end
    }

    /// The length in bytes.
    pub fn len(self) -> Offset {
        self.range.len()
    }

    /// Whether this span is empty.
    pub fn is_empty(self) -> bool {
        self.range.is_empty()
    }

    /// The file path as a symbol.
    pub fn file(self) -> Symbol {
        self.file
    }

    /// The line number (1-based).
    pub fn line(self) -> u32 {
        self.line
    }

    /// The column number (1-based).
    pub fn col(self) -> u32 {
        self.col
    }

    /// Merge two spans (for compound nodes).
    /// Uses the file of the first span.
    pub fn merge(self, other: Span) -> Span {
        Span {
            file: self.file,
            range: self.range.merge(other.range),
            line: self.line.min(other.line),
            col: self.col,
        }
    }

    /// Create a span pointing to nothing (for synthetic nodes).
    pub fn synthetic(file: impl Into<Symbol>) -> Self {
        Span {
            file: file.into(),
            range: TextRange::new(0, 0),
            line: 0,
            col: 0,
        }
    }

    /// Whether this is a synthetic span.
    pub fn is_synthetic(self) -> bool {
        self.range.start == 0 && self.range.end == 0 && self.line == 0
    }
}

impl Default for Span {
    fn default() -> Self {
        Span {
            file: Symbol::EMPTY,
            range: TextRange::new(0, 0),
            line: 0,
            col: 0,
        }
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_basic() {
        let span = Span::new("test.sv", 10, 20);
        assert_eq!(span.start(), 10);
        assert_eq!(span.end(), 20);
        assert_eq!(span.len(), 10);
        assert_eq!(span.file(), Symbol::intern("test.sv"));
    }

    #[test]
    fn test_text_range() {
        let r = TextRange::new(5, 10);
        assert_eq!(r.len(), 5);
        assert!(r.contains(7));
        assert!(!r.contains(10)); // exclusive end
        assert!(!r.contains(4));
    }

    #[test]
    fn test_text_range_merge() {
        let r1 = TextRange::new(0, 5);
        let r2 = TextRange::new(10, 20);
        let merged = r1.merge(r2);
        assert_eq!(merged.start, 0);
        assert_eq!(merged.end, 20);
    }

    #[test]
    fn test_span_merge() {
        let s1 = Span::new("f.sv", 0, 10);
        let s2 = Span::new("f.sv", 20, 30);
        let merged = s1.merge(s2);
        assert_eq!(merged.start(), 0);
        assert_eq!(merged.end(), 30);
    }

    #[test]
    fn test_synthetic_span() {
        let span = Span::synthetic("builtin");
        assert!(span.is_synthetic());
    }
}
