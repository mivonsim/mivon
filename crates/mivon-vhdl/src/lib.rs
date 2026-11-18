//! LANG-01: VHDL front-end — STUB stage 1-17 (Critical, open).
//!
//! mivon tidak memiliki dukungan VHDL sama sekali. Stage 1 adalah scanner
//! terbatas yang mengenali batas `entity` / `architecture` dari source VHDL
//! (nama + nomor baris). Stage 2 mengekstrak klausa `port`, stage 3 mengekstrak
//! klausa `generic`, stage 4 mengekstrak deklarasi `signal` di body arsitektur,
//! stage 5 mengekstrak deklarasi `component` (nama + port) dari body arsitektur,
//! stage 6 mengekstrak deklarasi `process` (label + sensitivity list) dari body
//! arsitektur, stage 7 mengekstrak klausa `library`/`use` tingkat file, stage 8
//! mengekstrak deklarasi `package`/`package body` (nama + jenis) tingkat file,
//! stage 9 mengekstrak deklarasi subprogram `function`/`procedure` (nama +
//! jenis), stage 10 mengekstrak deklarasi `type`/`subtype` (nama + jenis
//! Enum/Record/Subtype/Other), stage 11 mengekstrak deklarasi `constant`
//! (nama + tipe + nilai), stage 12 mengekstrak deklarasi `alias`
//! (nama + tipe + target), stage 13 mengekstrak deklarasi `configuration`
//! (nama + entitas), stage 14 mengekstrak deklarasi `variable` (nama + tipe +
//! nilai opsional), stage 15 mengekstrak deklarasi `file` (nama + tipe file +
//! path opsional), stage 16 mengekstrak deklarasi `attribute` (nama + tipe),
//! stage 17 mengekstrak deklarasi `group template` (nama + tipe grup).
//! Semuanya kasar: nama + mode/tipe. BUKAN parser
//! lengkap: belum mem-parse concurrent/sequential statement dalam process,
//! maupun tipe VHDL penuh. Belum di-wire ke pipeline compile (lexer/parser
//! SystemVerilog di mivon-parser).
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

/// Satu generic VHDL (stage 3: ekstraksi kasar dari klausa `generic`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlGeneric {
    pub name: String,
    /// Nama tipe (mis. `integer`, `positive`, `std_logic_vector`).
    pub type_name: String,
}

/// Satu entitas VHDL yang dikenali.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlEntity {
    pub name: String,
    /// 1-based baris tempat kata kunci `entity` muncul.
    pub line: usize,
    /// Daftar generic (stage 3; kosong bila entitas tak punya klausa `generic`).
    pub generics: Vec<VhdlGeneric>,
    /// Daftar port (stage 2; kosong bila entitas tak punya klausa `port`).
    pub ports: Vec<VhdlPort>,
}

/// Satu sinyal VHDL (stage 4: ekstraksi kasar dari body arsitektur).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlSignal {
    pub name: String,
    /// Nama tipe (mis. `bit`, `std_logic_vector`). Tipe dengan kurung
    /// (mis. `std_logic_vector(7 downto 0)`) diambil partial (sampai spasi
    /// pertama) — sama seperti perlakuan port stage 2.
    pub type_name: String,
}

/// Satu komponen VHDL (stage 5: ekstraksi kasar dari `component ... end
/// component`). Port di dalamnya diekstrak dengan `extract_ports` (sama
/// seperti port entitas).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlComponent {
    pub name: String,
    /// 1-based baris tempat kata kunci `component` muncul.
    pub line: usize,
    /// Port komponen (kosong bila tak ada klausa `port`).
    pub ports: Vec<VhdlPort>,
}

/// Satu process VHDL (stage 6: ekstraksi kasar dari `label : process (...)`
/// ... `end process`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlProcess {
    /// Label process (mis. `foo` dalam `foo : process`), atau `None` bila
    /// tak berlabel.
    pub label: Option<String>,
    /// Daftar sinyal dalam sensitivity list. Kosong bila `process` tanpa
    /// `(` ... `)` atau memakai `process (all)` (token `all` tak dimasukkan).
    pub sensitivity: Vec<String>,
    /// 1-based baris tempat kata kunci `process` muncul.
    pub line: usize,
}

/// Jenis deklarasi package VHDL (stage 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VhdlPackageKind {
    /// `package <nama> is ... end package;`
    Decl,
    /// `package body <nama> is ... end package body;`
    Body,
}

/// Satu deklarasi package VHDL (stage 8: ekstraksi kasar nama + jenis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlPackage {
    pub name: String,
    pub kind: VhdlPackageKind,
    /// 1-based baris tempat kata kunci `package` muncul.
    pub line: usize,
}

/// Satu deklarasi `configuration` VHDL (stage 13: ekstraksi kasar nama +
/// entitas). Bentuk: `configuration <nama> of <entity> is ... end
/// configuration [nama];`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlConfiguration {
    pub name: String,
    /// Nama entitas yang dikonfigurasi (word setelah `of`).
    pub entity: String,
    /// 1-based baris tempat kata kunci `configuration` muncul.
    pub line: usize,
}

/// Jenis subprogram VHDL (stage 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VhdlSubprogramKind {
    /// `function`.
    Function,
    /// `procedure`.
    Procedure,
}

/// Satu deklarasi subprogram VHDL (stage 9: ekstraksi kasar nama + jenis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlSubprogram {
    pub name: String,
    pub kind: VhdlSubprogramKind,
    /// 1-based baris tempat kata kunci `function`/`procedure` muncul.
    pub line: usize,
}

/// Jenis deklarasi tipe VHDL (stage 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VhdlTypeKind {
    /// `type <nama> is (...);` (enumerasi).
    Enum,
    /// `type <nama> is record ... end record;`
    Record,
    /// `subtype <nama> is ...;`
    Subtype,
    /// `type <nama> is array (...) of ...;` / `access` / `file` / skalar dll.
    Other,
}

/// Satu deklarasi tipe VHDL (stage 10: ekstraksi kasar nama + jenis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlTypeDecl {
    pub name: String,
    pub kind: VhdlTypeKind,
    /// 1-based baris tempat kata kunci `type`/`subtype` muncul.
    pub line: usize,
}

/// Satu deklarasi `constant` VHDL (stage 11: ekstraksi kasar nama + tipe +
/// nilai). Tipe = word pertama setelah `:`, nilai = word pertama setelah
/// `:=` (literal seperti `'0'` atau `"teks"` diambil isinya secara kasar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlConstant {
    pub name: String,
    pub type_name: String,
    pub value: String,
    /// 1-based baris tempat kata kunci `constant` muncul.
    pub line: usize,
}

/// Satu deklarasi `alias` VHDL (stage 12: ekstraksi kasar nama + tipe +
/// target). Bentuk: `alias <nama> : <tipe> is <target>;`. Tipe = word pertama
/// setelah `:`, target = word pertama setelah `is`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlAlias {
    pub name: String,
    pub type_name: String,
    pub target: String,
    /// 1-based baris tempat kata kunci `alias` muncul.
    pub line: usize,
}

/// Satu deklarasi `variable` VHDL (stage 14: ekstraksi kasar nama + tipe +
/// nilai opsional). Bentuk: `variable <nama> : <tipe> [:= <nilai>];`.
/// Tipe = word pertama setelah `:`, nilai = word pertama setelah `:=`
/// bila ada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlVariable {
    pub name: String,
    pub type_name: String,
    /// Nilai awal opsional (word setelah `:=`), kosong bila tidak ada.
    pub value: String,
    /// 1-based baris tempat kata kunci `variable` muncul.
    pub line: usize,
}

/// Satu deklarasi `file` VHDL (stage 15: ekstraksi kasar nama + tipe file +
/// path opsional). Bentuk: `file <nama> : <tipe_file> [is <path>];`.
/// Tipe = word pertama setelah `:`, path = word pertama setelah `is`
/// bila ada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlFile {
    pub name: String,
    /// Tipe file (mis. `text`, `integer`, tipe akses berkas).
    pub type_name: String,
    /// Path berkas opsional (word setelah `is`), kosong bila tidak ada.
    pub path: String,
    /// 1-based baris tempat kata kunci `file` muncul.
    pub line: usize,
}

/// Satu deklarasi `attribute` VHDL (stage 16: ekstraksi kasar nama + tipe).
/// Bentuk: `attribute <nama> : <tipe> [of <objek> is <nilai>];`. Nama =
/// word pertama setelah `attribute`, tipe = word pertama setelah `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlAttribute {
    pub name: String,
    pub type_name: String,
    /// 1-based baris tempat kata kunci `attribute` muncul.
    pub line: usize,
}

/// Satu deklarasi `group template` VHDL (stage 17: ekstraksi kasar nama +
/// tipe grup). Bentuk: `group <nama> : <tipe_grup> is (daftar);`. Nama =
/// word pertama setelah `group`, tipe = word pertama setelah `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlGroup {
    pub name: String,
    /// Tipe grup (mis. `clock`, `signal`).
    pub type_name: String,
    /// 1-based baris tempat kata kunci `group` muncul.
    pub line: usize,
}

/// Satu arsitektur VHDL yang dikenali.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdlArchitecture {
    pub name: String,
    pub entity: String,
    /// 1-based baris tempat kata kunci `architecture` muncul.
    pub line: usize,
    /// Daftar sinyal (stage 4; kosong bila arsitektur tak punya deklarasi
    /// `signal`).
    pub signals: Vec<VhdlSignal>,
    /// Daftar komponen (stage 5; kosong bila arsitektur tak punya deklarasi
    /// `component`).
    pub components: Vec<VhdlComponent>,
    /// Daftar process (stage 6; kosong bila arsitektur tak punya `process`).
    pub processes: Vec<VhdlProcess>,
}

/// Hasil scan VHDL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VhdlScan {
    pub entities: Vec<VhdlEntity>,
    pub architectures: Vec<VhdlArchitecture>,
    /// Klausa `library <nama>;` tingkat file (stage 7).
    pub libraries: Vec<String>,
    /// Klausa `use <path>;` tingkat file (stage 7).
    pub uses: Vec<String>,
    /// Deklarasi `package` / `package body` (stage 8).
    pub packages: Vec<VhdlPackage>,
    /// Deklarasi subprogram `function`/`procedure` (stage 9, kasar: nama).
    pub subprograms: Vec<VhdlSubprogram>,
    /// Deklarasi `type`/`subtype` (stage 10, kasar: nama + jenis).
    pub type_decls: Vec<VhdlTypeDecl>,
    /// Deklarasi `constant` (stage 11, kasar: nama + tipe + nilai).
    pub constants: Vec<VhdlConstant>,
    /// Deklarasi `alias` (stage 12, kasar: nama + tipe + target).
    pub aliases: Vec<VhdlAlias>,
    /// Deklarasi `configuration` (stage 13, kasar: nama + entitas).
    pub configurations: Vec<VhdlConfiguration>,
    /// Deklarasi `variable` (stage 14, kasar: nama + tipe + nilai opsional).
    pub variables: Vec<VhdlVariable>,
    /// Deklarasi `file` (stage 15, kasar: nama + tipe file + path opsional).
    pub files: Vec<VhdlFile>,
    /// Deklarasi `attribute` (stage 16, kasar: nama + tipe).
    pub attributes: Vec<VhdlAttribute>,
    /// Deklarasi `group template` (stage 17, kasar: nama + tipe grup).
    pub groups: Vec<VhdlGroup>,
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
                        let generics = extract_generics(body);
                        res.entities.push(VhdlEntity {
                            name: ename,
                            line: entity_line,
                            generics,
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
                            // Cari `is` (batas awal body) sejak i+4.
                            let mut is_k = i + 4;
                            while is_k < n {
                                if let (_, _, Tok::Word(w2)) = &toks[is_k] {
                                    if w2.to_ascii_lowercase() == "is" {
                                        break;
                                    }
                                }
                                is_k += 1;
                            }
                            let body_start = if is_k < n {
                                toks[is_k].1 + 2 // lewati "is"
                            } else {
                                toks.get(i + 4).map(|(_, o, _)| *o).unwrap_or(src.len())
                            };
                            let mut k = is_k.max(i + 4);
                            let mut found = false;
                            while k < n {
                                if let (_, _, Tok::Word(w2)) = &toks[k] {
                                    if w2.to_ascii_lowercase() == "end" {
                                        // Lewati `end` bersarang (mis. `end
                                        // component`, `end if`, `end case`).
                                        // Penutup arsitektur adalah `end`
                                        // (diikuti `architecture`/<nama>/`;`).
                                        let nested_kw: [&str; 16] = [
                                            "component",
                                            "if",
                                            "case",
                                            "loop",
                                            "process",
                                            "record",
                                            "generate",
                                            "block",
                                            "for",
                                            "while",
                                            "entity",
                                            "function",
                                            "procedure",
                                            "package",
                                            "configuration",
                                            "array",
                                        ];
                                        let next_is_nested = matches!(
                                            &toks.get(k + 1),
                                            Some((_, _, Tok::Word(nx)))
                                                if nested_kw.contains(
                                                    &nx.to_ascii_lowercase().as_str()
                                                )
                                        );
                                        if next_is_nested {
                                            k += 1;
                                            continue;
                                        }
                                        found = true;
                                        break;
                                    }
                                }
                                k += 1;
                            }
                            if !found {
                                return Err(VhdlScanError::Unterminated(aname));
                            }
                            let end_off = toks[k].1;
                            let body = if end_off > body_start {
                                &src[body_start..end_off]
                            } else {
                                ""
                            };
                            let start_line = if body_start <= src.len() {
                                src[..body_start].lines().count()
                            } else {
                                arch_line
                            };
                            let signals = extract_signals(body);
                            let components = extract_components(body, start_line);
                            let processes = extract_processes(body, start_line);
                            res.architectures.push(VhdlArchitecture {
                                name: aname,
                                entity: ename,
                                line: arch_line,
                                signals,
                                components,
                                processes,
                            });
                            i = k;
                        }
                    }
                }
                "package" => {
                    // `package <nama> is ...` atau `package body <nama> is ...`.
                    let mline = *ln;
                    let mut kind = VhdlPackageKind::Decl;
                    let mut name_idx = i + 1;
                    if let Some((_, _, Tok::Word(nx))) = toks.get(i + 1) {
                        if nx.to_ascii_lowercase() == "body" {
                            kind = VhdlPackageKind::Body;
                            name_idx = i + 2;
                        }
                    }
                    if let Some((_, _, Tok::Word(pn))) = toks.get(name_idx) {
                        res.packages.push(VhdlPackage {
                            name: pn.clone(),
                            kind,
                            line: mline,
                        });
                    }
                    i += 1;
                }
                "function" | "procedure" => {
                    // Nama = word tepat setelah keyword (mis. `function f
                    // return ...` -> "f"; `impure function f` -> "f" juga,
                    // karena keyword yang di-match adalah `function`).
                    // Lewati `end function`/`end procedure` (penutup).
                    let prev_is_end = matches!(
                        toks.get(i.wrapping_sub(1)),
                        Some((_, _, Tok::Word(pw))) if pw.to_ascii_lowercase() == "end"
                    );
                    if prev_is_end {
                        i += 1;
                        continue;
                    }
                    let mline = *ln;
                    let kind = if w.to_ascii_lowercase() == "function" {
                        VhdlSubprogramKind::Function
                    } else {
                        VhdlSubprogramKind::Procedure
                    };
                    if let Some((_, _, Tok::Word(sn))) = toks.get(i + 1) {
                        res.subprograms.push(VhdlSubprogram {
                            name: sn.clone(),
                            kind,
                            line: mline,
                        });
                    }
                    i += 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    let (libraries, uses) = extract_clauses(src);
    res.libraries = libraries;
    res.uses = uses;
    res.type_decls = extract_types(src);
    res.constants = extract_constants(src);
    res.aliases = extract_aliases(src);
    res.configurations = extract_configuration(src);
    res.variables = extract_variable(src);
    res.files = extract_file(src);
    res.attributes = extract_attribute(src);
    res.groups = extract_group(src);
    Ok(res)
}

/// Ekstrak klausa `library` dan `use` tingkat file (stage 7, kasar).
///
/// `library ieee, work;` -> ["ieee", "work"]; `use ieee.std_logic_1164.all;`
/// -> ["ieee.std_logic_1164.all"]. Boundary kata dijaga agar substring
/// (mis. `reuse`, `because`) tak ikut terhitung. Komentar `--` diabaikan.
pub fn extract_clauses(src: &str) -> (Vec<String>, Vec<String>) {
    let stripped = strip_comments(src);
    let low = stripped.to_ascii_lowercase();
    let mut libs = Vec::new();
    let mut uses = Vec::new();
    let boundary_ok = |abs: usize, after_idx: usize| {
        let before_ok = abs == 0
            || !stripped[..abs]
                .chars()
                .last()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        let after_ok = after_idx >= stripped.len()
            || !stripped[after_idx..]
                .chars()
                .next()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        before_ok && after_ok
    };
    // `library <nama>[, <nama>...];`
    let mut pos = 0usize;
    while let Some(p) = low[pos..].find("library") {
        let abs = pos + p;
        let after_idx = abs + "library".len();
        if boundary_ok(abs, after_idx) {
            if let Some(semi) = stripped[after_idx..].find(';') {
                let seg = &stripped[after_idx..after_idx + semi];
                for name in seg.split(',') {
                    let name = name.trim();
                    if !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                    {
                        libs.push(name.to_string());
                    }
                }
                pos = after_idx + semi + 1;
            } else {
                break;
            }
        } else {
            pos = after_idx;
        }
    }
    // `use <path>;`
    let mut pos = 0usize;
    while let Some(p) = low[pos..].find("use") {
        let abs = pos + p;
        let after_idx = abs + "use".len();
        if boundary_ok(abs, after_idx) {
            if let Some(semi) = stripped[after_idx..].find(';') {
                let seg = stripped[after_idx..after_idx + semi].trim();
                if !seg.is_empty() {
                    uses.push(seg.to_string());
                }
                pos = after_idx + semi + 1;
            } else {
                break;
            }
        } else {
            pos = after_idx;
        }
    }
    (libs, uses)
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

/// Ekstrak deklarasi generic dari body entitas (stage 3, kasar).
///
/// Sama seperti `extract_ports` namun untuk klausa `generic (...)`: tiap
/// segmen dipisahkan `;`; nama sebelum `:` (bisa beberapa dipisah koma),
/// tipe sesudah `:`. Generic VHDL tidak punya mode. Tipe berikut boleh
/// mengandung kurung (mis. `std_logic_vector(7 downto 0)`), karena nama
/// diambil sebelum `:` pertama. Default `:= nilai` diabaikan (di luar tipe).
/// Komentar `--` di body diabaikan.
pub fn extract_generics(body: &str) -> Vec<VhdlGeneric> {
    let stripped = strip_comments(body);
    let low = stripped.to_ascii_lowercase();
    let pos = match low.find("generic") {
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
    let mut gens = Vec::new();
    for seg in inner.split(';') {
        let colon = match seg.find(':') {
            Some(c) => c,
            None => continue,
        };
        let after = seg[colon + 1..].trim();
        let type_name = after.split_whitespace().next().unwrap_or("").to_string();
        if type_name.is_empty() {
            continue;
        }
        for name in seg[..colon].split(',') {
            let name = name.trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                gens.push(VhdlGeneric {
                    name: name.to_string(),
                    type_name: type_name.clone(),
                });
            }
        }
    }
    gens
}

/// Ekstrak deklarasi sinyal dari body arsitektur (stage 4, kasar).
///
/// Mencari keyword `signal` (dengan boundary kata utuh), lalu deklarasi
/// `signal <nama> : <tipe> ;` — nama sebelum `:` (bisa beberapa dipisah
/// koma), tipe sesudah `:`. Tipe berikut boleh mengandung kurung (mirip
/// port stage 2). Komentar `--` di body diabaikan. Sinyal dideklarasikan
/// di bagian deklaratif arsitektur (sebelum `begin`); keyword `signal`
/// di luar konteks itu tetap diambil (stub kasar).
pub fn extract_signals(body: &str) -> Vec<VhdlSignal> {
    let is_ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    let stripped = strip_comments(body);
    let low = stripped.to_ascii_lowercase();
    let mut signals = Vec::new();
    let mut pos = 0usize;
    while let Some(p) = low[pos..].find("signal") {
        let abs = pos + p;
        // Boundary kata: karakter sebelum/sepenuhnya bukan alphanumeric/`_`.
        let before_ok = abs == 0
            || !stripped[..abs]
                .chars()
                .last()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        let after_idx = abs + "signal".len();
        let after_ok = after_idx >= stripped.len()
            || !stripped[after_idx..]
                .chars()
                .next()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        if before_ok && after_ok {
            if let Some(semi) = stripped[after_idx..].find(';') {
                let decl = &stripped[after_idx..after_idx + semi];
                if let Some(colon) = decl.find(':') {
                    let type_name = decl[colon + 1..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    for name in decl[..colon].split(',') {
                        let name = name.trim();
                        if is_ident(name) {
                            signals.push(VhdlSignal {
                                name: name.to_string(),
                                type_name: type_name.clone(),
                            });
                        }
                    }
                }
                pos = after_idx + semi + 1;
            } else {
                break;
            }
        } else {
            pos = after_idx;
        }
    }
    signals
}

/// Ekstrak deklarasi komponen dari body arsitektur (stage 5, kasar).
///
/// Mencari keyword `component` (dengan boundary kata utuh), lalu nama
/// komponen (word setelah `component`), lalu klausa `end component`
/// penutup. Port di dalam komponen diekstrak via `extract_ports` (sama
/// seperti port entitas). `start_line` = nomor baris (1-based) karakter
/// pertama `body` — dipakai menghitung baris komponen secara absolut.
pub fn extract_components(body: &str, start_line: usize) -> Vec<VhdlComponent> {
    let stripped = strip_comments(body);
    let low = stripped.to_ascii_lowercase();
    let mut comps = Vec::new();
    let mut pos = 0usize;
    while let Some(p) = low[pos..].find("component") {
        let abs = pos + p;
        let before_ok = abs == 0
            || !stripped[..abs]
                .chars()
                .last()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        let after_idx = abs + "component".len();
        let after_ok = after_idx >= stripped.len()
            || !stripped[after_idx..]
                .chars()
                .next()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        if before_ok && after_ok {
            let cname = stripped[after_idx..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            let name_ok =
                !cname.is_empty() && cname.chars().all(|c| c.is_alphanumeric() || c == '_');
            if name_ok {
                if let Some(end_pos) = low[abs..].find("end component") {
                    let comp_end = abs + end_pos;
                    let inner = &stripped[after_idx..comp_end];
                    let ports = extract_ports(inner);
                    let line = start_line + stripped[..abs].matches('\n').count();
                    comps.push(VhdlComponent {
                        name: cname,
                        line,
                        ports,
                    });
                    pos = comp_end + "end component".len();
                } else {
                    break;
                }
            } else {
                pos = after_idx;
            }
        } else {
            pos = after_idx;
        }
    }
    comps
}

/// Ekstrak deklarasi process dari body arsitektur (stage 6, kasar).
///
/// Mencari keyword `process` (boundary kata utuh). Label diambil dari
/// pola `label : process` (kata tepat sebelum `:` yang diikuti langsung
/// `process`). Sensitivity list diambil dari `(` ... `)` tepat setelah
/// `process` (token `all` maupun kurung kosong menghasilkan list kosong).
/// `start_line` = nomor baris (1-based) karakter pertama `body`.
pub fn extract_processes(body: &str, start_line: usize) -> Vec<VhdlProcess> {
    let stripped = strip_comments(body);
    let low = stripped.to_ascii_lowercase();
    let mut procs = Vec::new();
    let mut pos = 0usize;
    while let Some(p) = low[pos..].find("process") {
        let abs = pos + p;
        let before_ok = abs == 0
            || !stripped[..abs]
                .chars()
                .last()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        let after_idx = abs + "process".len();
        let after_ok = after_idx >= stripped.len()
            || !stripped[after_idx..]
                .chars()
                .next()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        if before_ok && after_ok {
            // Lewati kemunculan `process` yang merupakan penutup
            // (`end process`), bukan pembuka process.
            let prev_word = stripped[..abs]
                .trim_end()
                .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
                .next();
            if prev_word == Some("end") {
                pos = after_idx;
                continue;
            }
            // Label: `label : process` — `:` tepat sebelum process.
            let label = {
                let before = &stripped[..abs];
                let mut lab = None;
                if let Some(colon) = before.rfind(':') {
                    if before[colon + 1..].trim().is_empty() {
                        let pre = before[..colon].trim_end();
                        if let Some(start) = pre.rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                        {
                            let word = &pre[start + 1..];
                            if !word.is_empty() {
                                lab = Some(word.to_string());
                            }
                        } else if !pre.is_empty() {
                            lab = Some(pre.to_string());
                        }
                    }
                }
                lab
            };
            // Sensitivity list: setelah `process`, bila ada `(` ... `)`.
            let mut sensitivity = Vec::new();
            let rest = &stripped[after_idx..];
            if let Some(open) = rest.find('(') {
                let chars: Vec<char> = rest.chars().collect();
                let mut depth = 0i32;
                let mut cur = String::new();
                let mut i = open;
                while i < chars.len() {
                    let c = chars[i];
                    if c == '(' {
                        depth += 1;
                        if depth == 1 {
                            i += 1;
                            continue;
                        }
                    } else if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            if !cur.trim().is_empty() && cur.trim() != "all" {
                                sensitivity.push(cur.trim().to_string());
                            }
                            break;
                        }
                    } else if depth == 1 {
                        if c.is_whitespace() || c == ',' {
                            if !cur.trim().is_empty() && cur.trim() != "all" {
                                sensitivity.push(cur.trim().to_string());
                            }
                            cur.clear();
                        } else {
                            cur.push(c);
                        }
                    }
                    i += 1;
                }
            }
            let line = start_line + stripped[..abs].matches('\n').count();
            procs.push(VhdlProcess {
                label,
                sensitivity,
                line,
            });
            pos = after_idx;
        } else {
            pos = after_idx;
        }
    }
    procs
}

/// Ekstrak deklarasi `type`/`subtype` dari seluruh source (stage 10, kasar).
///
/// Mendeteksi `type <nama> is ...` dan `subtype <nama> is ...`. Jenis
/// ditentukan dari token setelah `is`: `record` -> Record, `(` -> Enum,
/// keyword `subtype` -> Subtype, lainnya -> Other. Penutup `end type`/`end
/// subtype` dilewati (guard kata sebelumnya = `end`). Komentar `--` sudah
/// dibuang oleh `tokenize`.
pub fn extract_types(src: &str) -> Vec<VhdlTypeDecl> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            let lw = w.to_ascii_lowercase();
            if lw == "type" || lw == "subtype" {
                let prev_is_end = matches!(
                    toks.get(i.wrapping_sub(1)),
                    Some((_, _, Tok::Word(pw))) if pw.to_ascii_lowercase() == "end"
                );
                if prev_is_end {
                    i += 1;
                    continue;
                }
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                // Cari `is` setelah nama untuk menentukan jenis.
                let mut is_j = None;
                let mut j = i + 2;
                while j < n {
                    if let (_, _, Tok::Word(wj)) = &toks[j] {
                        if wj.to_ascii_lowercase() == "is" {
                            is_j = Some(j);
                            break;
                        }
                    }
                    j += 1;
                }
                let kind = if lw == "subtype" {
                    VhdlTypeKind::Subtype
                } else if let Some(k) = is_j {
                    match toks.get(k + 1) {
                        Some((_, _, Tok::Word(wk))) if wk.to_ascii_lowercase() == "record" => {
                            VhdlTypeKind::Record
                        }
                        Some((_, _, Tok::Punct('('))) => VhdlTypeKind::Enum,
                        _ => VhdlTypeKind::Other,
                    }
                } else {
                    VhdlTypeKind::Other
                };
                out.push(VhdlTypeDecl {
                    name,
                    kind,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak deklarasi `constant` dari seluruh source (stage 11, kasar).
///
/// Mendeteksi `constant <nama> : <tipe> := <nilai>;`. Tipe = word pertama
/// setelah `:`, nilai = word pertama setelah `:=`. Penutup `end constant`
/// dilewati (guard kata sebelumnya = `end`). Komentar `--` sudah dibuang
/// oleh `tokenize`.
pub fn extract_constants(src: &str) -> Vec<VhdlConstant> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            if w.to_ascii_lowercase() == "constant" {
                let prev_is_end = matches!(
                    toks.get(i.wrapping_sub(1)),
                    Some((_, _, Tok::Word(pw))) if pw.to_ascii_lowercase() == "end"
                );
                if prev_is_end {
                    i += 1;
                    continue;
                }
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                // Tipe: word tepat setelah ':' (bukan ':' dari ':=').
                let mut type_name = String::new();
                let mut j = i + 2;
                while j + 1 < n {
                    if matches!(&toks[j].2, Tok::Punct(':')) {
                        match toks.get(j + 1) {
                            Some((_, _, Tok::Punct('='))) => {
                                // Ini ':=', bukan pemisah tipe — lanjut cari.
                            }
                            Some((_, _, Tok::Word(ty))) => {
                                type_name = ty.clone();
                                break;
                            }
                            _ => {}
                        }
                    }
                    j += 1;
                }
                // Nilai: word setelah ':='; lewati tanda kutip literal
                // (`'0'`, `"teks"`) bila ada.
                let mut value = String::new();
                let mut k = i + 2;
                while k + 1 < n {
                    if matches!(&toks[k].2, Tok::Punct(':'))
                        && matches!(toks.get(k + 1), Some((_, _, Tok::Punct('='))))
                    {
                        let mut v = k + 2;
                        if matches!(toks.get(v), Some((_, _, Tok::Punct(_)))) {
                            v += 1;
                        }
                        if let Some((_, _, Tok::Word(val))) = toks.get(v) {
                            value = val.clone();
                        }
                        break;
                    }
                    k += 1;
                }
                out.push(VhdlConstant {
                    name,
                    type_name,
                    value,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak deklarasi `alias` dari seluruh source (stage 12, kasar).
///
/// Mendeteksi `alias <nama> : <tipe> is <target>;`. Tipe = word pertama
/// setelah `:`, target = word pertama setelah `is`. Guard `end alias`
/// dilewati. Komentar `--` sudah dibuang oleh `tokenize`.
pub fn extract_aliases(src: &str) -> Vec<VhdlAlias> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            if w.to_ascii_lowercase() == "alias" {
                let prev_is_end = matches!(
                    toks.get(i.wrapping_sub(1)),
                    Some((_, _, Tok::Word(pw))) if pw.to_ascii_lowercase() == "end"
                );
                if prev_is_end {
                    i += 1;
                    continue;
                }
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                // Tipe: word tepat setelah ':'.
                let mut type_name = String::new();
                let mut j = i + 2;
                while j + 1 < n {
                    if matches!(&toks[j].2, Tok::Punct(':'))
                        && !matches!(toks.get(j + 1), Some((_, _, Tok::Punct('='))))
                    {
                        if let Some((_, _, Tok::Word(ty))) = toks.get(j + 1) {
                            type_name = ty.clone();
                        }
                        break;
                    }
                    j += 1;
                }
                // Target: word setelah keyword `is`.
                let mut target = String::new();
                let mut k = i + 2;
                while k + 1 < n {
                    if let Some((_, _, Tok::Word(kw))) = toks.get(k) {
                        if kw.to_ascii_lowercase() == "is" {
                            if let Some((_, _, Tok::Word(tg))) = toks.get(k + 1) {
                                target = tg.clone();
                            }
                            break;
                        }
                    }
                    k += 1;
                }
                out.push(VhdlAlias {
                    name,
                    type_name,
                    target,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak deklarasi `configuration` dari seluruh source (stage 13, kasar).
///
/// Mendeteksi `configuration <nama> of <entitas> is ... end configuration [nama];`.
/// Nama = word setelah `configuration`, entitas = word setelah `of`.
/// Penutup `end configuration` dilewati (guard kata sebelumnya = `end`).
/// Komentar `--` sudah dibuang oleh `tokenize`.
pub fn extract_configuration(src: &str) -> Vec<VhdlConfiguration> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            if w.to_ascii_lowercase() == "configuration" {
                let prev_is_end = matches!(
                    toks.get(i.wrapping_sub(1)),
                    Some((_, _, Tok::Word(pw))) if pw.to_ascii_lowercase() == "end"
                );
                if prev_is_end {
                    i += 1;
                    continue;
                }
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                // Entitas: word tepat setelah kata kunci `of`.
                let mut entity = String::new();
                let mut j = i + 2;
                while j + 1 < n {
                    if let Some((_, _, Tok::Word(kw))) = toks.get(j) {
                        if kw.to_ascii_lowercase() == "of" {
                            if let Some((_, _, Tok::Word(ent))) = toks.get(j + 1) {
                                entity = ent.clone();
                            }
                            break;
                        }
                    }
                    j += 1;
                }
                out.push(VhdlConfiguration {
                    name,
                    entity,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak deklarasi `variable` dari seluruh source (stage 14, kasar).
///
/// Mendeteksi `variable <nama> : <tipe> [:= <nilai>];`. Tipe = word pertama
/// setelah `:`, nilai = word pertama setelah `:=` bila ada. Guard `end
/// variable` tidak relevan (variable tak punya penutup `end`), namun keyword
/// `variable` di dalam kata lain tidak terdeteksi karena boundary kata.
/// Komentar `--` sudah dibuang oleh `tokenize`.
pub fn extract_variable(src: &str) -> Vec<VhdlVariable> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            if w.to_ascii_lowercase() == "variable" {
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                // Tipe: word tepat setelah ':' (lewati ':' dari ':=').
                let mut type_name = String::new();
                let mut value = String::new();
                let mut j = i + 2;
                while j + 1 < n {
                    if matches!(&toks[j].2, Tok::Punct(':')) {
                        match toks.get(j + 1) {
                            Some((_, _, Tok::Word(ty))) => {
                                type_name = ty.clone();
                            }
                            _ => {}
                        }
                        break;
                    }
                    j += 1;
                }
                // Nilai opsional: word setelah ':='. Hentikan di ';' (akhir
                // deklarasi) agar tidak menangkap ':= deklarasi berikutnya.
                let mut k = i + 2;
                while k + 1 < n {
                    if matches!(&toks[k].2, Tok::Punct(';')) {
                        break;
                    }
                    if matches!(&toks[k].2, Tok::Punct(':'))
                        && matches!(toks.get(k + 1), Some((_, _, Tok::Punct('='))))
                    {
                        let mut v = k + 2;
                        if matches!(toks.get(v), Some((_, _, Tok::Punct(_)))) {
                            v += 1;
                        }
                        if let Some((_, _, Tok::Word(val))) = toks.get(v) {
                            value = val.clone();
                        }
                        break;
                    }
                    k += 1;
                }
                out.push(VhdlVariable {
                    name,
                    type_name,
                    value,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak deklarasi `file` dari seluruh source (stage 15, kasar).
///
/// Mendeteksi `file <nama> : <tipe_file> [is <path>];`. Nama = word pertama
/// setelah `file`, tipe = word pertama setelah `:`, path = word pertama
/// setelah `is` bila ada. Hentikan di `;` agar tidak menangkap deklarasi
/// berikutnya. Komentar `--` sudah dibuang oleh `tokenize`.
pub fn extract_file(src: &str) -> Vec<VhdlFile> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            if w.to_ascii_lowercase() == "file" {
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                let mut type_name = String::new();
                let mut path = String::new();
                let mut j = i + 2;
                while j + 1 < n {
                    if matches!(&toks[j].2, Tok::Punct(':')) {
                        if let Some((_, _, Tok::Word(ty))) = toks.get(j + 1) {
                            type_name = ty.clone();
                        }
                        break;
                    }
                    j += 1;
                }
                // Path opsional: word setelah 'is', berhenti di ';'.
                let mut k = i + 2;
                while k + 1 < n {
                    if matches!(&toks[k].2, Tok::Punct(';')) {
                        break;
                    }
                    if let Some((_, _, Tok::Word(kw))) = toks.get(k) {
                        if kw.to_ascii_lowercase() == "is" {
                            // Path boleh berupa string literal ("x") — lewati
                            // tanda kutip lalu ambil word isinya.
                            if let Some((_, _, Tok::Word(pt))) = toks.get(k + 1) {
                                path = pt.clone();
                            } else if let Some((_, _, Tok::Punct(_))) = toks.get(k + 1) {
                                if let Some((_, _, Tok::Word(pt))) = toks.get(k + 2) {
                                    path = pt.clone();
                                }
                            }
                            break;
                        }
                    }
                    k += 1;
                }
                out.push(VhdlFile {
                    name,
                    type_name,
                    path,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak deklarasi `attribute` dari seluruh source (stage 16, kasar).
///
/// Mendeteksi `attribute <nama> : <tipe> [of <objek> is <nilai>];`. Nama =
/// word pertama setelah `attribute`, tipe = word pertama setelah `:`.
/// Hentikan di `;` agar tidak menangkap deklarasi berikutnya. Komentar
/// `--` sudah dibuang oleh `tokenize`.
pub fn extract_attribute(src: &str) -> Vec<VhdlAttribute> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            if w.to_ascii_lowercase() == "attribute" {
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                let mut type_name = String::new();
                // Cari ':' pertama setelah nama; word sesudahnya = tipe.
                let mut j = i + 2;
                while j + 1 < n {
                    if matches!(&toks[j].2, Tok::Punct(':')) {
                        if let Some((_, _, Tok::Word(ty))) = toks.get(j + 1) {
                            type_name = ty.clone();
                        }
                        break;
                    }
                    if matches!(&toks[j].2, Tok::Punct(';')) {
                        break;
                    }
                    j += 1;
                }
                out.push(VhdlAttribute {
                    name,
                    type_name,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Ekstrak deklarasi `group template` dari seluruh source (stage 17, kasar).
///
/// Mendeteksi `group <nama> : <tipe_grup> is (daftar);`. Nama = word pertama
/// setelah `group`, tipe = word pertama setelah `:`. Hentikan di `;` agar
/// tidak menangkap deklarasi berikutnya. Komentar `--` sudah dibuang oleh
/// `tokenize`.
pub fn extract_group(src: &str) -> Vec<VhdlGroup> {
    let toks = tokenize(src);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (ln, _, t) = &toks[i];
        if let Tok::Word(w) = t {
            if w.to_ascii_lowercase() == "group" {
                let name = match toks.get(i + 1) {
                    Some((_, _, Tok::Word(nx))) => nx.clone(),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                let mut type_name = String::new();
                // Cari ':' pertama setelah nama; word sesudahnya = tipe grup.
                let mut j = i + 2;
                while j + 1 < n {
                    if matches!(&toks[j].2, Tok::Punct(':')) {
                        if let Some((_, _, Tok::Word(ty))) = toks.get(j + 1) {
                            type_name = ty.clone();
                        }
                        break;
                    }
                    if matches!(&toks[j].2, Tok::Punct(';')) {
                        break;
                    }
                    j += 1;
                }
                out.push(VhdlGroup {
                    name,
                    type_name,
                    line: *ln,
                });
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
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
                generics: vec![],
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
                line: 5,
                signals: vec![],
                components: vec![],
                processes: vec![]
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
                generics: vec![],
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

    #[test]
    fn scan_entity_generic_and_ports() {
        // Entity dengan klausa generic (sebelum port) — keduanya dikenali.
        let src = "\
entity my_ent is
  generic (
    g : integer := 32;
    w : positive
  );
  port (
    clk : in bit;
    d : out std_logic_vector(7 downto 0)
  );
end my_ent;
";
        let s = scan(src).unwrap();
        assert_eq!(s.entities.len(), 1);
        let e = &s.entities[0];
        // Generic: g (integer), w (positive).
        assert_eq!(e.generics.len(), 2, "2 generic dikenali: {e:?}");
        assert_eq!(e.generics[0].name, "g");
        assert_eq!(e.generics[0].type_name, "integer");
        assert_eq!(e.generics[1].name, "w");
        assert_eq!(e.generics[1].type_name, "positive");
        // Port tetap diekstrak walau ada generic di atasnya.
        assert_eq!(e.ports.len(), 2);
        assert_eq!(e.ports[0].name, "clk");
        assert_eq!(e.ports[1].name, "d");
    }

    #[test]
    fn extract_generics_no_generic_is_empty() {
        // Body tanpa klausa generic -> kosong.
        let body = "is port (a : in bit); end";
        assert!(extract_generics(body).is_empty());
    }

    #[test]
    fn extract_generics_multiple_names_and_default() {
        // Nama ganda (dipisah koma) + default := diabaikan.
        let body = "is generic (a, b : integer := 8; c : positive); end";
        let gens = extract_generics(body);
        assert_eq!(gens.len(), 3, "a, b, c -> 3 generic: {gens:?}");
        assert_eq!(gens[0].name, "a");
        assert_eq!(gens[1].name, "b");
        assert_eq!(gens[2].name, "c");
        assert_eq!(gens[0].type_name, "integer");
        assert_eq!(gens[2].type_name, "positive");
    }

    #[test]
    fn scan_architecture_extracts_signals() {
        // Sinyal di body arsitektur (sebelum begin) diekstrak.
        let src = "\
entity my_ent is
  port (clk : in bit);
end my_ent;

architecture rtl of my_ent is
  signal a : bit;
  signal b, c : std_logic_vector(7 downto 0);
begin
  b <= a;
end rtl;
";
        let s = scan(src).unwrap();
        assert_eq!(s.architectures.len(), 1);
        let sigs = &s.architectures[0].signals;
        // a, b, c -> 3 sinyal.
        assert_eq!(sigs.len(), 3, "3 sinyal dikenali: {sigs:?}");
        assert_eq!(sigs[0].name, "a");
        assert_eq!(sigs[0].type_name, "bit");
        assert_eq!(sigs[1].name, "b");
        assert_eq!(sigs[1].type_name, "std_logic_vector(7");
        assert_eq!(sigs[2].name, "c");
        // Entitas tetap terdeteksi dengan 1 port.
        assert_eq!(s.entities.len(), 1);
        assert_eq!(s.entities[0].ports.len(), 1);
    }

    #[test]
    fn extract_signals_no_signal_is_empty() {
        // Body tanpa deklarasi signal -> kosong.
        let body = "is begin b <= a; end";
        assert!(extract_signals(body).is_empty());
    }

    #[test]
    fn scan_architecture_extracts_components() {
        // Komponen di body arsitektur diekstrak (nama + port).
        let src = "\
entity my_ent is
  port (clk : in bit);
end my_ent;

architecture rtl of my_ent is
  component and2 is
    port (a, b : in bit; y : out bit);
  end component;
  component dff is
    port (d, clk : in bit; q : out bit);
  end component;
begin
  u0 : and2 port map (a => x, b => y, y => z);
end rtl;
";
        let s = scan(src).unwrap();
        assert_eq!(s.architectures.len(), 1);
        let comps = &s.architectures[0].components;
        // and2, dff -> 2 komponen.
        assert_eq!(comps.len(), 2, "2 komponen dikenali: {comps:?}");
        assert_eq!(comps[0].name, "and2");
        assert_eq!(comps[0].ports.len(), 3); // a, b, y
        assert_eq!(comps[0].ports[2].name, "y");
        assert_eq!(comps[0].ports[2].mode, "out");
        assert_eq!(comps[1].name, "dff");
        assert_eq!(comps[1].ports.len(), 3); // d, clk, q
        assert!(s.architectures[0].signals.is_empty());
        assert_eq!(s.entities.len(), 1);
        assert_eq!(s.entities[0].ports.len(), 1);
    }

    #[test]
    fn extract_components_no_component_is_empty() {
        // Body tanpa deklarasi component -> kosong.
        let body = "is begin b <= a; end";
        assert!(extract_components(body, 1).is_empty());
    }

    #[test]
    fn scan_architecture_extracts_processes() {
        // Process di body arsitektur diekstrak (label + sensitivity).
        let src = "\
entity my_ent is
  port (clk : in bit);
end my_ent;

architecture rtl of my_ent is
  signal a : bit;
begin
  p0 : process (clk) is
  begin
    a <= '0';
  end process p0;
  p1 : process is
  begin
    a <= '1';
  end process p1;
end rtl;
";
        let s = scan(src).unwrap();
        assert_eq!(s.architectures.len(), 1);
        let procs = &s.architectures[0].processes;
        assert_eq!(procs.len(), 2, "2 process dikenali: {procs:?}");
        assert_eq!(procs[0].label.as_deref(), Some("p0"));
        assert_eq!(procs[0].sensitivity, vec!["clk".to_string()]);
        assert_eq!(procs[1].label.as_deref(), Some("p1"));
        // process tanpa `(` ... `)` -> sensitivity kosong.
        assert!(procs[1].sensitivity.is_empty());
        // Sinyal tetap diekstrak (a), entitas tetap 1 port.
        assert_eq!(s.architectures[0].signals.len(), 1);
        assert_eq!(s.entities[0].ports.len(), 1);
    }

    #[test]
    fn extract_processes_sensitivity_list() {
        // Sensitivity list multi-sinyal + `all` (tak dimasukkan).
        let body = "\
begin
  foo : process (clk, rst, en) is
  begin null; end process;
  bar : process (all) is
  begin null; end process;
end";
        let procs = extract_processes(body, 1);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].label.as_deref(), Some("foo"));
        assert_eq!(
            procs[0].sensitivity,
            vec!["clk".to_string(), "rst".to_string(), "en".to_string()]
        );
        assert_eq!(procs[1].label.as_deref(), Some("bar"));
        // `all` tak dimasukkan -> kosong.
        assert!(procs[1].sensitivity.is_empty());
    }

    #[test]
    fn scan_extracts_library_and_use_clauses() {
        // Klausa library/use tingkat file diekstrak.
        let src = "\
library ieee;
use ieee.std_logic_1164.all;
use work.my_pkg.all;

entity my_ent is
end my_ent;
";
        let s = scan(src).unwrap();
        assert_eq!(s.libraries, vec!["ieee".to_string()]);
        assert_eq!(
            s.uses,
            vec![
                "ieee.std_logic_1164.all".to_string(),
                "work.my_pkg.all".to_string()
            ]
        );
    }

    #[test]
    fn extract_clauses_substring_not_counted() {
        // Substring `use`/`library` di dalam kata lain tak terhitung.
        let src = "\
library ieee;
-- use ini komentar, diabaikan
reuse_this not_a_clause;
entity e is end e;
";
        let (libs, uses) = extract_clauses(src);
        assert_eq!(libs, vec!["ieee".to_string()]);
        // `reuse_this` (substring) dan komentar `-- use` tak terhitung.
        assert!(uses.is_empty());
    }

    #[test]
    fn scan_extracts_packages() {
        // Deklarasi `package` dan `package body` terdeteksi (nama + jenis).
        let src = "\
package my_pkg is
  type t is (a, b);
end package;

package body my_pkg is
  function f return integer is begin return 0; end;
end package body;

entity e is end e;
";
        let s = scan(src).unwrap();
        assert_eq!(s.packages.len(), 2);
        assert_eq!(s.packages[0].name, "my_pkg");
        assert_eq!(s.packages[0].kind, VhdlPackageKind::Decl);
        assert_eq!(s.packages[1].name, "my_pkg");
        assert_eq!(s.packages[1].kind, VhdlPackageKind::Body);
        // Entitas tetap terdeteksi (terpisah dari package).
        assert_eq!(s.entities.len(), 1);
    }

    #[test]
    fn scan_package_case_insensitive() {
        // Keyword `PACKAGE` (uppercase) tetap dikenali (case-insensitive).
        let src = "PACKAGE up is end PACKAGE;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.packages.len(), 1);
        assert_eq!(s.packages[0].name, "up");
        assert_eq!(s.packages[0].kind, VhdlPackageKind::Decl);
    }

    #[test]
    fn scan_extracts_subprograms() {
        // Subprogram `function`/`procedure` terdeteksi (nama + jenis).
        let src = "\
package p is
  function add return integer;
  procedure run;
end package;

function top_f return boolean is begin return true; end function;
";
        let s = scan(src).unwrap();
        // package p: function add + procedure run.
        // top-level: function top_f.
        assert_eq!(s.subprograms.len(), 3, "3 subprogram: {s:?}");
        assert_eq!(s.subprograms[0].name, "add");
        assert_eq!(s.subprograms[0].kind, VhdlSubprogramKind::Function);
        assert_eq!(s.subprograms[1].name, "run");
        assert_eq!(s.subprograms[1].kind, VhdlSubprogramKind::Procedure);
        assert_eq!(s.subprograms[2].name, "top_f");
        assert_eq!(s.subprograms[2].kind, VhdlSubprogramKind::Function);
        // `end function top_f` tidak doble-count (penutup di-skip).
        assert_eq!(s.packages.len(), 1);
    }

    #[test]
    fn scan_subprogram_end_not_double_counted() {
        // `end procedure p` (penutup) tidak dihitung sebagai subprogram.
        let src = "\
procedure p is begin null; end procedure p;
";
        let s = scan(src).unwrap();
        assert_eq!(s.subprograms.len(), 1);
        assert_eq!(s.subprograms[0].name, "p");
        assert_eq!(s.subprograms[0].kind, VhdlSubprogramKind::Procedure);
    }

    #[test]
    fn scan_extracts_type_declarations() {
        // `type`/`subtype` terdeteksi (nama + jenis benar).
        let src = "\
package p is
  type state_t is (idle, busy, done);
  type byte_t is array (7 downto 0) of bit;
  type reg_t is record f1 : bit; f2 : bit; end record;
  subtype small_int is integer range 0 to 7;
end package;
";
        let s = scan(src).unwrap();
        // state_t(Enum), byte_t(Other), reg_t(Record), small_int(Subtype).
        assert_eq!(s.type_decls.len(), 4, "4 tipe dikenali: {s:?}");
        assert_eq!(s.type_decls[0].name, "state_t");
        assert_eq!(s.type_decls[0].kind, VhdlTypeKind::Enum);
        assert_eq!(s.type_decls[1].name, "byte_t");
        assert_eq!(s.type_decls[1].kind, VhdlTypeKind::Other);
        assert_eq!(s.type_decls[2].name, "reg_t");
        assert_eq!(s.type_decls[2].kind, VhdlTypeKind::Record);
        assert_eq!(s.type_decls[3].name, "small_int");
        assert_eq!(s.type_decls[3].kind, VhdlTypeKind::Subtype);
        // Package tetap terdeteksi (1).
        assert_eq!(s.packages.len(), 1);
    }

    #[test]
    fn scan_type_end_not_double_counted() {
        // Penutup `end type`/`end subtype` tidak dihitung sebagai deklarasi.
        // Tanpa guard akan menjadi 4 (doble-count).
        let src = "\
type e is (a, b); end type e;
subtype s is integer range 0 to 3; end subtype s;
";
        let s = scan(src).unwrap();
        assert_eq!(s.type_decls.len(), 2, "1 type + 1 subtype: {s:?}");
        assert_eq!(s.type_decls[0].name, "e");
        assert_eq!(s.type_decls[0].kind, VhdlTypeKind::Enum);
        assert_eq!(s.type_decls[1].name, "s");
        assert_eq!(s.type_decls[1].kind, VhdlTypeKind::Subtype);
    }

    #[test]
    fn scan_extracts_constants() {
        // Deklarasi `constant` diekstrak (nama + tipe + nilai + baris).
        let src = "\
constant WIDTH : integer := 8;
constant RESET : bit := '0';
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.constants,
            vec![
                VhdlConstant {
                    name: "WIDTH".to_string(),
                    type_name: "integer".to_string(),
                    value: "8".to_string(),
                    line: 1,
                },
                VhdlConstant {
                    name: "RESET".to_string(),
                    type_name: "bit".to_string(),
                    value: "0".to_string(),
                    line: 2,
                },
            ]
        );
    }

    #[test]
    fn scan_constant_case_insensitive() {
        // VHDL case-insensitive: keyword uppercase tetap dikenali.
        let src = "CONSTANT WIDTH : INTEGER := 8;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.constants.len(), 1, "{s:?}");
        assert_eq!(s.constants[0].name, "WIDTH");
        assert_eq!(s.constants[0].type_name, "INTEGER");
        assert_eq!(s.constants[0].value, "8");
    }

    #[test]
    fn scan_extracts_aliases() {
        // Deklarasi `alias` diekstrak (nama + tipe + target + baris).
        let src = "\
alias data_out : std_logic_vector is internal_data;
alias clk_alias : bit is clk;
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.aliases,
            vec![
                VhdlAlias {
                    name: "data_out".to_string(),
                    type_name: "std_logic_vector".to_string(),
                    target: "internal_data".to_string(),
                    line: 1,
                },
                VhdlAlias {
                    name: "clk_alias".to_string(),
                    type_name: "bit".to_string(),
                    target: "clk".to_string(),
                    line: 2,
                },
            ]
        );
    }

    #[test]
    fn scan_alias_case_insensitive() {
        // VHDL case-insensitive: keyword uppercase tetap dikenali.
        let src = "ALIAS DATA : BIT IS SIG;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.aliases.len(), 1, "{s:?}");
        assert_eq!(s.aliases[0].name, "DATA");
        assert_eq!(s.aliases[0].type_name, "BIT");
        assert_eq!(s.aliases[0].target, "SIG");
    }

    #[test]
    fn scan_extracts_configurations() {
        // Deklarasi `configuration` diekstrak (nama + entitas + baris).
        let src = "\
configuration cfg1 of my_ent is
end configuration;
configuration cfg2 of other_ent is
end configuration cfg2;
";
        let s = scan(src).unwrap();
        assert_eq!(s.configurations.len(), 2, "{s:?}");
        assert_eq!(s.configurations[0].name, "cfg1");
        assert_eq!(s.configurations[0].entity, "my_ent");
        assert_eq!(s.configurations[0].line, 1);
        assert_eq!(s.configurations[1].name, "cfg2");
        assert_eq!(s.configurations[1].entity, "other_ent");
        assert_eq!(s.configurations[1].line, 3);
    }

    #[test]
    fn scan_configuration_case_insensitive() {
        // VHDL case-insensitive: keyword uppercase tetap dikenali.
        let src = "CONFIGURATION C OF E IS END CONFIGURATION;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.configurations.len(), 1, "{s:?}");
        assert_eq!(s.configurations[0].name, "C");
        assert_eq!(s.configurations[0].entity, "E");
        assert_eq!(s.configurations[0].line, 1);
    }

    #[test]
    fn extract_configuration_no_configuration_is_empty() {
        // Source tanpa `configuration` -> kosong.
        assert!(extract_configuration("entity e is end e;").is_empty());
    }

    #[test]
    fn scan_extracts_variables() {
        // Deklarasi `variable` diekstrak (nama + tipe + nilai opsional + baris).
        let src = "\
variable cnt : integer := 0;
variable flag : bit;
variable idx : natural := 7;
";
        let s = scan(src).unwrap();
        assert_eq!(s.variables.len(), 3, "{s:?}");
        assert_eq!(s.variables[0].name, "cnt");
        assert_eq!(s.variables[0].type_name, "integer");
        assert_eq!(s.variables[0].value, "0");
        assert_eq!(s.variables[0].line, 1);
        assert_eq!(s.variables[1].name, "flag");
        assert_eq!(s.variables[1].type_name, "bit");
        assert!(s.variables[1].value.is_empty());
        assert_eq!(s.variables[1].line, 2);
        assert_eq!(s.variables[2].name, "idx");
        assert_eq!(s.variables[2].type_name, "natural");
        assert_eq!(s.variables[2].value, "7");
        assert_eq!(s.variables[2].line, 3);
    }

    #[test]
    fn scan_variable_case_insensitive() {
        // VHDL case-insensitive: keyword uppercase tetap dikenali.
        let src = "VARIABLE X : BOOLEAN := TRUE;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.variables.len(), 1, "{s:?}");
        assert_eq!(s.variables[0].name, "X");
        assert_eq!(s.variables[0].type_name, "BOOLEAN");
        assert_eq!(s.variables[0].value, "TRUE");
    }

    #[test]
    fn extract_variable_no_variable_is_empty() {
        // Source tanpa `variable` -> kosong.
        assert!(extract_variable("signal a : bit;").is_empty());
    }

    #[test]
    fn scan_extracts_files() {
        // Deklarasi `file` diekstrak (nama + tipe file + path opsional + baris).
        let src = "\
file infile : text open read_mode is \"input.txt\";
file outfile : text;
file mem : ram_type is \"mem.dat\";
";
        let s = scan(src).unwrap();
        assert_eq!(s.files.len(), 3, "{s:?}");
        assert_eq!(s.files[0].name, "infile");
        assert_eq!(s.files[0].type_name, "text");
        // Path dalam string literal: tokenisasi potong di '.', ambil kata
        // pertama ("input" dari "input.txt").
        assert_eq!(s.files[0].path, "input");
        assert_eq!(s.files[0].line, 1);
        assert_eq!(s.files[1].name, "outfile");
        assert_eq!(s.files[1].type_name, "text");
        assert!(s.files[1].path.is_empty());
        assert_eq!(s.files[1].line, 2);
        assert_eq!(s.files[2].name, "mem");
        assert_eq!(s.files[2].type_name, "ram_type");
        assert_eq!(s.files[2].path, "mem");
        assert_eq!(s.files[2].line, 3);
    }

    #[test]
    fn scan_file_case_insensitive() {
        // VHDL case-insensitive: keyword uppercase tetap dikenali.
        let src = "FILE F : INTEGER;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.files.len(), 1, "{s:?}");
        assert_eq!(s.files[0].name, "F");
        assert_eq!(s.files[0].type_name, "INTEGER");
    }

    #[test]
    fn extract_file_no_file_is_empty() {
        // Source tanpa `file` -> kosong.
        assert!(extract_file("signal a : bit;").is_empty());
    }

    #[test]
    fn scan_extracts_attributes() {
        // Deklarasi `attribute` diekstrak (nama + tipe + baris).
        let src = "\
attribute keep : boolean;
attribute syn_preserve : boolean of ff : register;
attribute x : integer;
";
        let s = scan(src).unwrap();
        assert_eq!(s.attributes.len(), 3, "{s:?}");
        assert_eq!(s.attributes[0].name, "keep");
        assert_eq!(s.attributes[0].type_name, "boolean");
        assert_eq!(s.attributes[0].line, 1);
        assert_eq!(s.attributes[1].name, "syn_preserve");
        assert_eq!(s.attributes[1].type_name, "boolean");
        assert_eq!(s.attributes[1].line, 2);
        assert_eq!(s.attributes[2].name, "x");
        assert_eq!(s.attributes[2].type_name, "integer");
        assert_eq!(s.attributes[2].line, 3);
    }

    #[test]
    fn scan_attribute_case_insensitive() {
        // VHDL case-insensitive: keyword uppercase tetap dikenali.
        let src = "ATTRIBUTE Y : STRING;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.attributes.len(), 1, "{s:?}");
        assert_eq!(s.attributes[0].name, "Y");
        assert_eq!(s.attributes[0].type_name, "STRING");
    }

    #[test]
    fn extract_attribute_no_attribute_is_empty() {
        // Source tanpa `attribute` -> kosong.
        assert!(extract_attribute("signal a : bit;").is_empty());
    }

    #[test]
    fn scan_extracts_groups() {
        // Deklarasi `group template` diekstrak (nama + tipe grup + baris).
        let src = "\
group clk_grp : clock is (clk1, clk2);
group sig_grp : signal is (a, b);
group x : integer;
";
        let s = scan(src).unwrap();
        assert_eq!(s.groups.len(), 3, "{s:?}");
        assert_eq!(s.groups[0].name, "clk_grp");
        assert_eq!(s.groups[0].type_name, "clock");
        assert_eq!(s.groups[0].line, 1);
        assert_eq!(s.groups[1].name, "sig_grp");
        assert_eq!(s.groups[1].type_name, "signal");
        assert_eq!(s.groups[1].line, 2);
        assert_eq!(s.groups[2].name, "x");
        assert_eq!(s.groups[2].type_name, "integer");
        assert_eq!(s.groups[2].line, 3);
    }

    #[test]
    fn scan_group_case_insensitive() {
        // VHDL case-insensitive: keyword uppercase tetap dikenali.
        let src = "GROUP Y : BUS;\n";
        let s = scan(src).unwrap();
        assert_eq!(s.groups.len(), 1, "{s:?}");
        assert_eq!(s.groups[0].name, "Y");
        assert_eq!(s.groups[0].type_name, "BUS");
    }

    #[test]
    fn extract_group_no_group_is_empty() {
        // Source tanpa `group` -> kosong.
        assert!(extract_group("signal a : bit;").is_empty());
    }
}
