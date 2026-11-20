//! LANG-02: SystemC front-end — STUB stage 1-11 (Critical, open).
//!
//! mivon tidak memiliki dukungan SystemC sama sekali. Stage 1 adalah scanner
//! terbatas yang mengenali makro `SC_MODULE(<nama>)` dan `SC_CTOR(<nama>)`
//! dari source SystemC (C++). Stage 2 menambah ekstraksi deklarasi
//! port/signal `sc_in`/`sc_out`/`sc_inout`/`sc_signal` (nama + arah + tipe).
//! Stage 3 menambah ekstraksi process `SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`.
//! Stage 4 menambah ekstraksi deklarasi clock `sc_clock` (nama). Stage 5
//! menambah deteksi kontrol simulasi `sc_start`/`sc_stop`. Stage 6 menambah
//! deteksi entry point `sc_main`. Stage 7 menambah ekstraksi deklarasi
//! channel/sinkronisasi `sc_fifo`/`sc_event`/`sc_mutex`/`sc_semaphore`
//! (nama + jenis). Stage 8 menambah ekstraksi sensitivity list
//! `sensitive << clk.pos()` (sinyal + edge). Stage 9 menambah deteksi
//! `wait()` / `wait(<durasi>, <satuan>)` (durasi + satuan). Stage 10
//! menambah deteksi makro pelaporan `SC_REPORT_INFO`/`SC_REPORT_WARNING`/
//! `SC_REPORT_ERROR`/`SC_REPORT_FATAL` (jenis + baris). Stage 11 menambah
//! deteksi pemanggilan `sc_trace` (baris). BUKAN parser
//! C++/SystemC lengkap: belum mem-parse body process/ctor, maupun tipe SystemC
//! bersarang penuh. Belum
//! di-wire ke pipeline compile mivon (SystemVerilog).
//!
//! Ini titik awal yang kecil namun ter-test; item besar LANG-02 akan
//! dipecah bertahap (parser C++/SystemC, elaborasi, simulasi) di stage
//! berikutnya.

/// Satu modul SystemC yang dikenali (`SC_MODULE`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCModule {
    pub name: String,
    /// 1-based baris tempat makro `SC_MODULE` muncul.
    pub line: usize,
}

/// Satu konstruktor SystemC yang dikenali (`SC_CTOR`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCCtor {
    pub name: String,
    /// 1-based baris tempat makro `SC_CTOR` muncul.
    pub line: usize,
}

/// Hasil scan SystemC.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SystemCScan {
    pub modules: Vec<SystemCModule>,
    pub ctors: Vec<SystemCCtor>,
    /// Deklarasi port/signal (`sc_in`/`sc_out`/`sc_inout`/`sc_signal`).
    pub ports: Vec<SystemCPort>,
    /// Deklarasi process (`SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`); stage 3.
    pub processes: Vec<SystemCProcess>,
    /// Deklarasi clock (`sc_clock`); stage 4.
    pub clocks: Vec<SystemCClock>,
    /// Kontrol simulasi (`sc_start`/`sc_stop`); stage 5.
    pub sim_control: Vec<SystemCSimControl>,
    /// Entry point `sc_main` (`int sc_main(...)`); stage 6. `None` bila
    /// source tak punya `sc_main` (biasanya modul/header, bukan testbench).
    pub sc_main: Option<usize>,
    /// Deklarasi channel/sinkronisasi (`sc_fifo`/`sc_event`/`sc_mutex`/
    /// `sc_semaphore`); stage 7.
    pub channels: Vec<SystemCChannel>,
    /// Sensitivity list process (`sensitive << clk.pos()`); stage 8.
    pub sensitivities: Vec<SystemCSensitivity>,
    /// Pemanggilan `wait()` / `wait(<durasi>, <satuan>)`; stage 9.
    pub waits: Vec<SystemCWait>,
    /// Pemanggilan makro pelaporan `SC_REPORT_*`; stage 10.
    pub reports: Vec<SystemCReport>,
    /// Pemanggilan `sc_trace`; stage 11.
    pub traces: Vec<SystemCTrace>,
}

/// Jenis process SystemC.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SystemCProcessKind {
    /// `SC_METHOD`.
    Method,
    /// `SC_THREAD`.
    Thread,
    /// `SC_CTHREAD`.
    CThread,
}

/// Satu deklarasi process SystemC (`SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCProcess {
    pub kind: SystemCProcessKind,
    /// Nama metode/thread yang dijadikan process (identifier tepat setelah
    /// keyword, mis. `SC_METHOD(run)` -> "run").
    pub name: String,
    /// 1-based baris tempat keyword muncul.
    pub line: usize,
}

/// Satu deklarasi clock SystemC (`sc_clock`, stage 4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCClock {
    /// Nama sinyal clock (identifier tepat setelah keyword `sc_clock`).
    pub name: String,
    /// 1-based baris tempat keyword muncul.
    pub line: usize,
}

/// Jenis kontrol simulasi SystemC (`sc_start`/`sc_stop`, stage 5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SystemCSimControlKind {
    /// `sc_start(<durasi>)` — memulai simulasi.
    Start,
    /// `sc_stop()` — menghentikan simulasi.
    Stop,
}

/// Satu pemanggilan kontrol simulasi SystemC (`sc_start`/`sc_stop`, stage 5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCSimControl {
    pub kind: SystemCSimControlKind,
    /// 1-based baris tempat keyword muncul.
    pub line: usize,
}

/// Jenis channel/sinkronisasi SystemC (stage 7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SystemCChannelKind {
    /// `sc_fifo<T>` — channel FIFO (template tipe).
    Fifo,
    /// `sc_event` — event sinkronisasi.
    Event,
    /// `sc_mutex` — mutex.
    Mutex,
    /// `sc_semaphore` — semaphore.
    Semaphore,
}

/// Satu deklarasi channel/sinkronisasi SystemC (stage 7: ekstraksi kasar
/// nama + jenis). Nama = identifier setelah keyword (untuk `sc_fifo`,
/// setelah `>` penutup template).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCChannel {
    pub name: String,
    pub kind: SystemCChannelKind,
    /// 1-based baris tempat keyword muncul.
    pub line: usize,
}

/// Edge sensitivity SystemC (stage 8).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SystemCEdge {
    /// `.pos()` — edge naik.
    Pos,
    /// `.neg()` — edge turun.
    Neg,
    /// Tanpa `.pos()`/`.neg()` — sensitif pada perubahan nilai.
    None,
}

/// Satu entri sensitivity list SystemC
/// (`sensitive << clk.pos() << rst.neg();`, stage 8).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCSensitivity {
    /// Nama sinyal setelah `<<`.
    pub signal: String,
    /// Edge dari `.pos()` / `.neg()`, bila ada.
    pub edge: SystemCEdge,
    /// 1-based baris tempat sinyal muncul.
    pub line: usize,
}

/// Satu pemanggilan `wait()` SystemC (stage 9: deteksi kasar durasi +
/// satuan). `wait()` tanpa argumen menyimpan `None` untuk keduanya.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCWait {
    /// Word pertama di dalam `wait(...)` (mis. `"10"`), `None` bila `wait()`.
    pub amount: Option<String>,
    /// Word setelah koma (mis. `"SC_NS"`), `None` bila tak ada.
    pub unit: Option<String>,
    /// 1-based baris tempat keyword `wait` muncul.
    pub line: usize,
}

/// Jenis makro pelaporan SystemC (stage 10).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SystemCReportKind {
    /// `SC_REPORT_INFO`.
    Info,
    /// `SC_REPORT_WARNING`.
    Warning,
    /// `SC_REPORT_ERROR`.
    Error,
    /// `SC_REPORT_FATAL`.
    Fatal,
}

/// Satu pemanggilan makro pelaporan SystemC (stage 10: jenis + baris).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCReport {
    pub kind: SystemCReportKind,
    /// 1-based baris tempat makro muncul.
    pub line: usize,
}

/// Satu pemanggilan `sc_trace` SystemC (stage 11: baris).
/// Bentuk: `sc_trace(tf, sinyal, "nama");`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SystemCTrace {
    /// 1-based baris tempat `sc_trace` muncul.
    pub line: usize,
}

/// Arah deklarasi port/signal SystemC.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

impl std::fmt::Display for SystemCPortDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SystemCPortDirection::In => write!(f, "sc_in"),
            SystemCPortDirection::Out => write!(f, "sc_out"),
            SystemCPortDirection::InOut => write!(f, "sc_inout"),
            SystemCPortDirection::Signal => write!(f, "sc_signal"),
        }
    }
}

/// Satu deklarasi port/signal SystemC (`sc_in`/`sc_out`/`sc_inout`/`sc_signal`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
enum Tok {
    Word(String),
    Punct(char),
}

/// Scan source SystemC dan kenali makro `SC_MODULE(<nama>)` dan
/// `SC_CTOR(<nama>)`, deklarasi port/signal `sc_in`/`sc_out`/`sc_inout`/
/// `sc_signal`, process `SC_METHOD`/`SC_THREAD`/`SC_CTHREAD`, clock
/// `sc_clock`, serta kontrol simulasi `sc_start`/`sc_stop`.
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
/// - Clock: setelah keyword `sc_clock`, identifier tepat berikutnya adalah
///   nama clock (mis. `sc_clock clk;` atau `sc_clock clk("clk", 10, SC_NS);`).
/// - Kontrol simulasi: keyword `sc_start`/`sc_stop` dihitung sebagai panggilan
///   kontrol (jenis + baris).
/// - Entry point: keyword `sc_main` menandai baris entry point (hanya diambil
///   yang pertama; testbench SystemC wajib punya `sc_main`).
/// - Sensitivity: keyword `sensitive` diikuti `<< <sinyal>` (bisa beberapa,
///   dipisah `<<` lagi) sampai `;`; edge diambil dari `.pos()` / `.neg()`.
/// - Wait: keyword `wait` diikuti `(...)`; word pertama = durasi, word
///   setelah koma = satuan (mis. `SC_NS`). `wait()` kosong = keduanya None.
/// - Makro `SC_REPORT_INFO`/`SC_REPORT_WARNING`/`SC_REPORT_ERROR`/
///   `SC_REPORT_FATAL` dihitung sebagai laporan (jenis + baris).
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
                "sc_clock" => {
                    // Nama = identifier tepat setelah keyword `sc_clock`
                    // (mis. `sc_clock clk;` atau `sc_clock clk("clk", 10,
                    // SC_NS);`).
                    if let Some((mline, Tok::Word(cn))) = toks.get(i + 1) {
                        res.clocks.push(SystemCClock {
                            name: cn.clone(),
                            line: *mline,
                        });
                    }
                    i += 1;
                }
                "sc_start" => {
                    res.sim_control.push(SystemCSimControl {
                        kind: SystemCSimControlKind::Start,
                        line: *ln,
                    });
                    i += 1;
                }
                "sc_stop" => {
                    res.sim_control.push(SystemCSimControl {
                        kind: SystemCSimControlKind::Stop,
                        line: *ln,
                    });
                    i += 1;
                }
                "sc_main" => {
                    // Entry point `int sc_main(int argc, char* argv[])`.
                    if res.sc_main.is_none() {
                        res.sc_main = Some(*ln);
                    }
                    i += 1;
                }
                "sc_fifo" | "sc_event" | "sc_mutex" | "sc_semaphore" => {
                    // Jenis channel dari keyword (case-sensitive).
                    let kind = match w.as_str() {
                        "sc_fifo" => SystemCChannelKind::Fifo,
                        "sc_event" => SystemCChannelKind::Event,
                        "sc_mutex" => SystemCChannelKind::Mutex,
                        _ => SystemCChannelKind::Semaphore,
                    };
                    let mline = *ln;
                    // Nama: bila ada template `<...>` (sc_fifo), ambil word
                    // setelah `>` penutup; selain itu word tepat setelah
                    // keyword (seperti `sc_clock`).
                    let mut name = String::new();
                    let mut k = i + 1;
                    if let Some((_, Tok::Punct('<'))) = toks.get(k) {
                        let mut depth = 0i32;
                        let mut end = k;
                        let mut kk = k;
                        while kk < n {
                            match &toks[kk] {
                                (_, Tok::Punct('<')) => depth += 1,
                                (_, Tok::Punct('>')) => {
                                    depth -= 1;
                                    if depth == 0 {
                                        end = kk;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            kk += 1;
                        }
                        if let Some((_, Tok::Word(cn))) = toks.get(end + 1) {
                            name = cn.clone();
                            k = end + 1;
                        } else {
                            k = end + 1;
                        }
                    } else if let Some((_, Tok::Word(cn))) = toks.get(k) {
                        name = cn.clone();
                        k = k + 1;
                    }
                    if !name.is_empty() {
                        res.channels.push(SystemCChannel {
                            name,
                            kind,
                            line: mline,
                        });
                    }
                    i = k;
                }
                "sensitive" => {
                    // Batasi pencarian sampai `;` akhir statement agar `<<`
                    // di tempat lain tidak ikut terhitung.
                    let mut end = i + 1;
                    while end < n && !matches!(&toks[end].1, Tok::Punct(';')) {
                        end += 1;
                    }
                    let mut k = i + 1;
                    while k + 2 < end {
                        let is_shift = matches!(toks.get(k), Some((_, Tok::Punct('<'))))
                            && matches!(toks.get(k + 1), Some((_, Tok::Punct('<'))));
                        if !is_shift {
                            k += 1;
                            continue;
                        }
                        if let Some((sline, Tok::Word(sig))) = toks.get(k + 2) {
                            let mut edge = SystemCEdge::None;
                            if matches!(toks.get(k + 3), Some((_, Tok::Punct('.')))) {
                                if let Some((_, Tok::Word(m))) = toks.get(k + 4) {
                                    edge = match m.as_str() {
                                        "pos" => SystemCEdge::Pos,
                                        "neg" => SystemCEdge::Neg,
                                        _ => SystemCEdge::None,
                                    };
                                }
                            }
                            res.sensitivities.push(SystemCSensitivity {
                                signal: sig.clone(),
                                edge,
                                line: *sline,
                            });
                            k += 3;
                        } else {
                            k += 1;
                        }
                    }
                    i = end;
                }
                "wait" => {
                    // `wait();` -> None/None; `wait(10, SC_NS);` -> Some/Some.
                    let mut amount = None;
                    let mut unit = None;
                    if matches!(toks.get(i + 1), Some((_, Tok::Punct('(')))) {
                        if let Some((_, Tok::Word(a))) = toks.get(i + 2) {
                            amount = Some(a.clone());
                            let mut j = i + 3;
                            while j < n {
                                if matches!(toks.get(j), Some((_, Tok::Punct(')')))) {
                                    break;
                                }
                                if matches!(toks.get(j), Some((_, Tok::Punct(',')))) {
                                    if let Some((_, Tok::Word(u))) = toks.get(j + 1) {
                                        unit = Some(u.clone());
                                    }
                                    break;
                                }
                                j += 1;
                            }
                        }
                    }
                    res.waits.push(SystemCWait {
                        amount,
                        unit,
                        line: *ln,
                    });
                    i += 1;
                }
                "SC_REPORT_INFO" | "SC_REPORT_WARNING" | "SC_REPORT_ERROR" | "SC_REPORT_FATAL" => {
                    let kind = match w.as_str() {
                        "SC_REPORT_INFO" => SystemCReportKind::Info,
                        "SC_REPORT_WARNING" => SystemCReportKind::Warning,
                        "SC_REPORT_ERROR" => SystemCReportKind::Error,
                        _ => SystemCReportKind::Fatal,
                    };
                    res.reports.push(SystemCReport { kind, line: *ln });
                    i += 1;
                }
                "sc_trace" | "SC_TRACE" => {
                    res.traces.push(SystemCTrace { line: *ln });
                    i += 1;
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

    #[test]
    fn scan_extracts_clocks() {
        // Deklarasi `sc_clock` diekstrak (nama), baik dengan maupun tanpa
        // argumen konstruktor.
        let src = "\
SC_MODULE(tb) {
    sc_clock clk;
    sc_clock fast_clk(\"fast\", 10, SC_NS);
    SC_CTOR(tb) {}
};
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.clocks,
            vec![
                SystemCClock {
                    name: "clk".to_string(),
                    line: 2
                },
                SystemCClock {
                    name: "fast_clk".to_string(),
                    line: 3
                },
            ]
        );
        // Modul/ctor tetap terdeteksi.
        assert_eq!(s.modules.len(), 1);
        assert_eq!(s.ctors.len(), 1);
    }

    #[test]
    fn scan_clock_case_sensitive() {
        // `SC_CLOCK` (uppercase) BUKAN keyword sc_clock -> tidak terhitung.
        let src = "SC_CLOCK(not_clk);\n";
        let s = scan(src).unwrap();
        assert!(s.clocks.is_empty());
    }

    #[test]
    fn scan_extracts_sim_control() {
        // Panggilan `sc_start`/`sc_stop` terdeteksi (jenis + baris).
        let src = "\
int sc_main(int argc, char* argv[]) {
    sc_start(100, SC_NS);
    run_simulation();
    sc_stop();
    return 0;
}
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.sim_control,
            vec![
                SystemCSimControl {
                    kind: SystemCSimControlKind::Start,
                    line: 2
                },
                SystemCSimControl {
                    kind: SystemCSimControlKind::Stop,
                    line: 4
                },
            ]
        );
    }

    #[test]
    fn scan_sim_control_case_sensitive() {
        // `SC_START` (uppercase) BUKAN keyword -> tidak terhitung.
        let src = "SC_START(100);\n";
        let s = scan(src).unwrap();
        assert!(s.sim_control.is_empty());
    }

    #[test]
    fn scan_extracts_sc_main() {
        // Entry point `sc_main` terdeteksi (baris pertama diambil).
        let src = "\
#include \"systemc.h\"
int sc_main(int argc, char* argv[]) {
    sc_start(100, SC_NS);
    sc_stop();
    return 0;
}
";
        let s = scan(src).unwrap();
        assert_eq!(s.sc_main, Some(2));
    }

    #[test]
    fn scan_sc_main_case_sensitive() {
        // `SC_MAIN` (uppercase) BUKAN keyword -> tidak terhitung.
        let src = "int SC_MAIN(int argc, char* argv[]) { return 0; }\n";
        let s = scan(src).unwrap();
        assert_eq!(s.sc_main, None);
    }

    #[test]
    fn scan_extracts_channels() {
        // Channel/sinkronisasi `sc_fifo`/`sc_event`/`sc_mutex`/`sc_semaphore`
        // terdeteksi (nama + jenis).
        let src = "\
#include \"systemc.h\"
SC_MODULE(tb) {
    sc_fifo<int> data_fifo;
    sc_event done_evt;
    sc_mutex bus_mutex;
    sc_semaphore port_sem;
};
";
        let s = scan(src).unwrap();
        assert_eq!(s.channels.len(), 4, "4 channel dikenali: {s:?}");
        assert_eq!(s.channels[0].name, "data_fifo");
        assert_eq!(s.channels[0].kind, SystemCChannelKind::Fifo);
        assert_eq!(s.channels[1].name, "done_evt");
        assert_eq!(s.channels[1].kind, SystemCChannelKind::Event);
        assert_eq!(s.channels[2].name, "bus_mutex");
        assert_eq!(s.channels[2].kind, SystemCChannelKind::Mutex);
        assert_eq!(s.channels[3].name, "port_sem");
        assert_eq!(s.channels[3].kind, SystemCChannelKind::Semaphore);
        // Modul tetap terdeteksi (1).
        assert_eq!(s.modules.len(), 1);
    }

    #[test]
    fn scan_channel_case_sensitive() {
        // Keyword uppercase (`SC_FIFO`/`SC_EVENT`) BUKAN keyword -> kosong.
        let src = "SC_FIFO<int> f;\nSC_EVENT e;\n";
        let s = scan(src).unwrap();
        assert!(s.channels.is_empty());
    }

    #[test]
    fn scan_extracts_sensitivity() {
        // Sensitivity list `sensitive << sinyal.pos()` diekstrak (nama +
        // edge + baris); beberapa sinyal dalam satu statement didukung.
        let src = "\
SC_MODULE(tb) {
    SC_CTOR(tb) {
        SC_METHOD(run);
        sensitive << clk.pos() << rst.neg();
        sensitive << data;
    }
};
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.sensitivities,
            vec![
                SystemCSensitivity {
                    signal: "clk".to_string(),
                    edge: SystemCEdge::Pos,
                    line: 4,
                },
                SystemCSensitivity {
                    signal: "rst".to_string(),
                    edge: SystemCEdge::Neg,
                    line: 4,
                },
                SystemCSensitivity {
                    signal: "data".to_string(),
                    edge: SystemCEdge::None,
                    line: 5,
                },
            ]
        );
        assert_eq!(s.modules.len(), 1);
        assert_eq!(s.processes.len(), 1);
    }

    #[test]
    fn scan_sensitivity_case_sensitive() {
        // `SENSITIVE` (uppercase) BUKAN keyword -> tidak terhitung.
        let src = "SENSITIVE << clk.pos();\n";
        let s = scan(src).unwrap();
        assert!(s.sensitivities.is_empty());
    }

    #[test]
    fn scan_extracts_waits() {
        // `wait(10, SC_NS)` -> durasi + satuan; `wait()` -> keduanya None.
        let src = "\
SC_MODULE(tb) {
    void run() {
        wait(10, SC_NS);
        wait();
    }
};
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.waits,
            vec![
                SystemCWait {
                    amount: Some("10".to_string()),
                    unit: Some("SC_NS".to_string()),
                    line: 3,
                },
                SystemCWait {
                    amount: None,
                    unit: None,
                    line: 4,
                },
            ]
        );
        assert_eq!(s.modules.len(), 1);
    }

    #[test]
    fn scan_wait_case_sensitive() {
        // `WAIT` (uppercase) BUKAN keyword -> tidak terhitung.
        let src = "WAIT(10, SC_NS);\n";
        let s = scan(src).unwrap();
        assert!(s.waits.is_empty());
    }

    #[test]
    fn scan_extracts_reports() {
        // Makro SC_REPORT_* terdeteksi (jenis + baris).
        let src = "\
SC_MODULE(tb) {
    void run() {
        SC_REPORT_INFO(\"ID\", \"msg\");
        SC_REPORT_WARNING(\"ID\", \"msg\");
        SC_REPORT_ERROR(\"ID\", \"msg\");
        SC_REPORT_FATAL(\"ID\", \"msg\");
    }
};
";
        let s = scan(src).unwrap();
        assert_eq!(
            s.reports,
            vec![
                SystemCReport {
                    kind: SystemCReportKind::Info,
                    line: 3,
                },
                SystemCReport {
                    kind: SystemCReportKind::Warning,
                    line: 4,
                },
                SystemCReport {
                    kind: SystemCReportKind::Error,
                    line: 5,
                },
                SystemCReport {
                    kind: SystemCReportKind::Fatal,
                    line: 6,
                },
            ]
        );
        assert_eq!(s.modules.len(), 1);
    }

    #[test]
    fn scan_report_case_sensitive() {
        // `sc_report_info` (lowercase) BUKAN makro -> tidak terhitung.
        let src = "sc_report_info(\"ID\", \"msg\");\n";
        let s = scan(src).unwrap();
        assert!(s.reports.is_empty());
    }

    #[test]
    fn scan_extracts_traces() {
        // Pemanggilan `sc_trace` diekstrak (baris).
        let src = "\
sc_trace(tf, clk, \"clk\");
sc_trace(tf, rst, \"rst\");
";
        let s = scan(src).unwrap();
        assert_eq!(s.traces.len(), 2, "{s:?}");
        assert_eq!(s.traces[0].line, 1);
        assert_eq!(s.traces[1].line, 2);
        assert_eq!(s.modules.len(), 0);
    }

    #[test]
    fn scan_trace_case_sensitive() {
        // `SC_TRACE` uppercase juga terdeteksi (arm terima keduanya).
        let src = "SC_TRACE(tf, a, \"a\");\n";
        let s = scan(src).unwrap();
        assert_eq!(s.traces.len(), 1, "{s:?}");
        assert_eq!(s.traces[0].line, 1);
        assert_eq!(s.modules.len(), 0);
    }
}
