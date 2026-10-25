//! Pemeriksa aturan protokol bus pada waveform (ENT-24 / FEAT-23).
//!
//! 1 file = 1 tanggung jawab: validasi legalitas sinyal protokol (APB /
//! AXI4-Lite) dari VCD. Decode transaksi ada di `decode.rs`; modul ini
//! hanya melaporkan pelanggaran aturan (bukan mengubah data).
//!
//! Sampel diambil dengan *merge sweep*: seluruh waktu perubahan digabung
//! sekali, lalu setiap sinyal punya kursor sendiri — O(N log N), bukan
//! O(N²) seperti `sample_at` per waktu union.

use mivon_core::diagnostics::{DiagCode, Diagnostic};
use mivon_core::error::SimError;

use super::data::{VcdData, VcdSignal};
use super::util::{full_name, timeline_of};

/// Satu pelanggaran aturan protokol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Violation {
    /// Waktu (satuan timescale VCD) saat pelanggaran terdeteksi.
    pub(crate) time: u64,
    /// Kode aturan, mis. `APB-PENABLE-NO-SEL`.
    pub(crate) rule: &'static str,
    /// Penjelasan manusia.
    pub(crate) detail: String,
}

fn high(v: &str) -> bool {
    v == "1"
}

/// Error pemakaian/konfigurasi (bukan I/O waveform). Kode tetap W0001 agar
/// exit code 10, tapi explanation/help-nya khusus — explanation generik
/// "file VCD tidak bisa dibaca" akan menyesatkan untuk argumen yang salah.
fn usage_err(msg: impl Into<String>) -> SimError {
    SimError::Diagnostic(
        Diagnostic::error(DiagCode::WaveformError, msg.into())
            .with_explanation(
                "Argumen subcommand protocol atau nama sinyal pada VCD tidak valid untuk checker ini.",
            )
            .with_suggestion(
                "Periksa --proto (apb | axi4lite) dan nama sinyal dengan subcommand tree.",
            ),
    )
}

/// Resolusi nama sinyal: nama penuh persis dulu, lalu komponen terakhir yang
/// unik. Nama ambigu (lebih dari satu instance) → error, bukan diam-diam
/// memeriksa instance pertama.
fn resolve<'a>(data: &'a VcdData, name: &str) -> Result<&'a VcdSignal, SimError> {
    let last = |s: &VcdSignal| full_name(s).rsplit('.').next().unwrap_or("").to_string();
    let exact: Vec<&VcdSignal> = data
        .signals
        .iter()
        .filter(|s| full_name(s) == name)
        .collect();
    let candidates: Vec<&VcdSignal> = if exact.is_empty() {
        data.signals
            .iter()
            .filter(|s| last(s) == name || last(s).eq_ignore_ascii_case(name))
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => Ok(one),
        [] => Err(usage_err(format!(
            "check: sinyal '{}' tidak ditemukan pada VCD",
            name
        ))),
        many => Err(usage_err(format!(
            "check: sinyal '{}' ambigu ({} instance) — checker protokol belum mendukung VCD multi-instance",
            name,
            many.len()
        ))),
    }
}

/// Timeline satu sinyal sebagai bit (1 = tinggi, selain itu rendah) + kursor
/// sweep. `at()` wajib dipanggil dengan waktu menaik.
struct Track {
    changes: Vec<(u64, bool)>,
    cur: usize,
    val: bool,
}

impl Track {
    fn new(data: &VcdData, name: &str) -> Result<Self, SimError> {
        let sig = resolve(data, name)?;
        Ok(Self {
            changes: timeline_of(data, sig)
                .into_iter()
                .map(|(t, v)| (t, high(&v)))
                .collect(),
            cur: 0,
            val: false,
        })
    }

    fn at(&mut self, t: u64) -> bool {
        while self.cur < self.changes.len() && self.changes[self.cur].0 <= t {
            self.val = self.changes[self.cur].1;
            self.cur += 1;
        }
        self.val
    }
}

/// Handshake VALID&READY: konjungsi menjadi benar pada timestamp ini, dengan
/// toleransi zero-delay (salah satu sinyal boleh berubah pada timestamp yang
/// sama). Tidak dihitung ulang saat keduanya tetap tinggi — penting karena
/// waktu sampel adalah event-time (semua perubahan sinyal), bukan clock edge.
fn handshake(pv: bool, pr: bool, cv: bool, cr: bool) -> bool {
    // Keduanya baru tinggi bersamaan, atau VALID turun tepat saat READY naik
    // (penyelesaian zero-delay). Arah sebaliknya (VALID naik + READY turun)
    // bukan transfer — READY yang turun tidak boleh menyelesaikan handshake.
    let both_now = cv && cr && !(pv && pr);
    let completion = pv && !pr && !cv && cr;
    both_now || completion
}

/// Gabungan waktu perubahan semua track (sorted-unik).
fn merged_times(tracks: &[&Track]) -> Vec<u64> {
    let mut times: Vec<u64> = tracks
        .iter()
        .flat_map(|t| t.changes.iter().map(|(t, _)| *t))
        .collect();
    times.sort_unstable();
    times.dedup();
    times
}

/// Cek aturan APB:
/// - PENABLE=1 wajib disertai PSEL=1.
/// - ACCESS (PSEL&PENABLE) tidak boleh berakhir tanpa PREADY=1.
///
/// PREADY wajib ada: tanpa itu dua aturan di atas tidak bisa dievaluasi, jadi
/// lebih baik error eksplisit daripada diam-diam meloloskan waveform.
pub(crate) fn check_apb(data: &VcdData) -> Result<Vec<Violation>, SimError> {
    let mut psel = Track::new(data, "psel")?;
    let mut penable = Track::new(data, "penable")?;
    let mut pready = Track::new(data, "pready")?;
    let times = merged_times(&[&psel, &penable, &pready]);
    let mut out = Vec::new();
    let mut prev: Option<(bool, bool, bool)> = None;
    for t in times {
        let sel = psel.at(t);
        let en = penable.at(t);
        let rdy = pready.at(t);
        // Sekali per interval ilegal (bukan setiap perubahan sinyal lain
        // selama state ilegal bertahan).
        let was_illegal = prev.map(|(p_sel, p_en, _)| p_en && !p_sel).unwrap_or(false);
        if en && !sel && !was_illegal {
            out.push(Violation {
                time: t,
                rule: "APB-PENABLE-NO-SEL",
                detail: "PENABLE=1 saat PSEL tidak aktif (0/X/Z)".to_string(),
            });
        }
        if let Some((p_sel, p_en, p_rdy)) = prev {
            // PREADY yang naik tepat pada waktu yang sama tetap dianggap
            // menyelesaikan transfer (model zero-delay).
            if p_sel && p_en && !p_rdy && !rdy && (!sel || !en) {
                out.push(Violation {
                    time: t,
                    rule: "APB-ACCESS-ABORTED",
                    detail: "ACCESS berakhir tanpa PREADY=1".to_string(),
                });
            }
        }
        prev = Some((sel, en, rdy));
    }
    Ok(out)
}

/// Cek aturan AXI4-Lite:
/// - VALID (AW/W/AR) tidak boleh ditarik sebelum READY.
/// - BVALID/RVALID tidak boleh aktif tanpa request yang belum direspons
///   (counter outstanding, bukan sekadar "pernah ada handshake").
pub(crate) fn check_axi4lite(data: &VcdData) -> Result<Vec<Violation>, SimError> {
    let mut awv = Track::new(data, "awvalid")?;
    let mut awr = Track::new(data, "awready")?;
    let mut wv = Track::new(data, "wvalid")?;
    let mut wr = Track::new(data, "wready")?;
    let mut arv = Track::new(data, "arvalid")?;
    let mut arr = Track::new(data, "arready")?;
    let mut bv = Track::new(data, "bvalid")?;
    let mut br = Track::new(data, "bready")?;
    let mut rv = Track::new(data, "rvalid")?;
    let mut rr = Track::new(data, "rready")?;
    let times = merged_times(&[&awv, &awr, &wv, &wr, &arv, &arr, &bv, &br, &rv, &rr]);

    let mut out = Vec::new();
    let mut aw_out: i64 = 0;
    let mut ar_out: i64 = 0;
    // Satu burst respons tanpa request dilaporkan paling banyak sekali.
    let mut b_reported = false;
    let mut r_reported = false;
    // Urutan: awvalid, awready, wvalid, wready, arvalid, arready,
    //          bvalid, bready, rvalid, rready.
    let mut prev = [false; 10];
    for t in times {
        let cur = [
            awv.at(t),
            awr.at(t),
            wv.at(t),
            wr.at(t),
            arv.at(t),
            arr.at(t),
            bv.at(t),
            br.at(t),
            rv.at(t),
            rr.at(t),
        ];

        // 1) VALID ditarik sebelum handshake. READY yang naik tepat pada
        //    waktu yang sama berarti handshake sah.
        let chans = [
            (0usize, 1usize, "AXIL-AWVALID-WITHDRAWN", "AW"),
            (2, 3, "AXIL-WVALID-WITHDRAWN", "W"),
            (4, 5, "AXIL-ARVALID-WITHDRAWN", "AR"),
        ];
        for (vi, ri, rule, name) in chans {
            if prev[vi] && !prev[ri] && !cur[vi] && !cur[ri] {
                out.push(Violation {
                    time: t,
                    rule,
                    detail: format!("{}VALID ditarik sebelum READY", name),
                });
            }
        }

        // 2) Handshake request menambah outstanding — diproses sebelum
        //    respons pada timestamp yang sama.
        if handshake(prev[0], prev[1], cur[0], cur[1]) {
            aw_out += 1;
        }
        if handshake(prev[4], prev[5], cur[4], cur[5]) {
            ar_out += 1;
        }

        // 3) Respons tanpa request yang tertunda. Satu burst BVALID/RVALID
        //    dilaporkan paling banyak sekali; reset saat sinyal turun tanpa
        //    menyelesaikan handshake (completion zero-delay tetap dihitung).
        let b_start = cur[6] && !prev[6];
        let b_hs = handshake(prev[6], prev[7], cur[6], cur[7]);
        if !b_reported && (b_start || b_hs) && aw_out == 0 {
            out.push(Violation {
                time: t,
                rule: "AXIL-BVALID-NO-REQUEST",
                detail: "BVALID aktif tanpa handshake AW yang belum direspons".to_string(),
            });
            b_reported = true;
        }
        // Reset setelah BVALID turun (termasuk jalur completion zero-delay:
        // laporan di atas sudah dieksekusi, jadi tidak ada duplikasi).
        if !cur[6] {
            b_reported = false;
        }
        if b_hs {
            aw_out = (aw_out - 1).max(0);
        }
        let r_start = cur[8] && !prev[8];
        let r_hs = handshake(prev[8], prev[9], cur[8], cur[9]);
        if !r_reported && (r_start || r_hs) && ar_out == 0 {
            out.push(Violation {
                time: t,
                rule: "AXIL-RVALID-NO-REQUEST",
                detail: "RVALID aktif tanpa handshake AR yang belum direspons".to_string(),
            });
            r_reported = true;
        }
        if !cur[8] {
            r_reported = false;
        }
        if r_hs {
            ar_out = (ar_out - 1).max(0);
        }

        prev = cur;
    }
    Ok(out)
}

/// Entry CLI: cek protokol, cetak pelanggaran, dan kembalikan Err bila ada
/// (exit code non-zero → bisa dipakai sebagai gate CI).
pub(crate) fn check(input: &str, proto: &str) -> Result<(), SimError> {
    let data = super::data::parse_vcd(input)?;
    let p = proto.to_ascii_lowercase().replace('_', "");
    let violations = match p.as_str() {
        "apb" => check_apb(&data)?,
        "axi4lite" | "axi-lite" | "axi4-lite" | "axilite" => check_axi4lite(&data)?,
        other => {
            return Err(usage_err(format!(
                "protokol tidak dikenal: '{}' (pilihan: apb | axi4lite)",
                other
            )))
        }
    };
    println!(
        "  check: {} (proto={}, max_time={})",
        input, proto, data.max_time
    );
    for v in &violations {
        println!("  t={:<6} {} — {}", v.time, v.rule, v.detail);
    }
    if violations.is_empty() {
        println!("  OK: tidak ada pelanggaran protokol");
        Ok(())
    } else {
        Err(SimError::with_diag(
            DiagCode::ProtocolViolation,
            format!(
                "{} pelanggaran protokol {} terdeteksi",
                violations.len(),
                proto
            ),
        ))
    }
}
