//! LANG-02: SystemC front-end — STUB stage 1 (Critical, open).
//!
//! mivon tidak memiliki dukungan SystemC sama sekali. Stage 1 adalah scanner
//! terbatas yang mengenali makro `SC_MODULE(<nama>)` dan `SC_CTOR(<nama>)`
//! dari source SystemC (C++). Mengembalikan nama modul/ctor + nomor baris.
//! BUKAN parser C++/SystemC lengkap: belum mem-parse port (`sc_in`/`sc_out`),
//! signal (`sc_signal`), process (`SC_METHOD`/`SC_THREAD`), maupun tipe
//! SystemC. Belum di-wire ke pipeline compile mivon (SystemVerilog).
//!
//! Ini titik awal yang kecil namun ter-test; item besar LANG-02 akan
//! dipecah bertahap (parser C++/SystemC, elaborasi, simulasi) di stage
//! berikutnya.

/// Satu modul SystemC yang dikenali (`SC_MODULE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCModule {
    pub name: String,
    /// 1-based baris tempat makro `SC_MODULE` muncul.
    pub line: usize,
}

/// Satu konstruktor SystemC yang dikenali (`SC_CTOR`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCCtor {
    pub name: String,
    /// 1-based baris tempat makro `SC_CTOR` muncul.
    pub line: usize,
}

/// Hasil scan SystemC.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemCScan {
    pub modules: Vec<SystemCModule>,
    pub ctors: Vec<SystemCCtor>,
}

/// Kesalahan scan tingkat rendah (stage 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemCScanError {
    /// Makro tak ditutup kurung `)`.
    Unterminated(String),
}

/// Token kasar hasil tokenisasi (C++/SystemC): word alphanumerik + `_`,
/// tanda kurung, titik-koma; melacak nomor baris. Komentar `//` dan `/* */`
/// diabaikan. String literal diabaikan secara kasar (tidak di-skip — scanner
/// ini hanya mencari makro di tingkat modul, bukan di dalam string).
fn tokenize(src: &str) -> Vec<(usize, Tok)> {
    let ch: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    while i < ch.len() {
        let c = ch[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Komentar baris `//`.
        if c == '/' && i + 1 < ch.len() && ch[i + 1] == '/' {
            while i < ch.len() && ch[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // Komentar blok `/* */`.
        if c == '/' && i + 1 < ch.len() && ch[i + 1] == '*' {
            i += 2;
            while i + 1 < ch.len() && !(ch[i] == '*' && ch[i + 1] == '/') {
                if ch[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2; // lewati `*/`
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < ch.len() && (ch[i].is_alphanumeric() || ch[i] == '_') {
                i += 1;
            }
            let word: String = ch[start..i].iter().collect();
            out.push((line, Tok::Word(word)));
        } else {
            out.push((line, Tok::Punct(c)));
            i += 1;
        }
    }
    out
}

/// Token kasar.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Punct(char),
}

/// Scan source SystemC dan kenali makro `SC_MODULE(<nama>)` dan
/// `SC_CTOR(<nama>)`.
///
/// Aturan (kasar, case-sensitive — makro SystemC ditulis persis `SC_MODULE`
/// / `SC_CTOR`): setelah kata makro, harapkan `(` lalu identifier nama, lalu
/// `)`. Nama diambil sebagai word tepat setelah `(`.
pub fn scan(src: &str) -> Result<SystemCScan, SystemCScanError> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut res = SystemCScan::default();
    let mut i = 0usize;
    while i < n {
        if let (ln, Tok::Word(w)) = &toks[i] {
            match w.as_str() {
                "SC_MODULE" | "SC_CTOR" => {
                    // Cari `(` berikutnya, lalu nama, lalu `)`.
                    let mut j = i + 1;
                    // Lewati whitespace/punct sampai `(`.
                    while j < n {
                        if let (_, Tok::Punct('(')) = &toks[j] {
                            break;
                        }
                        j += 1;
                    }
                    if j >= n {
                        return Err(SystemCScanError::Unterminated(w.clone()));
                    }
                    // Nama = word tepat setelah `(`.
                    let name = if let Some((_, Tok::Word(nm))) = toks.get(j + 1) {
                        nm.clone()
                    } else {
                        return Err(SystemCScanError::Unterminated(w.clone()));
                    };
                    // Cari `)` penutup setelah nama; jika tak ada -> error.
                    let mut k = j + 2;
                    let mut found = false;
                    while k < n {
                        if let (_, Tok::Punct(')')) = &toks[k] {
                            found = true;
                            break;
                        }
                        k += 1;
                    }
                    if !found {
                        return Err(SystemCScanError::Unterminated(w.clone()));
                    }
                    let mline = *ln;
                    if w == "SC_MODULE" {
                        res.modules.push(SystemCModule { name, line: mline });
                    } else {
                        res.ctors.push(SystemCCtor { name, line: mline });
                    }
                    i = k;
                }
                _ => {}
            }
        }
        i += 1;
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_sc_module_and_ctor() {
        let src = "\
SC_MODULE(alu) {
    sc_in<bool> clk;
    SC_CTOR(alu) {
        SC_METHOD(run);
        sensitive << clk.pos();
    }
};

SC_MODULE(tb) {
    SC_CTOR(tb) { }
};
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.modules,
            vec![
                SystemCModule {
                    name: "alu".to_string(),
                    line: 1
                },
                SystemCModule {
                    name: "tb".to_string(),
                    line: 9
                },
            ]
        );
        assert_eq!(
            s.ctors,
            vec![
                SystemCCtor {
                    name: "alu".to_string(),
                    line: 3
                },
                SystemCCtor {
                    name: "tb".to_string(),
                    line: 10
                },
            ]
        );
    }

    #[test]
    fn scan_skips_line_comment() {
        // Makro di dalam komentar `//` tidak dikenali.
        let src = "// SC_MODULE(ghost)\nSC_MODULE(real) { SC_CTOR(real) {} };\n";
        let s = scan(src).unwrap();
        assert_eq!(s.modules.len(), 1);
        assert_eq!(s.modules[0].name, "real");
        assert_eq!(s.ctors.len(), 1);
    }

    #[test]
    fn scan_skips_block_comment() {
        // Makro di dalam komentar `/* */` tidak dikenali.
        let src = "/* SC_MODULE(ghost) */\nSC_MODULE(real) { SC_CTOR(real) {} };\n";
        let s = scan(src).unwrap();
        assert_eq!(s.modules.len(), 1);
        assert_eq!(s.ctors.len(), 1);
    }

    #[test]
    fn scan_case_sensitive_macro() {
        // `sc_module` (lowercase) BUKAN makro SystemC -> tidak dikenali.
        let src = "sc_module(not_macro) { };\n";
        let s = scan(src).unwrap();
        assert!(s.modules.is_empty());
        assert!(s.ctors.is_empty());
    }

    #[test]
    fn scan_unterminated_module_errors() {
        // Tak ada `)` penutup -> error Unterminated.
        let r = scan("SC_MODULE(dangling");
        assert!(matches!(r, Err(SystemCScanError::Unterminated(_))));
    }
}
