use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

use dashmap::{DashMap, Entry};

// PERF-08: Thread-local cache untuk mengurangi DashMap contention.
// Setiap thread menyimpan HashMap<String, Symbol> kecil (max 256 entries)
// yang di-cache dari hasil intern sebelumnya.
const TL_CACHE_MAX: usize = 256;

thread_local! {
    static TL_CACHE: RefCell<HashMap<String, Symbol>> = RefCell::new(HashMap::with_capacity(TL_CACHE_MAX));
}

// ─── Symbol ───

/// Interned string identifier.
///
/// # Examples
///
/// ```
/// use mivon_diag::intern::Symbol;
///
/// let sym = Symbol::intern("hello");
/// assert_eq!(sym.as_str(), "hello");
/// ```
#[derive(Copy, Clone, Eq, PartialEq, Default)]
pub struct Symbol(u32);

// Debug manual: index intern (u32) tidak berarti bagi pembaca. Cetak string
// aslinya agar dump AST (`mivon --ast`) menampilkan nama sinyal/modul, bukan
// `Symbol(54)`. Format `Symbol("counter")` mempertahankan penanda tipe.
// Lookup langsung ke tabel (bukan `as_str`) agar index tak valid — mungkin
// lewat `unsafe from_index` — tidak membuat Debug panic; fallback `Symbol(#id)`.
impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(s) = table().strings.read().get(self.0 as usize) {
            return write!(f, "Symbol({:?})", s.as_ref());
        }
        write!(f, "Symbol(#{})", self.0)
    }
}

impl Symbol {
    /// The empty symbol.
    pub const EMPTY: Symbol = Symbol(0);

    /// Create a new symbol for the given string.
    /// Interns the string if not already present.
    pub fn intern(s: &str) -> Self {
        table().intern(s)
    }

    /// Get the string representation of this symbol.
    pub fn as_str(&self) -> &'static str {
        table().get(self.0)
    }

    /// Get the raw index.
    pub fn index(self) -> u32 {
        self.0
    }

    /// Number of unique symbols interned so far.
    pub fn count() -> u32 {
        table().strings.read().len() as u32
    }

    /// Create a symbol from a raw index (unsafe — only for deserialization).
    ///
    /// # Safety
    ///
    /// `idx` must be a valid index previously returned by [`Symbol::intern`] or
    /// [`Symbol::index`]. Passing an arbitrary value may produce a symbol whose
    /// string cannot be looked up (`as_str` will panic on out-of-bounds).
    pub unsafe fn from_index(idx: u32) -> Self {
        Symbol(idx)
    }

    // ─── Convenience methods (delegate to as_str()) ───

    /// Check if the symbol's string starts with the given pattern.
    pub fn starts_with(&self, pat: &str) -> bool {
        self.as_str().starts_with(pat)
    }

    /// Check if the symbol's string ends with the given pattern.
    pub fn ends_with(&self, pat: &str) -> bool {
        self.as_str().ends_with(pat)
    }

    /// Check if the symbol's string contains the given pattern.
    pub fn contains(&self, pat: &str) -> bool {
        self.as_str().contains(pat)
    }

    /// Split the symbol's string by the given delimiter.
    pub fn split<'a>(&'a self, pat: &'a str) -> std::str::Split<'a, &'a str> {
        self.as_str().split(pat)
    }

    /// Split the symbol's string once by the given delimiter.
    pub fn split_once(&self, pat: &str) -> Option<(&str, &str)> {
        self.as_str().split_once(pat)
    }

    /// Strip the given suffix from the symbol's string.
    pub fn strip_suffix(&self, pat: &str) -> Option<&'static str> {
        self.as_str().strip_suffix(pat)
    }

    /// Convert to lowercase (returns owned String).
    pub fn to_lowercase(&self) -> String {
        self.as_str().to_lowercase()
    }

    /// Get the length of the symbol's string.
    pub fn len(&self) -> usize {
        self.as_str().len()
    }

    /// Check if the symbol's string is empty.
    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }

    /// Convert to a `&str` slice (alias for `as_str()`).
    pub fn as_deref(&self) -> &'static str {
        self.as_str()
    }

    /// Iterate over the characters of the symbol's string.
    pub fn chars(&self) -> std::str::Chars<'_> {
        self.as_str().chars()
    }

    /// Search for a character from the right.
    pub fn rfind(&self, pat: char) -> Option<usize> {
        self.as_str().rfind(pat)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ─── Serde ───
// Serialisasi sebagai string (bukan u32 index) karena index intern
// bersifat proses-lokal dan tidak stabil antar proses. Deserialisasi
// meng-intern ulang string ke tabel global proses saat ini.

impl serde::Serialize for Symbol {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Symbol {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Symbol::intern(&s))
    }
}

impl Hash for Symbol {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash by u32 index (bukan string content). Hash content-nya setara
        // karena intern menjamin string yang sama → index yang sama. Ini
        // menghindari (a) lock pada string table, (b) SipHash over string —
        // untuk HashMap<Symbol, _> berukuran puluhan ribu entri (parameter
        // context OpenTitan), hashing string per lookup adalah bottleneck
        // terbesar (perf: as_str + memcmp + SipHash ≈ 50% CPU elaboration).
        //
        // Konsekuensi: `Borrow<str>` TIDAK lagi tersedia untuk Symbol — semua
        // lookup `map.get("literal")` / `map.get(x.as_str())` WAJIB memakai
        // Symbol::intern terlebih dahulu (`map.get(&Symbol::intern("x"))`
        // atau `map.get(&sym)`). Kompilator menemukan seluruh situs yang
        // melanggar.
        self.0.hash(state);
    }
}

impl From<&str> for Symbol {
    fn from(s: &str) -> Self {
        Symbol::intern(s)
    }
}

impl PartialEq<&str> for Symbol {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<str> for Symbol {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl AsRef<str> for Symbol {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

// CATATAN: `impl Borrow<str> for Symbol` DIHAPUS. Symbol kini hash by u32
// index (lihat impl Hash) sehingga lookup `HashMap<Symbol, V>::get("literal")`
// tidak lagi valid (hash string ≠ hash u32). Semua lookup harus memakai
// Symbol yang sudah di-intern: `map.get(&Symbol::intern("x"))` / `map.get(&sym)`.

impl PartialEq<String> for Symbol {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

// ─── Global String Table ───

/// Thread-safe string table with O(1) DashMap lookup and O(1) intern.
///
/// Design:
/// - `lookup`: DashMap for O(1) string→u32 lookup
/// - `strings`: append-only Vec of `Box<str>` — DIMILIKI table, sehingga
///   `reset_string_table()` benar-benar membebaskan memori (tidak `Box::leak`).
/// - RwLock permits concurrent reads without serialization
struct StringTable {
    /// O(1) hash-based lookup — string → u32 index
    lookup: DashMap<String, u32, fxhash::FxBuildHasher>,
    /// Indexed storage — `Box<str>` owned oleh table.
    /// Lifetime `'static` yang dibagikan via `Symbol::get` dibuat dengan
    /// transmute: memori hidup selama table (di-free saat `reset_string_table`).
    /// RwLock: concurrent reads (as_str) don't block each other.
    strings: parking_lot::RwLock<Vec<Box<str>>>,
    // ID is derived from strings.len() under the lock — no separate counter needed.
}

fn table() -> &'static StringTable {
    static TABLE: OnceLock<StringTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let table = StringTable::new();
        // Populate 54 pre-defined symbols (index 0 = empty, 1..=53 = keywords)
        let prelude = vec![
            "",              // 0  = EMPTY
            "logic",         // 1
            "wire",          // 2
            "reg",           // 3
            "int",           // 4
            "integer",       // 5
            "bit",           // 6
            "byte",          // 7
            "shortint",      // 8
            "longint",       // 9
            "real",          // 10
            "time",          // 11
            "string",        // 12
            "input",         // 13
            "output",        // 14
            "inout",         // 15
            "module",        // 16
            "endmodule",     // 17
            "assign",        // 18
            "always",        // 19
            "initial",       // 20
            "begin",         // 21
            "end",           // 22
            "if",            // 23
            "else",          // 24
            "for",           // 25
            "while",         // 26
            "case",          // 27
            "parameter",     // 28
            "localparam",    // 29
            "function",      // 30
            "endfunction",   // 31
            "task",          // 32
            "endtask",       // 33
            "generate",      // 34
            "endgenerate",   // 35
            "class",         // 36
            "endclass",      // 37
            "package",       // 38
            "endpackage",    // 39
            "interface",     // 40
            "endinterface",  // 41
            "fork",          // 42
            "join",          // 43
            "typedef",       // 44
            "enum",          // 45
            "struct",        // 46
            "union",         // 47
            "import",        // 48
            "bind",          // 49
            "uvm_object",    // 50
            "uvm_component", // 51
            "uvm_test",      // 52
            "process",       // 53
        ];
        for s in &prelude {
            table.intern(s);
        }
        table
    })
}

/// Clear the string table for daemon mode (frees all memory).
pub fn reset_string_table() {
    let t = table();
    t.lookup.clear();
    let mut strings = t.strings.write();
    strings.clear();
    let prelude = vec![
        "",
        "logic",
        "wire",
        "reg",
        "int",
        "integer",
        "bit",
        "byte",
        "shortint",
        "longint",
        "real",
        "time",
        "string",
        "input",
        "output",
        "inout",
        "module",
        "endmodule",
        "assign",
        "always",
        "initial",
        "begin",
        "end",
        "if",
        "else",
        "for",
        "while",
        "case",
        "parameter",
        "localparam",
        "function",
        "endfunction",
        "task",
        "endtask",
        "generate",
        "endgenerate",
        "class",
        "endclass",
        "package",
        "endpackage",
        "interface",
        "endinterface",
        "fork",
        "join",
        "typedef",
        "enum",
        "struct",
        "union",
        "import",
        "bind",
        "uvm_object",
        "uvm_component",
        "uvm_test",
        "process",
    ];
    for s in &prelude {
        t.intern(s);
    }
}

impl StringTable {
    fn new() -> Self {
        StringTable {
            lookup: DashMap::with_hasher(fxhash::FxBuildHasher::default()),
            strings: parking_lot::RwLock::new(Vec::with_capacity(10240)),
        }
    }

    /// Intern a string — O(1) amortized.
    ///
    /// Uses DashMap `entry()` API for atomic check-and-insert,
    /// eliminating the O(n²) bottleneck from the old linear-scan approach.
    /// Fast path (cache hit) avoids allocation by using `get()` first.
    fn intern(&self, s: &str) -> Symbol {
        // PERF-08: Thread-local cache hit (zero DashMap contention)
        let result = TL_CACHE.with(|cache| cache.borrow().get(s).copied());
        if let Some(sym) = result {
            return sym;
        }

        // Fast path: no allocation on cache hit (common case — keywords interned thousands of times)
        let sym = if let Some(sym) = self.lookup.get(s) {
            Symbol(*sym)
        } else {
            // Slow path: allocate and insert atomically via entry() API
            let owned = s.to_string();
            match self.lookup.entry(owned) {
                Entry::Occupied(e) => Symbol(*e.get()),
                Entry::Vacant(e) => {
                    let boxed: Box<str> = Box::from(s);
                    let mut strings = self.strings.write();
                    let id = strings.len() as u32;
                    strings.push(boxed);
                    e.insert(id);
                    Symbol(id)
                }
            }
        };

        // PERF-08: Update thread-local cache
        TL_CACHE.with(|cache| {
            let mut c = cache.borrow_mut();
            if c.len() >= TL_CACHE_MAX {
                let keys: Vec<String> = c.keys().take(TL_CACHE_MAX / 2).cloned().collect();
                for k in keys {
                    c.remove(&k);
                }
            }
            c.insert(s.to_string(), sym);
        });

        sym
    }

    /// Retrieve a string by its u32 ID — O(1).
    ///
    /// `&'static str` dihasilkan dengan transmute lifetime dari `&Box<str>`
    /// di dalam table. Memori di-free saat `reset_string_table`; setelah itu
    /// Symbol lama tidak boleh dipakai (kontrak sama seperti Box::leak).
    fn get(&self, id: u32) -> &'static str {
        let strings = self.strings.read();
        let view = strings[id as usize].as_ref();
        unsafe { std::mem::transmute::<&str, &'static str>(view) }
    }
}

// ─── Pre-populated Symbols ───

/// Pre-populated symbols untuk keywords umum.
pub mod prelude {
    use super::Symbol;

    pub const S_EMPTY: Symbol = Symbol(0);
    pub const S_LOGIC: Symbol = Symbol(1);
    pub const S_WIRE: Symbol = Symbol(2);
    pub const S_REG: Symbol = Symbol(3);
    pub const S_INT: Symbol = Symbol(4);
    pub const S_INTEGER: Symbol = Symbol(5);
    pub const S_BIT: Symbol = Symbol(6);
    pub const S_BYTE: Symbol = Symbol(7);
    pub const S_SHORTINT: Symbol = Symbol(8);
    pub const S_LONGINT: Symbol = Symbol(9);
    pub const S_REAL: Symbol = Symbol(10);
    pub const S_TIME: Symbol = Symbol(11);
    pub const S_STRING: Symbol = Symbol(12);
    pub const S_INPUT: Symbol = Symbol(13);
    pub const S_OUTPUT: Symbol = Symbol(14);
    pub const S_INOUT: Symbol = Symbol(15);
    pub const S_MODULE: Symbol = Symbol(16);
    pub const S_ENDMODULE: Symbol = Symbol(17);
    pub const S_ASSIGN: Symbol = Symbol(18);
    pub const S_ALWAYS: Symbol = Symbol(19);
    pub const S_INITIAL: Symbol = Symbol(20);
    pub const S_BEGIN: Symbol = Symbol(21);
    pub const S_END: Symbol = Symbol(22);
    pub const S_IF: Symbol = Symbol(23);
    pub const S_ELSE: Symbol = Symbol(24);
    pub const S_FOR: Symbol = Symbol(25);
    pub const S_WHILE: Symbol = Symbol(26);
    pub const S_CASE: Symbol = Symbol(27);
    pub const S_PARAMETER: Symbol = Symbol(28);
    pub const S_LOCALPARAM: Symbol = Symbol(29);
    pub const S_FUNCTION: Symbol = Symbol(30);
    pub const S_ENDFUNCTION: Symbol = Symbol(31);
    pub const S_TASK: Symbol = Symbol(32);
    pub const S_ENDTASK: Symbol = Symbol(33);
    pub const S_GENERATE: Symbol = Symbol(34);
    pub const S_ENDGENERATE: Symbol = Symbol(35);
    pub const S_CLASS: Symbol = Symbol(36);
    pub const S_ENDCLASS: Symbol = Symbol(37);
    pub const S_PACKAGE: Symbol = Symbol(38);
    pub const S_ENDPACKAGE: Symbol = Symbol(39);
    pub const S_INTERFACE: Symbol = Symbol(40);
    pub const S_ENDINTERFACE: Symbol = Symbol(41);
    pub const S_FORK: Symbol = Symbol(42);
    pub const S_JOIN: Symbol = Symbol(43);
    pub const S_TYPEDEF: Symbol = Symbol(44);
    pub const S_ENUM: Symbol = Symbol(45);
    pub const S_STRUCT: Symbol = Symbol(46);
    pub const S_UNION: Symbol = Symbol(47);
    pub const S_IMPORT: Symbol = Symbol(48);
    pub const S_BIND: Symbol = Symbol(49);
    pub const S_UVM_OBJECT: Symbol = Symbol(50);
    pub const S_UVM_COMPONENT: Symbol = Symbol(51);
    pub const S_UVM_TEST: Symbol = Symbol(52);
    pub const S_PROCESS: Symbol = Symbol(53);
}

/// Initialize the global string table.
pub fn init_string_table() {
    let _ = table();
}

/// Convert a Symbol back to an owned String (for legacy interop).
pub fn sym_to_string(sym: Symbol) -> String {
    sym.as_str().to_string()
}

/// Intern all strings in a Vec into Symbols.
pub fn strings_to_symbols(strings: &[String]) -> Vec<Symbol> {
    strings.iter().map(|s| Symbol::intern(s)).collect()
}

/// Convert all Symbols in a slice to Strings.
pub fn symbols_to_strings(symbols: &[Symbol]) -> Vec<String> {
    symbols.iter().map(|s| s.as_str().to_string()).collect()
}

/// Intern all strings in a Vec<&str> into Symbols (borrowed variant).
pub fn strs_to_symbols(strings: &[&str]) -> Vec<Symbol> {
    strings.iter().map(|s| Symbol::intern(s)).collect()
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_basic() {
        let s1 = Symbol::intern("hello");
        let s2 = Symbol::intern("hello");
        let s3 = Symbol::intern("world");

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
        assert_eq!(s1.as_str(), "hello");
        assert_eq!(s3.as_str(), "world");
    }

    #[test]
    fn test_intern_equality() {
        let a = Symbol::intern("foo_bar_123");
        let b = Symbol::intern("foo_bar_123");
        let c = Symbol::intern("foo_bar_124");

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_intern_empty() {
        let e = Symbol::EMPTY;
        assert_eq!(e.as_str(), "");
    }

    #[test]
    fn test_symbol_display() {
        let s = Symbol::intern("display_test");
        assert_eq!(format!("{}", s), "display_test");
    }

    /// `Debug` wajib menampilkan string, bukan index intern — dump AST
    /// (`mivon --ast`) mencetak `Design {:#?}`, jadi index mentah membuat
    /// seluruh output tak terbaca (`Symbol(54)`).
    #[test]
    fn test_symbol_debug_shows_string() {
        let s = Symbol::intern("debug_test_sig");
        assert_eq!(format!("{:?}", s), "Symbol(\"debug_test_sig\")");
    }

    #[test]
    fn test_thread_safety() {
        use std::thread;
        let mut handles = Vec::new();
        for t in 0..8 {
            handles.push(thread::spawn(move || {
                for i in 0..300 {
                    let s = format!("thread{}_{}", t, i);
                    let sym = Symbol::intern(&s);
                    assert_eq!(sym.as_str(), s.as_str(), "Mismatch for thread{}_[{}]", t, i);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_prelude_symbols() {
        assert_eq!(prelude::S_LOGIC.as_str(), "logic");
        assert_eq!(prelude::S_MODULE.as_str(), "module");
        assert_eq!(prelude::S_UVM_OBJECT.as_str(), "uvm_object");
    }

    #[test]
    fn test_dashmap_intern_speed() {
        let start = std::time::Instant::now();
        for i in 0..10000 {
            let s = format!("fast_intern_{}", i);
            let _sym = Symbol::intern(&s);
        }
        let elapsed = start.elapsed();
        eprintln!("Interned 10000 strings in {:?}", elapsed);
        // Should be fast — O(1) per intern now
        // (Generous threshold for debug builds)
        assert!(elapsed.as_secs() < 30, "too slow: {:?}", elapsed);
    }
}
