//! LANG-02: SystemC front-end — STUB stage 1-2 (Critical, open).
//!
//! mivon tidak memiliki dukungan SystemC sama sekali. Stage 1 adalah scanner
//! terbatas yang mengenali makro `SC_MODULE(<nama>)` dan `SC_CTOR(<nama>)`
//! dari source SystemC (C++). Stage 2 menambah ekstraksi deklarasi
//! port/signal `sc_in`/`sc_out`/`sc_inout`/`sc_signal` (nama + arah + tipe).
//! Stage 3 menambah ekstraksi process `SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`.
//! BUKAN parser C++/SystemC lengkap: belum mem-parse body process, maupun
//! tipe SystemC bersarang penuh. Belum di-wire ke pipeline compile mivon
//! (SystemVerilog).
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
    /// Deklarasi port/signal (`sc_in`/`sc_out`/`sc_inout`/`sc_signal`).
    pub ports: Vec<SystemCPort>,
    /// Deklarasi process (`SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`); stage 3.
    pub processes: Vec<SystemCProcess>,
}

/// Jenis process SystemC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemCProcessKind {
    /// `SC_METHOD`.
    Method,
    /// `SC_THREAD`.
    Thread,
    /// `SC_CTHREAD`.
    CThread,
}

/// Satu deklarasi process SystemC (`SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCProcess {
    pub kind: SystemCProcessKind,
    /// Nama metode/thread yang dijadikan process (identifier tepat setelah
    /// keyword, mis. `SC_METHOD(run)` -> "run").
    pub name: String,
    /// 1-based baris tempat keyword muncul.
    pub line: usize,
}

/// Arah deklarasi port/signal SystemC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemCPortDirection {
    /// `sc_in<T>` — port masukan.
    In,
    /// `sc_out<T>` — port keluaran.
    Out,
    /// `sc_inout<T>` — port bidirectional.
    InOut,
    /// `sc_signal<T>` — signal internal (bukan port modul).
    Signal,
}

/// Satu deklarasi port/signal SystemC (`sc_in`/`sc_out`/`sc_inout`/`sc_signal`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCPort {
    pub name: String,
    pub direction: SystemCPortDirection,
    /// Nama tipe: kata pertama setelah `<`. Template bersarang (mis.
    /// `sc_uint<8>`) hanya mengambil kata luarnya (`sc_uint`) — stage 2
    /// kasar; belum parse parameter template secara penuh.
    pub type_name: String,
    /// 1-based baris tempat deklarasi muncul.
    pub line: usize,
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
/// `SC_CTOR(<nama>)`, deklarasi port/signal `sc_in`/`sc_out`/`sc_inout`/
/// `sc_signal`, serta process `SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`.
///
/// Aturan (kasar, case-sensitive):
/// - Makro `SC_MODULE`/`SC_CTOR`: setelah kata makro, harapkan `(` lalu
///   identifier nama, lalu `)`. Nama = word tepat setelah `(`.
/// - Deklarasi port/signal: setelah keyword `sc_in`/`sc_out`/`sc_inout`/
///   `sc_signal`, harapkan `<tipe>` lalu identifier nama, lalu `;`. Tipe
///   diambil sebagai kata pertama setelah `<` (template bersarang diabaikan).
/// - Process: setelah keyword `SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`, harapkan
///   `(` lalu identifier nama, lalu `)`. Nama = word tepat setelah `(`
///   (untuk `SC_CTHREAD(nama, clk)` diambil nama pertama).
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
                "sc_in" | "sc_out" | "sc_inout" | "sc_signal" => {
                    // Arah berdasar keyword.
                    let direction = match w.as_str() {
                        "sc_in" => SystemCPortDirection::In,
                        "sc_out" => SystemCPortDirection::Out,
                        "sc_inout" => SystemCPortDirection::InOut,
                        _ => SystemCPortDirection::Signal,
                    };
                    // Cari `<` pertama setelah keyword.
                    let mut j = i + 1;
                    while j < n {
                        if let (_, Tok::Punct('<')) = &toks[j] {
                            break;
                        }
                        j += 1;
                    }
                    if j >= n {
                        i = j;
                        continue;
                    }
                    // Tipe = kata pertama setelah `<`.
                    let type_name = if let Some((_, Tok::Word(t))) = toks.get(j + 1) {
                        t.clone()
                    } else {
                        i = j + 1;
                        continue;
                    };
                    // Cari `>` penutup (tangani `<`/`>` bersarang).
                    let mut depth = 0i32;
                    let mut k = j;
                    let mut end = j;
                    while k < n {
                        match &toks[k] {
                            (_, Tok::Punct('<')) => depth += 1,
                            (_, Tok::Punct('>')) => {
                                depth -= 1;
                                if depth == 0 {
                                    end = k;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    if end == j {
                        i = j + 1;
                        continue;
                    }
                    // Nama = word setelah `>`.
                    let (nline, nm) = match toks.get(end + 1) {
                        Some((nl, Tok::Word(nm))) => (*nl, nm.clone()),
                        _ => {
                            i = end + 1;
                            continue;
                        }
                    };
                    res.ports.push(SystemCPort {
                        name: nm,
                        direction,
                        type_name,
                        line: nline,
                    });
                    i = end + 1;
                }
                "SC_METHOD" | "SC_THREAD" | "SC_CTHREAD" => {
                    // Jenis process dari keyword.
                    let kind = match w.as_str() {
                        "SC_METHOD" => SystemCProcessKind::Method,
                        "SC_THREAD" => SystemCProcessKind::Thread,
                        _ => SystemCProcessKind::CThread,
                    };
                    // Cari `(` setelah keyword, lalu nama, lalu `)`.
                    let mut j = i + 1;
                    while j < n {
                        if let (_, Tok::Punct('(')) = &toks[j] {
                            break;
                        }
                        j += 1;
                    }
                    if j >= n {
                        i = j;
                        continue;
                    }
                    let name = if let Some((nl, Tok::Word(pn))) = toks.get(j + 1) {
                        (*nl, pn.clone())
                    } else {
                        i = j + 1;
                        continue;
                    };
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
                        i = j + 1;
                        continue;
                    }
                    res.processes.push(SystemCProcess {
                        kind,
                        name: name.1,
                        line: name.0,
                    });
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

    #[test]
    fn scan_extracts_ports() {
        // Ekstraksi port sc_in/sc_out/sc_inout/sc_signal + tipe.
        let src = "\
SC_MODULE(alu) {
    sc_in<bool> clk;
    sc_out<sc_uint<8>> data;
    sc_inout<sc_logic> bidir;
    sc_signal<sc_int<4>> tmp;
    SC_CTOR(alu) { }
};
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.ports,
            vec![
                SystemCPort {
                    name: "clk".to_string(),
                    direction: SystemCPortDirection::In,
                    type_name: "bool".to_string(),
                    line: 2
                },
                SystemCPort {
                    name: "data".to_string(),
                    direction: SystemCPortDirection::Out,
                    type_name: "sc_uint".to_string(),
                    line: 3
                },
                SystemCPort {
                    name: "bidir".to_string(),
                    direction: SystemCPortDirection::InOut,
                    type_name: "sc_logic".to_string(),
                    line: 4
                },
                SystemCPort {
                    name: "tmp".to_string(),
                    direction: SystemCPortDirection::Signal,
                    type_name: "sc_int".to_string(),
                    line: 5
                },
            ]
        );
        // Modul/ctor tetap terdeteksi.
        assert_eq!(s.modules.len(), 1);
        assert_eq!(s.ctors.len(), 1);
    }

    #[test]
    fn scan_ports_case_sensitive() {
        // `SC_IN` (uppercase) BUKAN keyword port -> tidak terhitung.
        let src = "SC_IN<bool> not_a_port;\n";
        let s = scan(src).unwrap();
        assert!(s.ports.is_empty());
    }

    #[test]
    fn scan_extracts_processes() {
        // Process SC_METHOD/SC_THREAD/SC_CTHREAD diekstrak (nama + jenis).
        let src = "\
SC_MODULE(alu) {
    SC_CTOR(alu) {
        SC_METHOD(run);
        SC_THREAD(worker);
        SC_CTHREAD(tick, clk);
    }
};
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.processes,
            vec![
                SystemCProcess {
                    kind: SystemCProcessKind::Method,
                    name: "run".to_string(),
                    line: 3
                },
                SystemCProcess {
                    kind: SystemCProcessKind::Thread,
                    name: "worker".to_string(),
                    line: 4
                },
                SystemCProcess {
                    kind: SystemCProcessKind::CThread,
                    name: "tick".to_string(),
                    line: 5
                },
            ]
        );
        // Modul/ctor tetap terdeteksi.
        assert_eq!(s.modules.len(), 1);
        assert_eq!(s.ctors.len(), 1);
    }

    #[test]
    fn scan_process_case_sensitive() {
        // `sc_method` (lowercase) BUKAN process SystemC -> tidak terhitung.
        let src = "sc_method(not_proc);\n";
        let s = scan(src).unwrap();
        assert!(s.processes.is_empty());
    }
}
