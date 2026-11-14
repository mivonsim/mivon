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

/// Satu sinyal port VHDL (stage 2: ekstraksi kasar dari klausa `port`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlPort {
    pub name: String,
    /// Mode: `in`/`out`/`inout`/`buffer`/`linkage` (teks asli, lower-cased).
    pub mode: String,
    /// Nama tipe (mis. `bit`, `std_logic_vector`).
    pub type_name: String,
}

/// Satu entitas VHDL yang dikenali.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlEntity {
    pub name: String,
    /// 1-based baris tempat kata kunci `entity` muncul.
    pub line: usize,
    /// Daftar port (stage 2; kosong bila entitas tak punya klausa `port`).
    pub ports: Vec<VhdlPort>,
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
/// baca tunggal, sambil melacak nomor baris DAN offset karakter awal token.
/// Komentar `--` diabaikan.
fn tokenize(src: &str) -> Vec<(usize, usize, Tok)> {
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
            out.push((line, start, Tok::Word(word)));
        } else {
            out.push((line, i, Tok::Punct(c)));
            i += 1;
        }
    }
    out
}

/// Scan source VHDL dan kenali entitas + arsitektur.
///
/// Aturan (kasar, case-insensitive):
/// - `entity <nama> [is ...] end [entity] [<nama>] ;` → entitas `<nama>`,
///   beserta port dari klausa `port (...)` (stage 2).
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
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            match w.to_ascii_lowercase().as_str() {
                "entity" => {
                    if i + 1 >= n {
                        return Err(VhdlScanError::Unterminated("entity".to_string()));
                    }
                    if let Tok::Word(name) = &toks[i + 1].2 {
                        let entity_line = *ln;
                        let ename = name.clone();
                        // Offset `is` (i+2) dan `end` (k) untuk ekstrak body.
                        let is_off = toks.get(i + 2).map(|(_, o, _)| *o).unwrap_or(src.len());
                        // Lompat ke `end` terdekat.
                        let mut k = i + 2;
                        let mut found = false;
                        while k < n {
                            if let (_, _, Tok::Word(w2)) = &toks[k] {
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
                        let end_off = toks[k].1;
                        let body = if end_off > is_off {
                            &src[is_off..end_off]
                        } else {
                            ""
                        };
                        let ports = extract_ports(body);
                        res.entities.push(VhdlEntity {
                            name: ename,
                            line: entity_line,
                            ports,
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
                        (&toks[i + 1].2, &toks[i + 2].2, &toks[i + 3].2)
                    {
                        if ofw.to_ascii_lowercase() == "of" {
                            let arch_line = *ln;
                            let aname = an.clone();
                            let ename = en.clone();
                            let mut k = i + 4;
                            let mut found = false;
                            while k < n {
                                if let (_, _, Tok::Word(w2)) = &toks[k] {
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

/// Ekstrak deklarasi port dari body entitas (stage 2, kasar).
///
/// Menemukan klausa `port (...)`, mencocokkan tanda kurung bersarang, lalu
/// tiap segmen yang dipisahkan `;` sebelum `:` adalah daftar nama (bisa
/// beberapa dipisah koma); setelah `:` adalah `mode tipe`. Tipe berikut
/// boleh mengandung kurung (mis. `std_logic_vector(7 downto 0)`), karena
/// name diambil sebelum `:` pertama. Komentar `--` di body diabaikan.
pub fn extract_ports(body: &str) -> Vec<VhdlPort> {
    let stripped = strip_comments(body);
    let low = stripped.to_ascii_lowercase();
    let pos = match low.find("port") {
        Some(p) => p,
        None => return Vec::new(),
    };
    let open = match stripped[pos..].find('(') {
        Some(o) => pos + o,
        None => return Vec::new(),
    };
    // Cocokkan kurung bersarang untuk menemukan `)` penutup klausa.
    let mut depth = 0i32;
    let mut close = None;
    for (idx, c) in stripped[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = match close {
        Some(c) => c,
        None => return Vec::new(),
    };
    let inner = &stripped[open + 1..close];
    let mut ports = Vec::new();
    for seg in inner.split(';') {
        let colon = match seg.find(':') {
            Some(c) => c,
            None => continue,
        };
        let after = seg[colon + 1..].trim();
        // mode = kata pertama; sisanya = tipe.
        let mut parts = after.split_whitespace();
        let mode = parts.next().unwrap_or("").to_ascii_lowercase();
        let type_name = parts.next().unwrap_or("").to_string();
        if mode.is_empty() {
            continue;
        }
        for name in seg[..colon].split(',') {
            let name = name.trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                ports.push(VhdlPort {
                    name: name.to_string(),
                    mode: mode.clone(),
                    type_name: type_name.clone(),
                });
            }
        }
    }
    ports
}

/// Buang komentar baris `--` dari sebuah teks.
fn strip_comments(s: &str) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(ch.len());
    let mut i = 0;
    while i < ch.len() {
        if ch[i] == '-' && i + 1 < ch.len() && ch[i + 1] == '-' {
            while i < ch.len() && ch[i] != '\n' {
                i += 1;
            }
            continue;
        }
        out.push(ch[i]);
        i += 1;
    }
    out
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
                line: 1,
                ports: vec![
                    VhdlPort {
                        name: "a".to_string(),
                        mode: "in".to_string(),
                        type_name: "bit".to_string(),
                    },
                    VhdlPort {
                        name: "b".to_string(),
                        mode: "out".to_string(),
                        type_name: "bit".to_string(),
                    },
                ]
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
                line: 1,
                ports: vec![]
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

    #[test]
    fn scan_ports_multiple_names_and_vector_type() {
        let src = "\
entity my_ent is
  port (
    clk : in bit;
    a, b : out std_logic_vector(7 downto 0);
    en : inout bit
  );
end my_ent;
";
        let s = scan(src).unwrap();
        assert_eq!(s.entities.len(), 1);
        let ports = &s.entities[0].ports;
        // clk, a, b, en -> 4 port.
        assert_eq!(ports.len(), 4, "4 port dikenali: {ports:?}");
        assert_eq!(ports[0].name, "clk");
        assert_eq!(ports[0].mode, "in");
        assert_eq!(ports[0].type_name, "bit");
        // a dan b berbagi mode/out dan tipe vektor.
        assert_eq!(ports[1].name, "a");
        assert_eq!(ports[1].mode, "out");
        assert_eq!(ports[1].type_name, "std_logic_vector(7");
        assert_eq!(ports[2].name, "b");
        assert_eq!(ports[3].name, "en");
        assert_eq!(ports[3].mode, "inout");
    }

    #[test]
    fn extract_ports_no_port_clause_is_empty() {
        // Entitas tanpa klausa port -> ports kosong.
        let body = "is signal x : bit; end";
        assert!(extract_ports(body).is_empty());
    }

    #[test]
    fn extract_ports_skips_comments() {
        // Port di dalam komentar `--` tidak diambil.
        let body = "is port (a : in bit); -- b : out bit end";
        let ports = extract_ports(body);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].name, "a");
    }
}
