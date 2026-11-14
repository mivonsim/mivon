//! LANG-01: VHDL front-end — STUB stage 1 (Critical, open).
//!
//! mivon tidak memiliki dukungan VHDL sama sekali. Stage 1 adalah scanner
//! terbatas yang mengenali batas `entity` / `architecture` dari source VHDL
//! (nama + nomor baris). BUKAN parser lengkap: belum mem-parse port,
//! generic, concurrent/sequential statement, maupun tipe VHDL. Belum
//! di-wire ke pipeline compile (lexer/parser SystemVerilog di mivon-parser).
//!
//! Ini titik awal yang kecil namun ter-test; item besar LANG-01 akan
//! dipecah bertahap (lexer VHDL, parser, elaborasi, simulasi) di stage
//! berikutnya.

/// Satu entitas VHDL yang dikenali.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlEntity {
    pub name: String,
    /// 1-based baris tempat kata kunci `entity` muncul.
    pub line: usize,
}

/// Satu arsitektur VHDL yang dikenali.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlArchitecture {
    pub name: String,
    pub entity: String,
    /// 1-based baris tempat kata kunci `architecture` muncul.
    pub line: usize,
}

/// Hasil scan VHDL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VhdlScan {
    pub entities: Vec<VhdlEntity>,
    pub architectures: Vec<VhdlArchitecture>,
}

/// Kesalahan scan tingkat rendah (stage 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VhdlScanError {
    /// Entitas/arsitektur tak ditutup oleh `end ... ;`.
    Unterminated(String),
}

/// Token kasar hasil tokenisasi (komentar `--` sudah dibuang).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Punct(char),
}

/// Tokenisasi kasar: pecah menjadi word (alphanumerik + `_`) dan tanda
/// baca tunggal, sambil melacak nomor baris. Komentar `--` diabaikan.
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
        // Komentar baris `--` (VHDL).
        if c == '-' && i + 1 < ch.len() && ch[i + 1] == '-' {
            while i < ch.len() && ch[i] != '\n' {
                i += 1;
            }
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

/// Scan source VHDL dan kenali entitas + arsitektur.
///
/// Aturan stage 1 (kasar, case-insensitive):
/// - `entity <nama> ... end [entity] [<nama>] ;` → entitas `<nama>`.
/// - `architecture <nama> of <ent> ... end [architecture] [<nama>] ;`
///   → arsitektur `<nama>` dari entitas `<ent>`.
///
/// Blok yang tak ditutup `end` menghasilkan `VhdlScanError::Unterminated`.
pub fn scan(src: &str) -> Result<VhdlScan, VhdlScanError> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut res = VhdlScan::default();
    let mut i = 0usize;
    while i < n {
        let (ln, t) = &toks[i];
        if let Tok::Word(w) = t {
            match w.to_ascii_lowercase().as_str() {
                "entity" => {
                    if i + 1 >= n {
                        return Err(VhdlScanError::Unterminated("entity".to_string()));
                    }
                    if let Tok::Word(name) = &toks[i + 1].1 {
                        let entity_line = *ln;
                        let ename = name.clone();
                        // Lompat ke `end` terdekat.
                        let mut k = i + 2;
                        let mut found = false;
                        while k < n {
                            if let Tok::Word(w2) = &toks[k].1 {
                                if w2.to_ascii_lowercase() == "end" {
                                    found = true;
                                    break;
                                }
                            }
                            k += 1;
                        }
                        if !found {
                            return Err(VhdlScanError::Unterminated(ename));
                        }
                        res.entities.push(VhdlEntity {
                            name: ename,
                            line: entity_line,
                        });
                        i = k;
                    }
                }
                "architecture" => {
                    // Perlu setidaknya: arch of entity (i+1..i+3).
                    if i + 3 >= n {
                        return Err(VhdlScanError::Unterminated("architecture".to_string()));
                    }
                    if let (Tok::Word(an), Tok::Word(ofw), Tok::Word(en)) =
                        (&toks[i + 1].1, &toks[i + 2].1, &toks[i + 3].1)
                    {
                        if ofw.to_ascii_lowercase() == "of" {
                            let arch_line = *ln;
                            let aname = an.clone();
                            let ename = en.clone();
                            let mut k = i + 4;
                            let mut found = false;
                            while k < n {
                                if let Tok::Word(w2) = &toks[k].1 {
                                    if w2.to_ascii_lowercase() == "end" {
                                        found = true;
                                        break;
                                    }
                                }
                                k += 1;
                            }
                            if !found {
                                return Err(VhdlScanError::Unterminated(aname));
                            }
                            res.architectures.push(VhdlArchitecture {
                                name: aname,
                                entity: ename,
                                line: arch_line,
                            });
                            i = k;
                        }
                    }
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
    fn scan_entity_and_architecture() {
        let src = "\
entity my_ent is
  port (a : in bit; b : out bit);
end my_ent;

architecture rtl of my_ent is
begin
  b <= a;
end rtl;
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.entities,
            vec![VhdlEntity {
                name: "my_ent".to_string(),
                line: 1
            }]
        );
        assert_eq!(
            s.architectures,
            vec![VhdlArchitecture {
                name: "rtl".to_string(),
                entity: "my_ent".to_string(),
                line: 5
            }]
        );
    }

    #[test]
    fn scan_case_insensitive_keywords() {
        let src = "ENTITY FOO IS END FOO;";
        let s = scan(src).unwrap();
        assert_eq!(
            s.entities,
            vec![VhdlEntity {
                name: "FOO".to_string(),
                line: 1
            }]
        );
    }

    #[test]
    fn scan_skips_line_comments() {
        // Entitas di dalam komentar `--` tidak boleh dikenali.
        let src = "-- entity ghost is end ghost;\nentity real is end real;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.entities.len(), 1);
        assert_eq!(s.entities[0].name, "real");
    }

    #[test]
    fn scan_unterminated_entity_errors() {
        let r = scan("entity dangling is\n");
        assert!(matches!(r, Err(VhdlScanError::Unterminated(_))));
    }

    #[test]
    fn scan_two_entities_two_architectures() {
        let src = "\
entity a is end a;
entity b is end b;
architecture xa of a is end xa;
architecture xb of b is end xb;
";
        let s = scan(src).unwrap();
        assert_eq!(s.entities.len(), 2);
        assert_eq!(s.architectures.len(), 2);
        assert_eq!(s.architectures[0].entity, "a");
        assert_eq!(s.architectures[1].entity, "b");
    }
}
