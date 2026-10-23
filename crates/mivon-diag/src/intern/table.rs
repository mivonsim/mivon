//! String table utilities — format buffer, fast string builders.

use crate::intern::Symbol;
use std::fmt;
use std::mem;

/// Pre-allocated format buffer untuk mengurangi allocation di hot path.
pub struct FormatBuf {
    buf: String,
}

impl FormatBuf {
    /// Create a new format buffer.
    pub fn new() -> Self {
        FormatBuf {
            buf: String::with_capacity(128),
        }
    }

    /// Create a format buffer with specific capacity.
    pub fn with_capacity(cap: usize) -> Self {
        FormatBuf {
            buf: String::with_capacity(cap),
        }
    }

    /// Clear the buffer for reuse.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Get a reference to the current content.
    pub fn as_str(&self) -> &str {
        &self.buf
    }

    /// Write a formatted string into the buffer.
    pub fn write(&mut self, args: fmt::Arguments<'_>) -> &str {
        use std::fmt::Write;
        self.buf.clear();
        self.buf.write_fmt(args).expect("FormatBuf write failed");
        self.as_str()
    }

    /// Write a symbol into the buffer.
    pub fn write_symbol(&mut self, sym: Symbol) -> &str {
        self.write(format_args!("{}", sym))
    }

    /// Take the string, clearing the buffer.
    pub fn take(&mut self) -> String {
        mem::take(&mut self.buf)
    }
}

impl Default for FormatBuf {
    fn default() -> Self {
        Self::new()
    }
}

/// Fast string interning untuk temporary/cached strings.
pub trait InternExt {
    fn intern(&self) -> Symbol;
}

impl InternExt for str {
    fn intern(&self) -> Symbol {
        Symbol::intern(self)
    }
}

impl InternExt for String {
    fn intern(&self) -> Symbol {
        Symbol::intern(self.as_str())
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_buf() {
        let mut buf = FormatBuf::new();
        let s = buf.write(format_args!("hello_{}", 42));
        assert_eq!(s, "hello_42");
    }

    #[test]
    fn test_format_buf_reuse() {
        let mut buf = FormatBuf::new();
        buf.write(format_args!("first"));
        buf.write(format_args!("second"));
        assert_eq!(buf.as_str(), "second");
    }

    #[test]
    fn test_intern_ext() {
        let sym = "test_identifier".intern();
        assert_eq!(sym.as_str(), "test_identifier");
    }
}
