//! FEAT-23 / ENT-24: Runtime VIP protocol checker.
//!
//! Memantau sinyal protokol bus (APB / AXI4-Lite / AHB) SECARA RUNTIME selama
//! simulasi, cycle-by-cycle, dan melaporkan pelanggaran sebagai diagnostic.
//! Berbeda dari post-sim checker di `wave/protocol.rs` (membaca VCD pasca
//! simulasi), checker ini memantau nilai sinyal langsung dari
//! `SimulationState` saat simulasi berjalan, sehingga pelanggaran protokol
//! dapat ditangkap seketika terjadi (cycle-by-cycle) bukan setelah selesai.
//!
//! 1 file = 1 tanggung jawab: runtime VIP protocol checking.

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_ir::LogicVec;

use crate::simulator::engine::SimulationEngine;

/// Protokol bus yang dapat dipantau runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VipProtocol {
    Apb,
    Axi4Lite,
    Ahb,
}

impl std::fmt::Display for VipProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VipProtocol::Apb => write!(f, "apb"),
            VipProtocol::Axi4Lite => write!(f, "axi4lite"),
            VipProtocol::Ahb => write!(f, "ahb"),
        }
    }
}

/// Satu pelanggaran protokol yang terdeteksi runtime.
#[derive(Debug, Clone)]
pub struct VipViolation {
    pub time: u64,
    pub rule: &'static str,
    pub detail: String,
}

impl VipViolation {
    fn new(time: u64, rule: &'static str, detail: impl Into<String>) -> Self {
        Self {
            time,
            rule,
            detail: detail.into(),
        }
    }
}

/// Nilai boolean terakhir tiap sinyal yang dipantau (untuk deteksi tepi /
/// transisi antar time step).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum LastBool {
    #[default]
    Unknown,
    Low,
    High,
}

impl LastBool {
    fn from_lv(lv: &LogicVec) -> Self {
        if lv.to_u64() != 0 {
            LastBool::High
        } else {
            LastBool::Low
        }
    }
    fn is_high(&self) -> bool {
        matches!(self, LastBool::High)
    }
    fn is_known(&self) -> bool {
        matches!(self, LastBool::Low | LastBool::High)
    }
}

/// Hasil pemeriksaan satu time step.
#[derive(Debug, Default)]
pub struct VipCheckResult {
    pub violations: Vec<VipViolation>,
}

/// Checker runtime untuk satu protokol bus. Memantau sinyal dari engine state
/// setiap time step dan melaporkan pelanggaran aturan protokol.
pub struct VipChecker {
    pub protocol: VipProtocol,
    pub enabled: bool,
    pub violations: Vec<VipViolation>,
    // APB
    last_psel: LastBool,
    last_penable: LastBool,
    last_pready: LastBool,
    // AXI4-Lite
    last_awvalid: LastBool,
    last_awready: LastBool,
    last_wvalid: LastBool,
    last_wready: LastBool,
    last_arvalid: LastBool,
    last_arready: LastBool,
    last_bvalid: LastBool,
    last_bready: LastBool,
    last_rvalid: LastBool,
    last_rready: LastBool,
    // Outstanding request counter (AXI4-Lite)
    aw_outstanding: i64,
    ar_outstanding: i64,
    // AHB
    last_htrans: LastBool, // High jika bit[1] HTRANS = 1 (Nonseq/Seq)
    last_hready: LastBool,
    last_hwrite: LastBool,
    // AHB span state
    ahb_span_active: bool,
    ahb_ready_in_span: bool,
    ahb_saw_nonseq: bool,
    ahb_hwrite_start: Option<bool>,
    ahb_hwrite_reported: bool,
    // AHB extended state
    ahb_hresp_start: Option<u8>,
    ahb_hresp_reported: bool,
    ahb_burst_len: Option<u8>,
    ahb_burst_count: u8,
    ahb_hmastlock_start: Option<bool>,
    ahb_hmastlock_reported: bool,
}

impl VipChecker {
    pub fn new(protocol: VipProtocol) -> Self {
        Self {
            protocol,
            enabled: true,
            violations: Vec::new(),
            last_psel: LastBool::Unknown,
            last_penable: LastBool::Unknown,
            last_pready: LastBool::Unknown,
            last_awvalid: LastBool::Unknown,
            last_awready: LastBool::Unknown,
            last_wvalid: LastBool::Unknown,
            last_wready: LastBool::Unknown,
            last_arvalid: LastBool::Unknown,
            last_arready: LastBool::Unknown,
            last_bvalid: LastBool::Unknown,
            last_bready: LastBool::Unknown,
            last_rvalid: LastBool::Unknown,
            last_rready: LastBool::Unknown,
            aw_outstanding: 0,
            ar_outstanding: 0,
            last_htrans: LastBool::Unknown,
            last_hready: LastBool::Unknown,
            last_hwrite: LastBool::Unknown,
            ahb_span_active: false,
            ahb_ready_in_span: false,
            ahb_saw_nonseq: false,
            ahb_hwrite_start: None,
            ahb_hwrite_reported: false,
            ahb_hresp_start: None,
            ahb_hresp_reported: false,
            ahb_burst_len: None,
            ahb_burst_count: 0,
            ahb_hmastlock_start: None,
            ahb_hmastlock_reported: false,
        }
    }

    /// Resolve sinyal dari design top oleh nama. Mengembalikan SignalId atau
    /// None bila sinyal tidak ada.
    fn resolve_signal(engine: &SimulationEngine, name: &str) -> Option<usize> {
        engine
            .design
            .top
            .signals
            .iter()
            .enumerate()
            .find(|(_, s)| s.name.as_str() == name)
            .map(|(i, _)| i)
    }

    /// Baca nilai boolean sinyal. None bila sinyal tidak ditemukan.
    fn read_bool(engine: &SimulationEngine, name: &str) -> Option<bool> {
        let id = Self::resolve_signal(engine, name)?;
        let lv = engine.state.read_signal(id);
        Some(lv.to_u64() != 0)
    }

    /// Baca nilai u64 mentah sinyal. None bila sinyal tidak ditemukan.
    /// Dipakai untuk sinyal multi-bit (contoh: HTRANS) di mana bit tertentu
    /// bermakna, bukan seluruh nilai sebagai boolean.
    fn read_u64(engine: &SimulationEngine, name: &str) -> Option<u64> {
        let id = Self::resolve_signal(engine, name)?;
        Some(engine.state.read_signal(id).to_u64())
    }

    /// Jalankan pemeriksaan satu time step. Dipanggil dari engine run loop
    /// setelah semua sinyal di-commit untuk time step ini (region Postponed).
    pub fn check_time_step(&mut self, time: u64, engine: &SimulationEngine) -> Result<(), SimError> {
        if !self.enabled {
            return Ok(());
        }
        match self.protocol {
            VipProtocol::Apb => self.check_apb(time, engine),
            VipProtocol::Axi4Lite => self.check_axi4lite(time, engine),
            VipProtocol::Ahb => self.check_ahb(time, engine),
        }
    }

    /// Laporan ringkasan pelanggaran.
    pub fn report(&self) -> String {
        if self.violations.is_empty() {
            format!("VIP {}: tidak ada pelanggaran protokol", self.protocol)
        } else {
            let mut out = format!("VIP {}: {} pelanggaran protokol\n", self.protocol, self.violations.len());
            for v in &self.violations {
                out.push_str(&format!("  t={:<6} {} — {}\n", v.time, v.rule, v.detail));
            }
            out
        }
    }

    /// Emit semua pelanggaran terakumulasi sebagai diagnostic ke engine, lalu
    /// clear daftar agar tidak duplikasi pada laporan berikutnya.
    pub fn flush_diagnostics(&mut self, engine: &mut SimulationEngine) {
        for v in self.violations.drain(..) {
            engine.emit_warning(
                DiagCode::ProtocolViolation,
                format!(
                    "VIP {}: {} — {} (t={})",
                    self.protocol, v.rule, v.detail, v.time
                ),
            );
        }
    }

    // ═══ APB ═══
    // - PENABLE=1 wajib disertai PSEL=1.
    // - ACCESS (PSEL & PENABLE) tidak boleh berakhir tanpa PREADY=1.
    fn check_apb(&mut self, time: u64, engine: &SimulationEngine) -> Result<(), SimError> {
        let sel = match Self::read_bool(engine, "psel") {
            Some(v) => v,
            None => return Ok(()),
        };
        let en = match Self::read_bool(engine, "penable") {
            Some(v) => v,
            None => return Ok(()),
        };
        let rdy = match Self::read_bool(engine, "pready") {
            Some(v) => v,
            None => return Ok(()),
        };

        // Aturan 1: PENABLE=1 wajib disertai PSEL=1. Lapor sekali per interval
        // ilegal (bukan setiap perubahan sinyal lain selama state ilegal bertahan).
        // `was_illegal` = state ilegal sudah ada di time step sebelumnya.
        let was_illegal = self.last_penable.is_high() && !self.last_psel.is_high();
        if en && !sel && !was_illegal {
            self.violations.push(VipViolation::new(
                time,
                "APB-PENABLE-NO-SEL",
                "PENABLE=1 saat PSEL tidak aktif (0/X/Z)",
            ));
        }

        // Aturan 2: ACCESS (PSEL & PENABLE) berakhir tanpa PREADY=1.
        // Terjadi bila access aktif di time step lalu, access berakhir di
        // time step ini (penable turun atau psel turun), dan pready masih
        // low di time step ini (handshake belum selesai).
        let access_was_active = self.last_psel.is_high() && self.last_penable.is_high();
        let access_ended = access_was_active && !(sel && en);
        if access_was_active && access_ended && !rdy {
            self.violations.push(VipViolation::new(
                time,
                "APB-ACCESS-ABORTED",
                "ACCESS berakhir tanpa PREADY=1",
            ));
        }

        self.last_psel = LastBool::from_lv(&LogicVec::from_u64(if sel { 1 } else { 0 }, 1));
        self.last_penable = LastBool::from_lv(&LogicVec::from_u64(if en { 1 } else { 0 }, 1));
        self.last_pready = LastBool::from_lv(&LogicVec::from_u64(if rdy { 1 } else { 0 }, 1));
        Ok(())
    }

    // ═══ AXI4-Lite ═══
    // - VALID (AW/W/AR) tidak boleh ditarik sebelum READY.
    // - BVALID/RVALID tidak boleh aktif tanpa request yang belum direspons
    //   (counter outstanding).
    fn check_axi4lite(&mut self, time: u64, engine: &SimulationEngine) -> Result<(), SimError> {
        let read = |name: &str| -> bool {
            match Self::read_bool(engine, name) {
                Some(v) => v,
                None => return false,
            }
        };
        let awv = read("awvalid");
        let awr = read("awready");
        let wv = read("wvalid");
        let wr = read("wready");
        let arv = read("arvalid");
        let arr = read("arready");
        let bv = read("bvalid");
        let br = read("bready");
        let rv = read("rvalid");
        let rr = read("rready");

        // Aturan 1: VALID ditarik sebelum handshake.
        // Deteksi: VALID=1 & READY=0 di time step lalu, lalu VALID=0 & READY=0
        // di time step ini (ditarik tanpa pernah di-respons).
        let channels = [
            (
                self.last_awvalid.is_high(),
                self.last_awready.is_high(),
                awv,
                awr,
                "AXIL-AWVALID-WITHDRAWN",
                "AW",
            ),
            (
                self.last_wvalid.is_high(),
                self.last_wready.is_high(),
                wv,
                wr,
                "AXIL-WVALID-WITHDRAWN",
                "W",
            ),
            (
                self.last_arvalid.is_high(),
                self.last_arready.is_high(),
                arv,
                arr,
                "AXIL-ARVALID-WITHDRAWN",
                "AR",
            ),
        ];
        for (prev_valid, prev_ready, cur_valid, cur_ready, rule, name) in channels {
            if prev_valid && !prev_ready && !cur_valid && !cur_ready {
                self.violations.push(VipViolation::new(
                    time,
                    rule,
                    format!("{}VALID ditarik sebelum READY", name),
                ));
            }
        }

        // Aturan 2: Outstanding request counter.
        // Request mulai saat VALID naik (0→1). Request selesai saat handshake
        // (VALID=1 & READY=1).
        let aw_was_high = self.last_awvalid.is_high();
        if !aw_was_high && awv {
            self.aw_outstanding += 1;
        }
        if awv && awr {
            self.aw_outstanding = (self.aw_outstanding - 1).max(0);
        }
        let ar_was_high = self.last_arvalid.is_high();
        if !ar_was_high && arv {
            self.ar_outstanding += 1;
        }
        if arv && arr {
            self.ar_outstanding = (self.ar_outstanding - 1).max(0);
        }

        // Aturan 3: BVALID/RVALID tanpa outstanding request.
        // Lapor sekali per burst (flag direset saat sinyal turun).
        let b_start = bv && !self.last_bvalid.is_high();
        let b_hs = self.last_bvalid.is_high() && !self.last_bready.is_high() && bv && br;
        if (b_start || b_hs) && self.aw_outstanding == 0 {
            self.violations.push(VipViolation::new(
                time,
                "AXIL-BVALID-NO-REQUEST",
                "BVALID aktif tanpa handshake AW yang belum direspons",
            ));
        }
        if b_hs {
            self.aw_outstanding = (self.aw_outstanding - 1).max(0);
        }
        let r_start = rv && !self.last_rvalid.is_high();
        let r_hs = self.last_rvalid.is_high() && !self.last_rready.is_high() && rv && rr;
        if (r_start || r_hs) && self.ar_outstanding == 0 {
            self.violations.push(VipViolation::new(
                time,
                "AXIL-RVALID-NO-REQUEST",
                "RVALID aktif tanpa handshake AR yang belum direspons",
            ));
        }
        if r_hs {
            self.ar_outstanding = (self.ar_outstanding - 1).max(0);
        }

        self.last_awvalid = LastBool::from_lv(&LogicVec::from_u64(if awv { 1 } else { 0 }, 1));
        self.last_awready = LastBool::from_lv(&LogicVec::from_u64(if awr { 1 } else { 0 }, 1));
        self.last_wvalid = LastBool::from_lv(&LogicVec::from_u64(if wv { 1 } else { 0 }, 1));
        self.last_wready = LastBool::from_lv(&LogicVec::from_u64(if wr { 1 } else { 0 }, 1));
        self.last_arvalid = LastBool::from_lv(&LogicVec::from_u64(if arv { 1 } else { 0 }, 1));
        self.last_arready = LastBool::from_lv(&LogicVec::from_u64(if arr { 1 } else { 0 }, 1));
        self.last_bvalid = LastBool::from_lv(&LogicVec::from_u64(if bv { 1 } else { 0 }, 1));
        self.last_bready = LastBool::from_lv(&LogicVec::from_u64(if br { 1 } else { 0 }, 1));
        self.last_rvalid = LastBool::from_lv(&LogicVec::from_u64(if rv { 1 } else { 0 }, 1));
        self.last_rready = LastBool::from_lv(&LogicVec::from_u64(if rr { 1 } else { 0 }, 1));
        Ok(())
    }

    // ═══ AHB ═══
    // - HREADY wajib tinggi saat transfer selesai.
    // - HTRANS=SEQ wajib didahului NONSEQ sejak IDLE terakhir.
    // - HWRITE stabil selama burst (jika sinyal hwrite ada).
    fn check_ahb(&mut self, time: u64, engine: &SimulationEngine) -> Result<(), SimError> {
        // HTRANS multi-bit: bit[1] = 1 berarti Nonseq (10) atau Seq (11).
        // BUSY (01) dan Idle (00) punya bit[1] = 0 → bukan transfer aktif.
        let htrans_val = match Self::read_u64(engine, "htrans") {
            Some(v) => v,
            None => return Ok(()),
        };
        let is_seq_or_nonseq = (htrans_val >> 1) & 1 == 1;
        let htrans_high = LastBool::from_lv(&LogicVec::from_u64(if is_seq_or_nonseq { 1 } else { 0 }, 1));

        let hready = match Self::read_bool(engine, "hready") {
            Some(v) => v,
            None => return Ok(()),
        };
        let hwrite = Self::read_bool(engine, "hwrite");

        // Aturan 2: SEQ tanpa NONSEQ pendahulu sejak IDLE terakhir (termasuk
        // BUSY->SEQ tanpa NONSEQ). Lapor sekali saat SEQ muncul.
        // Hanya cek untuk SEQ (htrans=3), bukan NONSEQ (htrans=2).
        let is_seq = htrans_val == 3;
        if is_seq && !self.ahb_saw_nonseq {
            self.violations.push(VipViolation::new(
                time,
                "AHB-SEQ-WITHOUT-NONSEQ",
                "HTRANS=SEQ muncul tanpa HTRANS=NONSEQ pendahulu (burst tanpa awalan)",
            ));
        }

        let starts_span = is_seq_or_nonseq && !self.ahb_span_active;
        if starts_span {
            self.ahb_hwrite_start = hwrite;
            self.ahb_hwrite_reported = false;
        }

        if is_seq_or_nonseq {
            self.ahb_span_active = true;
            if hready {
                self.ahb_ready_in_span = true;
            }
            self.ahb_saw_nonseq = true;
        } else if !htrans_high.is_high() {
            // Idle (00) atau Busy (01) — bus idle/lewat. BUSY bukan akhir span.
            if self.ahb_span_active && !self.ahb_ready_in_span {
                self.violations.push(VipViolation::new(
                    time,
                    "AHB-TRANSFER-NO-READY",
                    "Transfer (NONSEQ/SEQ) berakhir di IDLE tanpa HREADY=1",
                ));
            }
            self.ahb_span_active = false;
            self.ahb_ready_in_span = false;
            self.ahb_saw_nonseq = false;
            self.ahb_hwrite_start = None;
            self.ahb_hwrite_reported = false;
        } else if self.ahb_span_active && hready {
            // Busy wait-state dalam burst: HREADY masih dihitung.
            self.ahb_ready_in_span = true;
        }

        // Aturan 3: HWRITE stabil selama burst (opsional, hanya bila hwrite ada).
        if self.ahb_span_active {
            if let Some(start) = self.ahb_hwrite_start {
                if let Some(now) = hwrite {
                    if start != now && !self.ahb_hwrite_reported {
                        self.violations.push(VipViolation::new(
                            time,
                            "AHB-HWRITE-UNSTABLE",
                            "HWRITE berubah selama burst (harus stabil sejak NONSEQ awal)",
                        ));
                        self.ahb_hwrite_reported = true;
                    }
                }
            }
        }

        // Aturan 4: HRESP checking (OKAY=00, ERROR=01, RETRY=10, SPLIT=11).
        // Lapor ERROR/RETRY/SPLIT saat transfer selesai (HREADY=1 DAN ada transfer aktif).
        let hresp = Self::read_u64(engine, "hresp");
        if let Some(hresp_val) = hresp {
            let hresp_code = (hresp_val & 0b11) as u8;
            // Hanya cek HRESP saat transfer aktif (NONSEQ/SEQ) DAN hready=1
            if is_seq_or_nonseq && hready {
                if hresp_code != 0b00 && !self.ahb_hresp_reported {
                    let (rule, detail) = match hresp_code {
                        0b01 => ("AHB-HRESP-ERROR", "Slave mengembalikan ERROR response"),
                        0b10 => ("AHB-HRESP-RETRY", "Slave meminta RETRY (ulangi transfer)"),
                        0b11 => ("AHB-HRESP-SPLIT", "Slave meminta SPLIT (deferred transfer)"),
                        _ => ("AHB-HRESP-UNKNOWN", "Kode HRESP tidak dikenal"),
                    };
                    self.violations.push(VipViolation::new(time, rule, detail));
                    self.ahb_hresp_reported = true;
                } else if hresp_code == 0b00 {
                    self.ahb_hresp_reported = false;
                }
            }
            self.ahb_hresp_start = Some(hresp_code);
        }

        // Aturan 5: HBURST checking (burst length validation).
        // HBURST: SINGLE=000, INCR=001, WRAP4=010, INCR4=011, WRAP8=100, INCR8=101, WRAP16=110, INCR16=111.
        // Validasi panjang burst: hitung SEQ dalam burst dan bandingkan dengan HBURST.
        let hburst = Self::read_u64(engine, "hburst");
        if let Some(hburst_val) = hburst {
            let burst_type = (hburst_val & 0b111) as u8;
            let is_nonseq = htrans_val == 2; // NONSEQ = 10b = 2
            let is_seq = htrans_val == 3;     // SEQ = 11b = 3
            
            if is_nonseq {
                // NONSEQ memulai burst baru
                self.ahb_burst_len = match burst_type {
                    0b000 => Some(1), // SINGLE
                    0b001 => None,    // INCR (undefined length)
                    0b010 => Some(4), // WRAP4
                    0b011 => Some(4), // INCR4
                    0b100 => Some(8), // WRAP8
                    0b101 => Some(8), // INCR8
                    0b110 => Some(16), // WRAP16
                    0b111 => Some(16), // INCR16
                    _ => None,
                };
                self.ahb_burst_count = 1;
            } else if is_seq && self.ahb_span_active && hready {
                // SEQ dalam burst aktif
                if self.ahb_burst_count > 0 {
                    self.ahb_burst_count += 1;
                }
                // Validasi: jika burst_len didefinisikan, count tidak boleh melebihi
                if let Some(len) = self.ahb_burst_len {
                    if self.ahb_burst_count > len {
                        self.violations.push(VipViolation::new(
                            time,
                            "AHB-BURST-LENGTH-EXCEEDED",
                            format!("Burst melebihi panjang didefinisikan HBURST: {} > {}", self.ahb_burst_count, len),
                        ));
                    }
                }
            } else if !htrans_high.is_high() {
                // Burst berakhir (IDLE/BUSY)
                if let Some(len) = self.ahb_burst_len {
                    if self.ahb_burst_count != len && self.ahb_burst_count > 0 {
                        self.violations.push(VipViolation::new(
                            time,
                            "AHB-BURST-LENGTH-MISMATCH",
                            format!("Burst berakhir dengan panjang {} ≠ HBURST {}", self.ahb_burst_count, len),
                        ));
                    }
                }
                self.ahb_burst_len = None;
                self.ahb_burst_count = 0;
            }
        }

        // Aturan 6: HMASTLOCK checking (lock stabil selama burst).
        let hmastlock = Self::read_bool(engine, "hmastlock");
        if let Some(lock) = hmastlock {
            let is_nonseq = htrans_val == 2; // NONSEQ = 10b = 2
            if is_nonseq && starts_span {
                // NONSEQ memulai burst baru
                self.ahb_hmastlock_start = Some(lock);
                self.ahb_hmastlock_reported = false;
            } else if self.ahb_span_active && hready {
                if let Some(start_lock) = self.ahb_hmastlock_start {
                    if start_lock != lock && !self.ahb_hmastlock_reported {
                        self.violations.push(VipViolation::new(
                            time,
                            "AHB-HMASTLOCK-UNSTABLE",
                            "HMASTLOCK berubah selama burst (harus stabil sejak NONSEQ awal)",
                        ));
                        self.ahb_hmastlock_reported = true;
                    }
                }
            } else if !htrans_high.is_high() {
                self.ahb_hmastlock_start = None;
                self.ahb_hmastlock_reported = false;
            }
        } else if !htrans_high.is_high() {
            // hmastlock tidak ada, reset state saat burst berakhir
            self.ahb_hmastlock_start = None;
            self.ahb_hmastlock_reported = false;
        }

        self.last_htrans = htrans_high;
        self.last_hready = LastBool::from_lv(&LogicVec::from_u64(if hready { 1 } else { 0 }, 1));
        if let Some(hw) = hwrite {
            self.last_hwrite = LastBool::from_lv(&LogicVec::from_u64(if hw { 1 } else { 0 }, 1));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_engine_with_signals(names: &[&str]) -> SimulationEngine {
        use mivon_core::Symbol;
        let signals: Vec<mivon_ir::SignalInfo> = names
            .iter()
            .enumerate()
            .map(|(_i, name)| mivon_ir::SignalInfo {
                name: Symbol::intern(name),
                width: 2, // cukup untuk hold nilai HTRANS 0..3
                kind: mivon_ir::SignalKind::Wire,
                net_type: mivon_ir::NetType::Wire,
                multi_driver: false,
                init_val: LogicVec::from_u64(0, 2),
                array_depth: 0,
                elem_width: 2,
                array_dims: Vec::new(),
                class_name: None,
                is_string: false,
                is_real: false,
                is_mailbox: false,
                is_semaphore: false,
                is_2state: false,
                is_dynamic: false,
                is_queue: false,
                is_associative: false,
                is_signed: false,
                is_const: false,
                msb: 0,
                lsb: 0,
                struct_fields: Vec::new(),
                packed_dims: Vec::new(),
                delay_rise: None,
                delay_fall: None,
                iface_type: None,
                iface_modport: None,
            })
            .collect();
        let design = mivon_ir::IrDesign {
            top: mivon_ir::IrModule {
                name: Symbol::intern("top"),
                signals,
                inputs: Vec::new(),
                outputs: Vec::new(),
                inouts: Vec::new(),
                processes: Vec::new(),
                sub_instances: Vec::new(),
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: Vec::new(),
            dpi_imports: Vec::new(),
            hier_signal_map: HashMap::new(),
            udp_defs: Vec::new(),
            specify_items: Vec::new(),
            timescale: None,
            module_functions: HashMap::new(),
            source_lines: None,
            source_file: None,
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };
        SimulationEngine::new(design, 1000)
    }

    fn set_signal(engine: &mut SimulationEngine, name: &str, val: bool) {
        let id = engine
            .design
            .top
            .signals
            .iter()
            .enumerate()
            .find(|(_, s)| s.name.as_str() == name)
            .map(|(i, _)| i)
            .unwrap();
        engine.state.write_signal(id, LogicVec::from_u64(if val { 1 } else { 0 }, 2));
    }

    fn set_signal_u64(engine: &mut SimulationEngine, name: &str, val: u64) {
        let id = engine
            .design
            .top
            .signals
            .iter()
            .enumerate()
            .find(|(_, s)| s.name.as_str() == name)
            .map(|(i, _)| i)
            .unwrap();
        engine.state.write_signal(id, LogicVec::from_u64(val, 2));
    }

    #[test]
    fn apb_penable_without_psel_reported_once() {
        let mut engine = test_engine_with_signals(&["psel", "penable", "pready"]);
        let mut chk = VipChecker::new(VipProtocol::Apb);
        // t=0: psel=0, penable=1 -> pelanggaran
        set_signal(&mut engine, "psel", false);
        set_signal(&mut engine, "penable", true);
        set_signal(&mut engine, "pready", true);
        chk.check_time_step(0, &engine).unwrap();
        assert!(!chk.violations.is_empty());
        assert_eq!(chk.violations[0].rule, "APB-PENABLE-NO-SEL");
        // t=1: state sama -> tidak duplikasi
        chk.check_time_step(1, &engine).unwrap();
        assert_eq!(chk.violations.len(), 1);
    }

    #[test]
    fn apb_access_without_pready_reported() {
        let mut engine = test_engine_with_signals(&["psel", "penable", "pready"]);
        let mut chk = VipChecker::new(VipProtocol::Apb);
        // t=0: access aktif, pready=0
        set_signal(&mut engine, "psel", true);
        set_signal(&mut engine, "penable", true);
        set_signal(&mut engine, "pready", false);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: access berakhir (penable turun) tanpa pready -> pelanggaran
        set_signal(&mut engine, "penable", false);
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "APB-ACCESS-ABORTED"));
    }

    #[test]
    fn apb_clean_no_violation() {
        let mut engine = test_engine_with_signals(&["psel", "penable", "pready"]);
        let mut chk = VipChecker::new(VipProtocol::Apb);
        // t=0: setup access dengan pready=1
        set_signal(&mut engine, "psel", true);
        set_signal(&mut engine, "penable", true);
        set_signal(&mut engine, "pready", true);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: access selesai dengan pready=1 -> bersih
        set_signal(&mut engine, "penable", false);
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk.violations.is_empty());
    }

    #[test]
    fn axi4lite_bvalid_without_request_reported() {
        let mut engine = test_engine_with_signals(&[
            "awvalid", "awready", "wvalid", "wready", "arvalid", "arready",
            "bvalid", "bready", "rvalid", "rready",
        ]);
        let mut chk = VipChecker::new(VipProtocol::Axi4Lite);
        // t=0: bvalid=1 tanpa outstanding AW -> pelanggaran
        set_signal(&mut engine, "bvalid", true);
        set_signal(&mut engine, "awvalid", false);
        chk.check_time_step(0, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AXIL-BVALID-NO-REQUEST"));
    }

    #[test]
    fn axi4lite_clean_handshake_no_violation() {
        let mut engine = test_engine_with_signals(&[
            "awvalid", "awready", "wvalid", "wready", "arvalid", "arready",
            "bvalid", "bready", "rvalid", "rready",
        ]);
        let mut chk = VipChecker::new(VipProtocol::Axi4Lite);
        // t=0: awvalid=1, awready=0 (request outstanding)
        set_signal(&mut engine, "awvalid", true);
        set_signal(&mut engine, "awready", false);
        chk.check_time_step(0, &engine).unwrap();
        assert_eq!(chk.aw_outstanding, 1);
        assert!(chk.violations.is_empty());
        // t=1: awready=1 -> handshake, outstanding=0
        set_signal(&mut engine, "awready", true);
        chk.check_time_step(1, &engine).unwrap();
        assert_eq!(chk.aw_outstanding, 0);
        assert!(chk.violations.is_empty());
        // t=2: awvalid turun, tanpa BVALID -> tetap bersih
        set_signal(&mut engine, "awvalid", false);
        set_signal(&mut engine, "awready", false);
        chk.check_time_step(2, &engine).unwrap();
        assert_eq!(chk.aw_outstanding, 0);
        assert!(chk.violations.is_empty());
    }

    #[test]
    fn ahb_seq_without_nonseq_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        // t=0: htrans=Nonseq (2 = 10b, bit[1]=1), hready=1
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", true);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: htrans tetap Nonseq, saw_nonseq sudah true -> tidak pelanggaran
        chk.check_time_step(1, &engine).unwrap();
        // t=2: htrans=0 (idle) -> reset saw_nonseq
        set_signal_u64(&mut engine, "htrans", 0);
        chk.check_time_step(2, &engine).unwrap();
        // t=3: htrans=3 (Seq=11b) tanpa nonseq setelah idle -> pelanggaran
        set_signal_u64(&mut engine, "htrans", 3);
        chk.check_time_step(3, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-SEQ-WITHOUT-NONSEQ"));
    }

    #[test]
    fn ahb_busy_not_seq_without_nonseq() {
        // BUSY (01b) punya bit[1]=0, jadi tidak boleh dianggap Seq/Nonseq
        // dan tidak boleh memicu AHB-SEQ-WITHOUT-NONSEQ.
        let mut engine = test_engine_with_signals(&["htrans", "hready"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        // t=0: htrans=Busy (1 = 01b), hready=1
        set_signal_u64(&mut engine, "htrans", 1);
        set_signal(&mut engine, "hready", true);
        chk.check_time_step(0, &engine).unwrap();
        assert!(chk.violations.is_empty());
        // t=1: htrans=Idle (0)
        set_signal_u64(&mut engine, "htrans", 0);
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk.violations.is_empty());
        // t=2: htrans=Seq (3 = 11b) setelah idle, tanpa NONSEQ -> pelanggaran
        set_signal_u64(&mut engine, "htrans", 3);
        chk.check_time_step(2, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-SEQ-WITHOUT-NONSEQ"));
    }

    #[test]
    fn ahb_transfer_no_ready_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        // t=0: htrans=Nonseq, hready=0 (transfer mulai, menunggu ready)
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", false);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: htrans=Idle, hready=0 (transfer berakhir tanpa ready) -> pelanggaran
        set_signal_u64(&mut engine, "htrans", 0);
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-TRANSFER-NO-READY"));
    }

    #[test]
    fn axi4lite_valid_withdrawn_before_ready_reported() {
        let mut engine = test_engine_with_signals(&[
            "awvalid", "awready", "wvalid", "wready", "arvalid", "arready",
            "bvalid", "bready", "rvalid", "rready",
        ]);
        let mut chk = VipChecker::new(VipProtocol::Axi4Lite);
        // t=0: awvalid=1, awready=0 (request pending)
        set_signal(&mut engine, "awvalid", true);
        set_signal(&mut engine, "awready", false);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: awvalid ditarik (0), awready=0 -> ditarik sebelum handshake
        set_signal(&mut engine, "awvalid", false);
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AXIL-AWVALID-WITHDRAWN"));
    }

    #[test]
    fn vip_report_and_flush() {
        let mut engine = test_engine_with_signals(&["psel", "penable", "pready"]);
        let mut chk = VipChecker::new(VipProtocol::Apb);
        set_signal(&mut engine, "psel", false);
        set_signal(&mut engine, "penable", true);
        chk.check_time_step(0, &engine).unwrap();
        let rep = chk.report();
        assert!(rep.contains("APB-PENABLE-NO-SEL"));
        assert_eq!(chk.violations.len(), 1);
        chk.flush_diagnostics(&mut engine);
        assert!(chk.violations.is_empty());
    }

    /// AHB HRESP ERROR response dilaporkan saat HREADY=1.
    #[test]
    fn ahb_hresp_error_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hresp"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        // t=0: NONSEQ, hready=0, hresp=OKAY
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", false);
        set_signal_u64(&mut engine, "hresp", 0); // OKAY
        chk.check_time_step(0, &engine).unwrap();
        assert!(chk.violations.is_empty());
        // t=1: hready=1, hresp=ERROR (01)
        set_signal(&mut engine, "hready", true);
        set_signal_u64(&mut engine, "hresp", 1); // ERROR
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-HRESP-ERROR"));
    }

    /// AHB HRESP RETRY response dilaporkan saat HREADY=1.
    #[test]
    fn ahb_hresp_retry_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hresp"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", false);
        set_signal_u64(&mut engine, "hresp", 0);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: hready=1, hresp=RETRY (10)
        set_signal(&mut engine, "hready", true);
        set_signal_u64(&mut engine, "hresp", 2); // RETRY
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-HRESP-RETRY"));
    }

    /// AHB HRESP SPLIT response dilaporkan saat HREADY=1.
    #[test]
    fn ahb_hresp_split_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hresp"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", false);
        set_signal_u64(&mut engine, "hresp", 0);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: hready=1, hresp=SPLIT (11)
        set_signal(&mut engine, "hready", true);
        set_signal_u64(&mut engine, "hresp", 3); // SPLIT
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-HRESP-SPLIT"));
    }

    /// AHB HRESP OKAY tidak melaporkan pelanggaran.
    #[test]
    fn ahb_hresp_okay_no_violation() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hresp"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", false);
        set_signal_u64(&mut engine, "hresp", 0);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: hready=1, hresp=OKAY
        set_signal(&mut engine, "hready", true);
        set_signal_u64(&mut engine, "hresp", 0);
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk.violations.is_empty());
    }

    /// AHB burst length melebihi HBURST INCR4 dilaporkan.
    #[test]
    fn ahb_burst_length_exceeded_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hburst"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        // t=0: NONSEQ, HBURST=INCR4 (011), hready=1
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", true);
        set_signal_u64(&mut engine, "hburst", 3); // INCR4
        chk.check_time_step(0, &engine).unwrap();
        // t=1: SEQ, hready=1 (burst count=2)
        set_signal_u64(&mut engine, "htrans", 3);
        chk.check_time_step(1, &engine).unwrap();
        // t=2: SEQ, hready=1 (burst count=3)
        chk.check_time_step(2, &engine).unwrap();
        // t=3: SEQ, hready=1 (burst count=4)
        chk.check_time_step(3, &engine).unwrap();
        // t=4: SEQ, hready=1 (burst count=5 > 4) -> pelanggaran
        chk.check_time_step(4, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-BURST-LENGTH-EXCEEDED"));
    }

    /// AHB burst length mismatch (berakhir sebelum HBURST) dilaporkan.
    #[test]
    fn ahb_burst_length_mismatch_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hburst"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        // t=0: NONSEQ, HBURST=INCR4 (011), hready=1
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", true);
        set_signal_u64(&mut engine, "hburst", 3); // INCR4
        chk.check_time_step(0, &engine).unwrap();
        // t=1: SEQ
        set_signal_u64(&mut engine, "htrans", 3);
        chk.check_time_step(1, &engine).unwrap();
        // t=2: IDLE (burst berakhir dengan count=2 < 4) -> pelanggaran
        set_signal_u64(&mut engine, "htrans", 0);
        chk.check_time_step(2, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-BURST-LENGTH-MISMATCH"));
    }

    /// AHB HMASTLOCK berubah selama burst dilaporkan.
    #[test]
    fn ahb_hmastlock_unstable_reported() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hmastlock"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        // t=0: NONSEQ, HMASTLOCK=0, hready=1
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", true);
        set_signal(&mut engine, "hmastlock", false);
        chk.check_time_step(0, &engine).unwrap();
        // t=1: SEQ, HMASTLOCK=1 (berubah!) -> pelanggaran
        set_signal_u64(&mut engine, "htrans", 3);
        set_signal(&mut engine, "hmastlock", true);
        chk.check_time_step(1, &engine).unwrap();
        assert!(chk
            .violations
            .iter()
            .any(|v| v.rule == "AHB-HMASTLOCK-UNSTABLE"));
    }

    /// AHB HMASTLOCK stabil selama burst tidak melaporkan pelanggaran.
    #[test]
    fn ahb_hmastlock_stable_no_violation() {
        let mut engine = test_engine_with_signals(&["htrans", "hready", "hmastlock"]);
        let mut chk = VipChecker::new(VipProtocol::Ahb);
        set_signal_u64(&mut engine, "htrans", 2);
        set_signal(&mut engine, "hready", true);
        set_signal(&mut engine, "hmastlock", true);
        chk.check_time_step(0, &engine).unwrap();
        set_signal_u64(&mut engine, "htrans", 3);
        chk.check_time_step(1, &engine).unwrap();
        set_signal_u64(&mut engine, "htrans", 0); // IDLE
        chk.check_time_step(2, &engine).unwrap();
        assert!(chk.violations.is_empty());
    }
}
