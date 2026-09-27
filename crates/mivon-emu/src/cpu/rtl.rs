//! RTL-linked CPU — EMULATOR.md §7.2 mode 3.
//!
//! CPU berasal dari **RTL user** (.sv/.v), bukan model software Rust. Mesin
//! (register file, ALU, control) dijalankan oleh mivon RTL engine (Tier A);
//! sisi Rust hanya menyediakan memori (`MemoryPort`) + orkestrasi bus.
//!
//! Kontrak antarmuka bus (picorv32-style) yang wajib dipenuhi module CPU:
//! ```systemverilog
//! module cpu (
//!     input  clk, resetn,
//!     output mem_valid, mem_instr, [31:0] mem_addr,
//!     output [31:0] mem_wdata, output [3:0] mem_wstrb,
//!     input  mem_ready, input [31:0] mem_rdata,
//!     output trap
//! );
//! ```
//! - `mem_valid` di-assert CPU (registered) sampai transaksi selesai.
//! - Host menjawab dengan `mem_ready=1` (+ `mem_rdata` untuk read) selama
//!   fase high; CPU men-sampling di posedge berikutnya (`mem_xfer` =
//!   `mem_valid && mem_ready`) → transaksi selesai, `mem_valid` turun.
//! - `trap` tinggi saat ebreak/ecall/instruksi ilegal → `CpuStep::Trap`.

use std::collections::VecDeque;

use mivon_ir::{IrDesign, LogicVec, SignalId};
use mivon_simulator::debugger::Debugger;
use mivon_simulator::simulator::checkpoint::SimCheckpoint;
use mivon_simulator::simulator::{
    DebugMode, EventKind, EventRegion, RegionEvent, SimulationEngine, SimulationLimit, StepMode,
};

use super::{CpuCore, CpuFault, CpuStep, Isa};
use crate::mem::MemoryPort;

/// Versi blob snapshot Direct RTL CPU (`CpuCore::snapshot`) — bump bila
/// layout field berubah.
const RTL_SNAPSHOT_VER: u8 = 1;

/// Batas cycle RTL per `step()` (guard: CPU macet / reset tidak selesai).
const MAX_CYCLES_PER_STEP: u64 = 10_000;
/// Trap cause untuk ecall (mode machine) — sama dengan interpreter.
const CAUSE_ECALL_M: u64 = 11;

/// Id sinyal port bus CPU RTL (di-resolve dari module top flattened).
#[derive(Debug, Clone)]
struct CpuPorts {
    clk: SignalId,
    resetn: SignalId,
    mem_valid: SignalId,
    mem_instr: SignalId,
    mem_addr: SignalId,
    mem_wdata: SignalId,
    mem_wstrb: SignalId,
    mem_ready: SignalId,
    mem_rdata: SignalId,
    trap: SignalId,
    /// Direct RTL Device: decode MMIO di RTL (opsional — None = RAM-only).
    mmio_sel: Option<SignalId>,
    /// Console UART RTL (opsional): tx_done pulse + byte untuk host.
    uart_tx_done: Option<SignalId>,
    uart_tx_byte: Option<SignalId>,
    /// UART RX (input host → device RTL, opsional): strobe + byte input.
    uart_rx_wr: Option<SignalId>,
    uart_rx_din: Option<SignalId>,
    uart_rx_pending: Option<SignalId>,
    /// Probe PC internal CPU (opsional, picorv32-specific `u_cpu.reg_pc`).
    /// Dipakai `pc()` — lebih akurat daripada alamat fetch terakhir host,
    /// karena picorv32 mem-prefetch 1 instruksi melewati ebreak/trap.
    reg_pc: Option<SignalId>,
}

/// CPU RTL-linked: eksekusi instruksi terjadi di RTL (`.sv`/`.v`), bukan .rs.
pub struct RtlLinkedCpu {
    /// Design RTL CPU (wrapper + file CPU) hasil elaborasi.
    design: IrDesign,
    /// Engine RTL dengan mode step (1 time unit per `step_cycle`).
    dbg: Debugger,
    ports: CpuPorts,
    /// Total time unit RTL yang sudah dikonsumsi.
    cycle: u64,
    /// PC instruksi terakhir yang selesai di-fetch (fetch terakhir).
    last_pc: u64,
    /// Transaksi bus yang sedang dilayani (menunggu posedge berikutnya).
    served: bool,
    served_instr: bool,
    served_addr: u64,
    /// Transaksi yang dilayani adalah MMIO (decoder RTL yang ack — host
    /// tidak men-drive mem_ready; penyelesaian dideteksi dari mem_valid turun).
    served_mmio: bool,
    /// Byte yang ditulis CPU ke Direct RTL Device (UART console) selama run.
    pub console_out: Vec<u8>,
    /// Input host → device RTL (UART RX): byte di-deliver ke `rx_wr`/`rx_din`
    /// saat `rx_pending` UART kosong (satu byte per read-clear).
    pending_rx: VecDeque<u8>,
    /// rx_wr sedang tinggi (tunggu 1 posedge agar edge-detect UART latch,
    /// lalu drop ke 0 agar byte berikutnya bisa membuat edge baru).
    rx_wr_high: bool,
}

/// Compile file RTL CPU (bisa lebih dari satu — wrapper + picorv32.v) → IrDesign.
fn compile_cpu_rtl(files: &[String], top: &str) -> Result<IrDesign, String> {
    if files.is_empty() {
        return Err("--rtl-cpu: tidak ada file RTL CPU".into());
    }
    let mut combined = String::new();
    for path in files {
        let mut pp = mivon_parser::preprocessor::Preprocessor::new();
        let processed = pp
            .preprocess_file(path)
            .map_err(|e| format!("{}: {}", path, e))?;
        combined.push_str(&format!("`line 1 \"{}\"\n", path));
        combined.push_str(&processed);
        combined.push('\n');
    }
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let preprocessed = pp
        .preprocess(&combined, None)
        .map_err(|e| format!("preprocessor: {}", e))?;
    let mut lexer = mivon_parser::lexer::Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let file_line_map = lexer.file_line_map.clone();
    let first_source = if file_line_map.is_empty() {
        "<rtl-cpu>".to_string()
    } else {
        file_line_map[0].2.clone()
    };
    let mut parser = mivon_parser::Parser::new(tokens, &first_source)
        .with_source_lines(&preprocessed)
        .with_file_line_map(file_line_map);
    let design = parser.parse_design().map_err(|e| e.to_string())?;
    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, first_source);
    let ir = elaborator
        .elaborate(
            Some(top),
            mivon_elaboration::ElaborateMode::StrictSimulation,
        )
        .map_err(|e| e.to_string())?;
    Ok(ir)
}

/// Resolve id sinyal port bus dari module top.
fn resolve_ports(design: &IrDesign) -> Result<CpuPorts, String> {
    let find = |name: &str| {
        design
            .top
            .signals
            .iter()
            .position(|s| s.name.as_str() == name)
    };
    macro_rules! need {
        ($($f:ident = $n:literal),* $(,)?) => {
            $(
                let $f = find($n).ok_or_else(|| {
                    format!(
                        "port '{}' tidak ditemukan di module top '{}' — CPU RTL harus punya antarmuka bus (clk, resetn, mem_valid, mem_instr, mem_addr, mem_wdata, mem_wstrb, mem_ready, mem_rdata, trap)",
                        $n, design.top.name.as_str()
                    )
                })?;
            )*
        };
    }
    need!(
        clk = "clk",
        resetn = "resetn",
        mem_valid = "mem_valid",
        mem_instr = "mem_instr",
        mem_addr = "mem_addr",
        mem_wdata = "mem_wdata",
        mem_wstrb = "mem_wstrb",
        mem_ready = "mem_ready",
        mem_rdata = "mem_rdata",
        trap = "trap",
    );
    Ok(CpuPorts {
        clk,
        resetn,
        mem_valid,
        mem_instr,
        mem_addr,
        mem_wdata,
        mem_wstrb,
        mem_ready,
        mem_rdata,
        trap,
        // Opsional: Direct RTL Device (MMIO decode + UART console di RTL).
        // Tidak ada di top → None (perilaku RAM-only, kompatibel ke belakang).
        mmio_sel: find("mmio_sel"),
        uart_tx_done: find("uart_tx_done"),
        uart_tx_byte: find("uart_tx_byte"),
        uart_rx_wr: find("uart_rx_wr"),
        uart_rx_din: find("uart_rx_din"),
        uart_rx_pending: find("uart_rx_pending"),
        reg_pc: find("u_cpu.reg_pc"),
    })
}

impl RtlLinkedCpu {
    /// Kompilasi file RTL CPU + resolve port bus. Belum reset — panggil
    /// `reset()` sebelum `step()`.
    pub fn from_files(files: &[String], top: &str) -> Result<Self, String> {
        let design = compile_cpu_rtl(files, top)?;
        let ports = resolve_ports(&design)?;
        let mut cpu = Self {
            design,
            dbg: Debugger {
                engine: SimulationEngine::new_with_limit(
                    // placeholder — diganti di reset()
                    mivon_ir::IrDesign::default(),
                    SimulationLimit::Unlimited,
                ),
            },
            ports,
            cycle: 0,
            last_pc: 0,
            served: false,
            served_instr: false,
            served_addr: 0,
            served_mmio: false,
            console_out: Vec::new(),
            pending_rx: VecDeque::new(),
            rx_wr_high: false,
        };
        cpu.recreate_engine();
        Ok(cpu)
    }

    /// Baca sinyal internal CPU RTL (debug/probe). Nama hierarkis (`u_cpu.reg_pc`).
    pub fn read_signal(&self, name: &str) -> Option<u64> {
        let id = self
            .design
            .top
            .signals
            .iter()
            .position(|s| s.name.as_str() == name)?;
        Some(self.dbg.engine.state.read_signal(id).to_u64())
    }

    /// Nama semua sinyal top design (debug/probe flatten naming).
    pub fn signal_names(&self) -> Vec<String> {
        self.design
            .top
            .signals
            .iter()
            .map(|s| s.name.as_str().to_string())
            .collect()
    }

    /// Lebar sinyal top design (debug/probe).
    pub fn signal_width(&self, name: &str) -> Option<usize> {
        let id = self
            .design
            .top
            .signals
            .iter()
            .position(|s| s.name.as_str() == name)?;
        Some(self.design.top.signals[id].width)
    }

    /// Satu tick clock (high+low) DENGAN layanan bus — untuk probing manual
    /// per cycle. Tidak menunggu fetch selesai (beda dari `step()`).
    pub fn debug_tick(&mut self, mem: &mut dyn MemoryPort) -> Result<(), CpuFault> {
        self.tick(Some(mem)).map(|_| ())
    }

    /// Engine baru + zero-kan SEMUA sinyal (hindari X di input bus / regfile).
    fn recreate_engine(&mut self) {
        let mut engine =
            SimulationEngine::new_with_limit(self.design.clone(), SimulationLimit::Unlimited);
        engine.debug_mode = DebugMode::Debug;
        engine.step_mode = StepMode::StepCycle;
        for id in 0..engine.state.signals.len() {
            let width = engine.design.top.signals[id].width;
            engine.state.write_signal(id, LogicVec::from_u64(0, width));
        }
        self.dbg = Debugger { engine };
        self.cycle = 0;
        self.served = false;
        self.served_mmio = false;
        self.last_pc = 0;
        self.console_out.clear();
        self.pending_rx.clear();
        self.rx_wr_high = false;
    }

    /// Reset: engine baru, resetn aktif-rendah beberapa cycle, lalu lepas.
    fn drive_reset(&mut self) {
        for _ in 0..4 {
            let _ = self.tick(None);
        }
        self.poke(self.ports.resetn, 1);
    }

    /// Tulis sinyal 1-bit (level input: resetn/mem_ready — nilai dibaca saat
    /// posedge berikutnya; tidak perlu event karena tidak mendeteksi edge).
    fn poke(&mut self, id: SignalId, val: u64) {
        self.dbg
            .engine
            .state
            .write_signal(id, LogicVec::from_u64(val, 1));
    }

    /// Tulis sinyal DI DALAM time step via event terjadwal (`SdfDelayedWrite`
    /// — jalur commit write tertunda engine). PENTING untuk clock: tulis
    /// langsung (`state.write_signal`) SEBELUM `step_cycle()` tidak pernah
    /// terdeteksi sebagai edge — engine mengambil snapshot Preponed di AWAL
    /// time step (nilai sudah berubah) sehingga `@(posedge clk)` tidak pernah
    /// membangunkan proses Sequential. Event yang diproses di region Active
    /// menulis nilai saat delta berjalan; snapshot awal delta masih nilai lama
    /// → transisi 0→1/1→0 terdeteksi benar (pola sama dengan `#1 clk = ~clk`).
    fn schedule_write(&mut self, id: SignalId, val: LogicVec) {
        let t = self.dbg.engine.state.time as usize;
        self.dbg.engine.push_event(
            t,
            RegionEvent {
                region: EventRegion::Active,
                event: EventKind::SdfDelayedWrite {
                    sig_id: id,
                    value: val,
                },
            },
        );
    }

    fn poke_val(&mut self, id: SignalId, val: u64) {
        let width = self.dbg.engine.design.top.signals[id].width;
        self.dbg
            .engine
            .state
            .write_signal(id, LogicVec::from_u64(val, width));
    }

    /// Baca sinyal 1-bit sebagai bool (X → false).
    fn bit(&self, id: SignalId) -> bool {
        self.dbg
            .engine
            .state
            .read_signal(id)
            .to_bool()
            .unwrap_or(false)
    }

    /// Baca sinyal sebagai u64 (X → 0).
    fn val(&self, id: SignalId) -> u64 {
        self.dbg.engine.state.read_signal(id).to_u64()
    }

    /// Satu langkah engine RTL (1 time unit; posedge/negedge dipicu oleh poke).
    fn step_once(&mut self) -> Result<(), CpuFault> {
        self.cycle += 1;
        self.dbg.step_cycle().map_err(|e| CpuFault {
            pc: self.pc(),
            reason: format!("rtl step: {}", e),
        })
    }

    /// Satu cycle clock penuh (high + low). Bila `mem` diberikan, layani
    /// transaksi bus yang sedang di-assert CPU.
    ///
    /// Mengembalikan Ok(true) bila SATU fetch instruksi selesai pada tick ini
    /// (= satu instruksi dieksekusi RTL).
    ///
    /// Protokol: picorv32 memakai stream valid/ready — `mem_valid` TETAP
    /// tinggi untuk transaksi back-to-back (tidak selalu turun antar txn).
    /// Karena itu penyelesaian transaksi non-MMIO dideteksi dari posedge
    /// berikutnya setelah serve (ready yang kita poke di-sample CPU), bukan
    /// dari mem_valid turun. MMIO (decoder RTL yang ack) tetap memakai
    /// deteksi mem_valid turun.
    fn tick(&mut self, mem: Option<&mut dyn MemoryPort>) -> Result<bool, CpuFault> {
        // ── High phase (posedge): CPU men-sampling mem_ready/mem_rdata ──
        self.schedule_write(self.ports.clk, LogicVec::from_u64(1, 1));
        self.step_once()?;
        let mut fetch_done = false;

        // Transaksi non-MMIO yang dilayani tick sebelumnya: posedge barusan
        // men-sample ready/rdata → transaksi SELESAI. Tutup sebelum melayani
        // transaksi baru agar back-to-back tetap berjalan.
        if self.served && !self.served_mmio {
            let was_fetch = self.served_instr;
            let addr = self.served_addr;
            self.served = false;
            self.served_mmio = false;
            self.poke(self.ports.mem_ready, 0);
            self.poke_val(self.ports.mem_rdata, 0);
            if was_fetch {
                // Fetch instruksi selesai = satu instruksi dieksekusi RTL.
                self.last_pc = addr;
                fetch_done = true;
            }
        }
        // Transaksi baru di-assert CPU (termasuk back-to-back setelah
        // penyelesaian di atas) → layani sekarang; selesai di posedge
        // berikutnya.
        if let Some(mem) = mem {
            if !self.served && self.bit(self.ports.mem_valid) {
                self.serve(mem)?;
            }
        }
        // ── Low phase ──
        self.schedule_write(self.ports.clk, LogicVec::from_u64(0, 1));
        self.step_once()?;
        Ok(fetch_done)
    }

    /// Layani transaksi bus CPU: baca → mem_rdata; tulis → mem.write per strobe.
    /// Transaksi MMIO (decode di RTL, `mmio_sel`) TIDAK dijawab host — decoder
    /// RTL memberi ack (`mem_ready` OR internal); host hanya menandai txn agar
    /// completion logic tahu kapan selesai.
    fn serve(&mut self, mem: &mut dyn MemoryPort) -> Result<(), CpuFault> {
        let addr = self.val(self.ports.mem_addr);
        let wstrb = self.val(self.ports.mem_wstrb) as u32;
        self.served_instr = self.bit(self.ports.mem_instr);
        self.served_addr = addr;
        self.served = true;
        let is_mmio = self.ports.mmio_sel.map(|id| self.bit(id)).unwrap_or(false);
        self.served_mmio = is_mmio;
        if is_mmio {
            // Direct RTL Device: RTL decoder yang ack. Host tidak men-drive
            // mem_ready/mem_rdata (hindari multi-driver). Byte UART ditangkap
            // di step() via uart_tx_done/uart_tx_byte.
            return Ok(());
        }
        if wstrb == 0 {
            // Read (fetch / load): 4 byte word-aligned.
            let data = mem.read(addr, 4).unwrap_or(0);
            self.poke_val(self.ports.mem_rdata, data);
        } else {
            // Write (store): strobe byte-enable.
            let wdata = self.val(self.ports.mem_wdata);
            for i in 0..4u32 {
                if (wstrb >> i) & 1 != 0 {
                    let _ = mem.write(addr + i as u64, 1, (wdata >> (8 * i)) & 0xff);
                }
            }
        }
        self.poke(self.ports.mem_ready, 1);
        Ok(())
    }

    /// Tangkap byte UART RTL (tx_done pulse) → console host.
    fn capture_console(&mut self) {
        let done = self
            .ports
            .uart_tx_done
            .map(|id| self.bit(id))
            .unwrap_or(false);
        if done {
            if let Some(byte_id) = self.ports.uart_tx_byte {
                let b = self.val(byte_id) as u8;
                self.console_out.push(b);
            }
        }
    }
}

impl Default for RtlLinkedCpu {
    fn default() -> Self {
        // Hanya untuk test-token; `from_files` adalah konstruktor utama.
        let mut cpu = Self {
            design: mivon_ir::IrDesign::default(),
            dbg: Debugger {
                engine: SimulationEngine::new_with_limit(
                    mivon_ir::IrDesign::default(),
                    SimulationLimit::Unlimited,
                ),
            },
            ports: CpuPorts {
                clk: 0,
                resetn: 0,
                mem_valid: 0,
                mem_instr: 0,
                mem_addr: 0,
                mem_wdata: 0,
                mem_wstrb: 0,
                mem_ready: 0,
                mem_rdata: 0,
                trap: 0,
                mmio_sel: None,
                uart_tx_done: None,
                uart_tx_byte: None,
                uart_rx_wr: None,
                uart_rx_din: None,
                uart_rx_pending: None,
                reg_pc: None,
            },
            cycle: 0,
            last_pc: 0,
            served: false,
            served_instr: false,
            served_addr: 0,
            served_mmio: false,
            console_out: Vec::new(),
            pending_rx: VecDeque::new(),
            rx_wr_high: false,
        };
        cpu.recreate_engine();
        cpu
    }
}

impl RtlLinkedCpu {
    /// Input host → device RTL (UART RX): antre byte; di-deliver ke `rx_wr`/
    /// `rx_din` saat UART RTL kosong (`rx_pending` = 0) — satu byte per
    /// read-clear. Menjadikan device bidirectional (console input).
    pub fn push_uart_input(&mut self, bytes: &[u8]) {
        self.pending_rx.extend(bytes.iter().copied());
    }

    /// Deliver byte input berikutnya ke port RX RTL: naikkan `rx_wr` (level,
    /// `rx_wr_high` ditandai — step() menurunkannya 1 posedge setelah latch
    /// UART agar byte berikutnya membuat edge baru).
    fn deliver_rx_byte(&mut self) {
        let (Some(din), Some(wr)) = (self.ports.uart_rx_din, self.ports.uart_rx_wr) else {
            return;
        };
        if let Some(b) = self.pending_rx.pop_front() {
            self.poke_val(din, b as u64);
            self.poke(wr, 1);
            self.rx_wr_high = true;
        }
    }

    /// UART RTL siap menerima byte berikutnya (pending kosong).
    fn uart_rx_idle(&self) -> bool {
        self.ports
            .uart_rx_pending
            .map(|id| !self.bit(id))
            .unwrap_or(false)
    }
}

impl CpuCore for RtlLinkedCpu {
    fn reset(&mut self) {
        self.recreate_engine();
        self.drive_reset();
    }

    fn step(&mut self, mem: &mut dyn MemoryPort) -> Result<CpuStep, CpuFault> {
        let start = self.cycle;
        for _ in 0..MAX_CYCLES_PER_STEP {
            let fetch_done = self.tick(Some(mem))?;
            // Direct RTL Device: byte UART RTL → console host.
            self.capture_console();
            // UART RX handshake: drop rx_wr 1 posedge setelah latch (edge
            // berikutnya terdeteksi UART), lalu deliver byte berikutnya saat
            // UART kosong (rx_pending = 0, read-clear oleh CPU).
            if self.rx_wr_high {
                if let Some(wr) = self.ports.uart_rx_wr {
                    self.poke(wr, 0);
                }
                self.rx_wr_high = false;
            }
            if !self.pending_rx.is_empty() && self.uart_rx_idle() {
                self.deliver_rx_byte();
            }
            // Trap (ebreak/ecall/ilegal) — CPU berhenti.
            if self.bit(self.ports.trap) {
                return Ok(CpuStep::Trap {
                    cause: CAUSE_ECALL_M,
                    tval: self.last_pc,
                });
            }
            if fetch_done {
                return Ok(CpuStep::InstructionExecuted {
                    cycles: self.cycle - start,
                });
            }
            // Transaksi MMIO: RTL decoder yang ack — selesai saat mem_valid
            // turun (host tidak men-drive ready).
            if self.served && self.served_mmio && !self.bit(self.ports.mem_valid) {
                let was_fetch = self.served_instr;
                let addr = self.served_addr;
                self.served = false;
                self.served_mmio = false;
                if was_fetch {
                    self.last_pc = addr;
                    return Ok(CpuStep::InstructionExecuted {
                        cycles: self.cycle - start,
                    });
                }
            }
        }
        Err(CpuFault {
            pc: self.pc(),
            reason: "RTL CPU tidak progress (max cycle per step)".into(),
        })
    }

    fn pc(&self) -> u64 {
        // Probe PC internal RTL bila tersedia (picorv32 `u_cpu.reg_pc`) —
        // akurat saat trap: picorv32 mem-prefetch 1 instruksi melewati ebreak
        // sehingga alamat fetch terakhir host = ebreak + 4.
        if let Some(id) = self.ports.reg_pc {
            return self.dbg.engine.state.read_signal(id).to_u64();
        }
        self.last_pc
    }

    fn set_pc(&mut self, _addr: u64) {
        // PC dikontrol RTL (PROGADDR_RESET / PROGADDR_IRQ) — tidak bisa
        // di-set dari host pada CPU RTL.
    }

    fn raise_interrupt(&mut self, _irq: u32, _level: bool) {
        // R4: wire ke port `irq` RTL. Belum diimplementasikan.
    }

    fn read_reg(&self, _idx: usize) -> u64 {
        // Register file internal RTL; tanpa RVFI port tidak bisa dibaca.
        0
    }

    fn isa(&self) -> Isa {
        Isa::RiscV32
    }

    /// Byte yang ditulis CPU RTL ke Direct RTL Device (UART console).
    fn console_output(&self) -> &[u8] {
        &self.console_out
    }

    // ── Snapshot (EMULATOR.md §14, R5) ──
    // State engine RTL (sinyal/waktu/RNG/process map/UVM/coverage/signal
    // history via `SimCheckpoint` SIM-17/18) + state bus & host-side CPU.
    // Antrean runtime (event/NBA/wait/fork) TIDAK ikut checkpoint → snapshot
    // DITOLAK bila engine belum idle; restore memakai `restore_checkpoint`
    // yang me-reset antrean tsb bersih.

    fn snapshot(&self) -> Result<Vec<u8>, String> {
        if let Some(queue) = self.dbg.engine.pending_runtime_events() {
            return Err(format!(
                "rtl-cpu snapshot: engine belum idle — antrean '{}' terisi \
                 (hanya aman antar langkah)",
                queue
            ));
        }
        let mut w = crate::snapshot::Writer::new();
        w.u8(RTL_SNAPSHOT_VER);
        w.u64(self.cycle);
        w.u64(self.last_pc);
        w.u64(self.served_addr);
        w.u8(self.served as u8);
        w.u8(self.served_instr as u8);
        w.u8(self.served_mmio as u8);
        w.u8(self.rx_wr_high as u8);
        w.blob(&self.console_out);
        w.u32(self.pending_rx.len() as u32);
        for b in &self.pending_rx {
            w.u8(*b);
        }
        let mut eng = Vec::new();
        self.dbg
            .engine
            .checkpoint()
            .serialize(&mut eng)
            .map_err(|e| format!("rtl-cpu snapshot: engine checkpoint: {}", e))?;
        w.blob(&eng);
        Ok(w.buf)
    }

    fn restore(&mut self, blob: &[u8]) -> Result<(), String> {
        let mut r = crate::snapshot::Reader::new(blob);
        let ver = r.u8()?;
        if ver != RTL_SNAPSHOT_VER {
            return Err(format!(
                "rtl-cpu snapshot: versi {} (didukung {})",
                ver, RTL_SNAPSHOT_VER
            ));
        }
        // Baca SEMUA dulu → state hanya berubah bila blob lengkap.
        let cycle = r.u64()?;
        let last_pc = r.u64()?;
        let served_addr = r.u64()?;
        let served = r.u8()? != 0;
        let served_instr = r.u8()? != 0;
        let served_mmio = r.u8()? != 0;
        let rx_wr_high = r.u8()? != 0;
        let console_out = r.blob()?.to_vec();
        let n = r.u32()? as usize;
        if n > 65_536 {
            return Err(format!(
                "rtl-cpu snapshot: pending_rx {} byte tidak wajar",
                n
            ));
        }
        let mut pending_rx = VecDeque::with_capacity(n);
        for _ in 0..n {
            pending_rx.push_back(r.u8()?);
        }
        let eng = r.blob()?.to_vec();
        let cp = SimCheckpoint::deserialize(&mut eng.as_slice())
            .map_err(|e| format!("rtl-cpu snapshot: engine checkpoint: {}", e))?;
        // Engine dulu (gagal = state bus/host belum disentuh), lalu state CPU.
        // Target engine harus dari design YANG SAMA (signals count dicek).
        self.dbg
            .engine
            .restore_checkpoint(cp)
            .map_err(|e| format!("rtl-cpu snapshot: engine restore: {}", e))?;
        self.cycle = cycle;
        self.last_pc = last_pc;
        self.served_addr = served_addr;
        self.served = served;
        self.served_instr = served_instr;
        self.served_mmio = served_mmio;
        self.rx_wr_high = rx_wr_high;
        self.console_out = console_out;
        self.pending_rx = pending_rx;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::{MemoryMap, RamRegion, RegionKind};
    use mivon_core::intern::Symbol;

    /// Elaborasi picorv32.v rekursif dalam — sama seperti main.rs / mivon-tests,
    /// compile design besar butuh stack jauh di atas 2MB default thread test.
    fn with_big_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap();
    }

    fn root_of(rel: &str) -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .to_path_buf();
        root.join(rel).to_string_lossy().to_string()
    }

    fn cpu_files() -> Vec<String> {
        vec![
            root_of("examples/rtl/rv32_bus_wrapper.sv"),
            root_of("examples/rtl/picorv32.v"),
        ]
    }

    /// SoC lengkap: picorv32 + UART + timer (semua Direct RTL Device).
    fn soc_files() -> Vec<String> {
        vec![
            root_of("examples/rtl/rv32_soc.sv"),
            root_of("examples/rtl/uart_console.sv"),
            root_of("examples/rtl/timer_console.sv"),
            root_of("examples/rtl/picorv32.v"),
        ]
    }

    fn map() -> MemoryMap {
        let mut m = MemoryMap::new();
        m.add(
            RamRegion::new(
                Symbol::intern("ram"),
                0x8000_0000,
                0x1_0000,
                RegionKind::Ram,
                false,
            )
            .unwrap(),
        )
        .unwrap();
        m
    }

    /// Bangun ELF64 LE minimal (1 segmen PT_LOAD) di 0x80000000.
    fn make_elf(payload: &[u8]) -> Vec<u8> {
        let entry = 0x8000_0000u64;
        let vaddr = 0x8000_0000u64;
        let mut d = vec![0u8; 64 + 56];
        d[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        d[4] = 2; // ELF64
        d[5] = 1; // LE
        d[6] = 1;
        d[18..20].copy_from_slice(&243u16.to_le_bytes()); // RISC-V
        d[24..32].copy_from_slice(&entry.to_le_bytes());
        d[32..40].copy_from_slice(&64u64.to_le_bytes());
        d[54..56].copy_from_slice(&56u16.to_le_bytes());
        d[56..58].copy_from_slice(&1u16.to_le_bytes());
        let po = 64usize;
        d[po..po + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        d[po + 8..po + 16].copy_from_slice(&(64u64 + 56).to_le_bytes());
        d[po + 16..po + 24].copy_from_slice(&vaddr.to_le_bytes());
        d[po + 32..po + 40].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        d[po + 40..po + 48].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        d.extend_from_slice(payload);
        d
    }

    // ── Encoder RV32I (sama dengan interpreter) ──
    fn r(f7: u8, rs2: u8, rs1: u8, f3: u8, rd: u8, op: u32) -> u32 {
        ((f7 as u32) << 25)
            | ((rs2 as u32) << 20)
            | ((rs1 as u32) << 15)
            | ((f3 as u32) << 12)
            | ((rd as u32) << 7)
            | op
    }
    fn i(imm: i32, rs1: u8, f3: u8, rd: u8, op: u32) -> u32 {
        ((imm as u32 & 0xfff) << 20)
            | ((rs1 as u32) << 15)
            | ((f3 as u32) << 12)
            | ((rd as u32) << 7)
            | op
    }
    fn s(imm: i32, rs2: u8, rs1: u8, f3: u8, op: u32) -> u32 {
        let imm = imm as u32 & 0xfff;
        ((imm >> 5) << 25)
            | ((rs2 as u32) << 20)
            | ((rs1 as u32) << 15)
            | ((f3 as u32) << 12)
            | ((imm & 0x1f) << 7)
            | op
    }
    const ADDI: u32 = 0x13;
    const LUI: u32 = 0x37;
    const SW: u32 = 0x23;
    const LW: u32 = 0x03;
    const OP: u32 = 0x33;

    fn code_bytes(words: &[u32]) -> Vec<u8> {
        let mut v = Vec::with_capacity(words.len() * 4);
        for w in words {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v
    }

    #[test]
    fn test_rtl_cpu_compile_and_ports() {
        with_big_stack(|| {
            let cpu = RtlLinkedCpu::from_files(&cpu_files(), "rv32_bus_wrapper").expect("compile");
            assert_eq!(cpu.design.top.name.as_str(), "rv32_bus_wrapper");
            assert_eq!(cpu.ports.clk, cpu.ports.clk); // resolved tanpa panic
            assert!(cpu.ports.mem_addr > 0);
        });
    }

    /// E2E: CPU (dari picorv32.v) mengeksekusi program bare-metal yang dimuat
    /// via ELF loader — bukan interpreter Rust. Verifikasi hasil di RAM.
    #[test]
    fn test_rtl_cpu_runs_elf_program() {
        with_big_stack(|| {
            // t0=42; a0=0x80000000; sw t0,0(a0); lw a1,0(a0); a2=a1+t0; sw a2,4(a0); ebreak
            let code = [
                i(42, 0, 0, 5, ADDI),                    // t0 = 42
                (0x80000u32 << 12) | (10u32 << 7) | LUI, // a0 = 0x80000000
                s(0, 5, 10, 2, SW),                      // mem[a0+0] = t0
                i(0, 10, 2, 11, LW),                     // a1 = mem[a0]
                r(0, 5, 11, 0, 12, OP),                  // a2 = a1 + t0 = 84
                s(4, 12, 10, 2, SW),                     // mem[a0+4] = a2
                0x0010_0073,                             // ebreak
            ];
            let elf = make_elf(&code_bytes(&code));
            let mut mem = map();
            crate::elf::load_elf(&elf, &mut mem).expect("load elf");

            let mut cpu =
                RtlLinkedCpu::from_files(&cpu_files(), "rv32_bus_wrapper").expect("compile");
            cpu.reset();
            let mut machine = crate::machine::Machine::new(Box::new(cpu), mem, 5000);
            let result = machine.run().expect("machine run");
            assert!(result.halted, "CPU RTL harus trap (ebreak): {:?}", result);
            // pc() = alamat fetch terakhir = ebreak (instruksi terakhir program).
            assert_eq!(
                result.pc & !3,
                0x8000_0018,
                "PC terakhir harus di ebreak (0x18)"
            );
            // Hasil komputasi RTL: a2 = 84 tersimpan di 0x80000004.
            let out = machine.mem.read(0x8000_0004, 4).expect("read");
            assert_eq!(out, 84, "CPU RTL harus menyimpan hasil 42+42=84");
            let stored = machine.mem.read(0x8000_0000, 4).expect("read");
            assert_eq!(stored, 42);
        });
    }

    #[test]
    fn test_rtl_cpu_snapshot_restore_resume_deterministic() {
        with_big_stack(|| {
            // Program sama dgn test ELF di atas: store 42 + hasil 84 ke RAM.
            let code = [
                i(42, 0, 0, 5, ADDI),                    // t0 = 42
                (0x80000u32 << 12) | (10u32 << 7) | LUI, // a0 = 0x80000000
                s(0, 5, 10, 2, SW),                      // mem[a0+0] = t0
                i(0, 10, 2, 11, LW),                     // a1 = mem[a0]
                r(0, 5, 11, 0, 12, OP),                  // a2 = a1 + t0 = 84
                s(4, 12, 10, 2, SW),                     // mem[a0+4] = a2
                0x0010_0073,                             // ebreak
            ];
            let bytes = code_bytes(&code);
            let fresh_mem = || {
                let mut m = map();
                let elf = make_elf(&bytes);
                crate::elf::load_elf(&elf, &mut m).expect("load elf");
                m
            };

            // ── Mesin A: 3 instruksi → snapshot → lanjut sampai ebreak ──
            let mut cpu_a =
                RtlLinkedCpu::from_files(&cpu_files(), "rv32_bus_wrapper").expect("compile");
            cpu_a.reset();
            let mut a = crate::machine::Machine::new(Box::new(cpu_a), fresh_mem(), 3);
            let r1 = a.run().expect("run awal");
            assert!(!r1.halted, "3 instr belum sampai ebreak");
            let snap = a.snapshot().expect("snapshot Direct RTL CPU");
            assert_eq!(snap.steps, 3, "counter kumulatif ikut snapshot");
            assert!(!snap.cpu.is_empty());

            a.max_steps = 5000;
            let r2 = a.run().expect("lanjut A");
            assert!(r2.halted, "lanjut → ebreak: {:?}", r2);
            let mem_a = a.mem.regions[0].bytes().to_vec();
            assert_eq!(a.mem.read(0x8000_0000, 4).unwrap(), 42);
            assert_eq!(a.mem.read(0x8000_0004, 4).unwrap(), 84);

            // ── Mesin B: CPU RTL baru + RAM kosong → restore → run sama ──
            let mut cpu_b =
                RtlLinkedCpu::from_files(&cpu_files(), "rv32_bus_wrapper").expect("compile");
            cpu_b.reset();
            let mut b = crate::machine::Machine::new(Box::new(cpu_b), fresh_mem(), 5000);
            b.restore(&snap).expect("restore");
            let r3 = b.run().expect("run B");
            assert_eq!(r3.halted, r2.halted, "hasil halt sama");
            assert_eq!(r3.steps, r2.steps, "steps kumulatif identik");
            assert_eq!(r3.pc, r2.pc, "pc akhir identik");
            assert_eq!(
                b.mem.regions[0].bytes(),
                mem_a.as_slice(),
                "memori hasil eksekusi identik (deterministik)"
            );
            assert_eq!(
                b.cpu.console_output(),
                a.cpu.console_output(),
                "console RTL identik"
            );
        });
    }

    /// E2E Direct RTL Device (R4): top `rv32_soc` = picorv32 + `uart_console`
    /// (decode MMIO 0x10000000 di RTL). CPU menulis 'A','B','C' ke UART RTL
    /// → byte ditangkap host (console); store lain tetap ke RAM host.
    #[test]
    fn test_rtl_cpu_mmio_uart_console() {
        with_big_stack(|| {
            let soc_files = soc_files();
            // t0='A'/'B'/'C' → sw ke 0x10000000 (UART RTL); t0=42 → sw ke
            // 0x80000004 (RAM host); ebreak.
            let code = [
                i(0x41, 0, 0, 5, ADDI),                  // t0 = 'A'
                (0x10000u32 << 12) | (10u32 << 7) | LUI, // a0 = 0x10000000 (UART)
                s(0, 5, 10, 2, SW),                      // uart 'A'
                i(0x42, 0, 0, 5, ADDI),                  // t0 = 'B'
                s(0, 5, 10, 2, SW),                      // uart 'B'
                i(0x43, 0, 0, 5, ADDI),                  // t0 = 'C'
                s(0, 5, 10, 2, SW),                      // uart 'C'
                i(42, 0, 0, 5, ADDI),                    // t0 = 42
                (0x80000u32 << 12) | (11u32 << 7) | LUI, // a1 = 0x80000000 (RAM)
                s(4, 5, 11, 2, SW),                      // mem[0x80000004] = 42
                0x0010_0073,                             // ebreak
            ];
            let elf = make_elf(&code_bytes(&code));
            let mut mem = map();
            crate::elf::load_elf(&elf, &mut mem).expect("load elf");

            let mut cpu = RtlLinkedCpu::from_files(&soc_files, "rv32_soc").expect("compile");
            cpu.reset();
            let mut machine = crate::machine::Machine::new(Box::new(cpu), mem, 5000);
            let result = machine.run().expect("machine run");
            assert!(result.halted, "CPU RTL harus trap (ebreak): {:?}", result);
            assert_eq!(result.pc & !3, 0x8000_0028, "PC terakhir = ebreak (0x28)");
            // Byte UART RTL ditangkap host, urut.
            assert_eq!(
                result.console, b"ABC",
                "console dari UART RTL: {:?}",
                result.console
            );
            // Store non-MMIO tetap ke RAM host.
            let stored = machine.mem.read(0x8000_0004, 4).expect("read");
            assert_eq!(stored, 42, "store RAM biasa harus tetap bekerja");
        });
    }

    /// E2E Direct RTL Device — MMIO READ: `uart_console` punya status register
    /// (tx_count di UART_BASE+4, mux mem_rdata DI RTL). CPU baca status sebelum/
    /// sesudah tiap tulis, simpan hasil ke RAM host → verifikasi nilai read.
    #[test]
    fn test_rtl_cpu_mmio_read_status() {
        with_big_stack(|| {
            let soc_files = soc_files();
            // a0 = UART (0x10000000), a1 = RAM (0x80000000).
            // baca tx_count awal (0) → ram[0]; tulis 'A' → baca (1) → ram[4];
            // tulis 'B' → baca (2) → ram[8]; ebreak.
            let code = [
                (0x10000u32 << 12) | (10u32 << 7) | LUI, // a0 = UART base
                (0x80000u32 << 12) | (11u32 << 7) | LUI, // a1 = RAM base
                i(4, 10, 2, 5, LW),                      // t0 = status (0)
                s(0, 5, 11, 2, SW),                      // ram[0] = t0
                i(0x41, 0, 0, 5, ADDI),                  // t0 = 'A'
                s(0, 5, 10, 2, SW),                      // uart 'A'
                i(4, 10, 2, 6, LW),                      // t1 = status (1)
                s(4, 6, 11, 2, SW),                      // ram[4] = t1
                i(0x42, 0, 0, 5, ADDI),                  // t0 = 'B'
                s(0, 5, 10, 2, SW),                      // uart 'B'
                i(4, 10, 2, 7, LW),                      // t2 = status (2)
                s(8, 7, 11, 2, SW),                      // ram[8] = t2
                0x0010_0073,                             // ebreak
            ];
            let elf = make_elf(&code_bytes(&code));
            let mut mem = map();
            crate::elf::load_elf(&elf, &mut mem).expect("load elf");

            let mut cpu = RtlLinkedCpu::from_files(&soc_files, "rv32_soc").expect("compile");
            cpu.reset();
            let mut machine = crate::machine::Machine::new(Box::new(cpu), mem, 5000);
            let result = machine.run().expect("machine run");
            assert!(result.halted, "CPU RTL harus trap (ebreak): {:?}", result);
            assert_eq!(result.pc & !3, 0x8000_0030, "PC terakhir = ebreak (0x30)");
            assert_eq!(
                result.console, b"AB",
                "console UART RTL: {:?}",
                result.console
            );
            // Status register UART RTL yang dibaca CPU (disimpan ke RAM):
            // tx_count sebelum tulis = 0, setelah tulis 'A' = 1, 'B' = 2.
            let read = |off: u64| machine.mem.read(0x8000_0000 + off, 4).expect("read");
            assert_eq!(read(0), 0, "tx_count awal");
            assert_eq!(read(4), 1, "tx_count setelah 'A'");
            assert_eq!(read(8), 2, "tx_count setelah 'B'");
        });
    }

    /// E2E R4 — Direct RTL Device INTERRUPT device-initiated: `timer_console`
    /// menghitung mundur 64 cycle setelah di-load (MMIO 0x10001000) lalu
    /// menaikkan `irq_timer` (bit 4, LEVEL) TANPA aksi CPU — interrupt murni
    /// dari device. picorv32 masuk handler, handler tulis 'T' ke UART +
    /// maskirq + retirq. Verifikasi: console "T" + trap di ebreak (retirq
    /// pulang ke instruksi setelah waitirq) + timer reload/IRQ state.
    #[test]
    fn test_rtl_cpu_irq_timer() {
        with_big_stack(|| {
            let soc_files = soc_files();
            // main @0x80000000: unmask device IRQ (bit 3 UART + bit 4 timer),
            // load timer 64 → countdown → waitirq → ebreak.
            let main = [
                (0xFFFFFu32 << 12) | (5u32 << 7) | LUI, // lui t0, 0xFFFFF
                ((-25i32 as u32 & 0xfff) << 20) | (5u32 << 15) | (5u32 << 7) | ADDI, // addi t0, -25 → 0xFFFFEFE7 (unmask bit 3+4)
                r(0b0000011, 0, 5, 0, 0, 0x0B),                                      // maskirq t0
                (0x10001u32 << 12) | (10u32 << 7) | LUI, // lui a0, 0x10001 → timer base (0x10001000)
                i(64, 0, 0, 6, ADDI),                    // t1 = 64 (countdown)
                s(0, 6, 10, 2, SW),                      // timer load 64
                0x0800_000B,                             // waitirq              (0x80000018)
                0x0010_0073,                             // ebreak               (0x8000001C)
            ];
            // handler @0x80000100: tulis 'T' ke UART, mask semua IRQ, retirq.
            let handler = [
                (0x10000u32 << 12) | (10u32 << 7) | LUI, // lui a0, 0x10000 → UART base
                i(0x54, 0, 0, 6, ADDI),                  // t1 = 'T'
                s(0, 6, 10, 0, 0x23),                    // sb t1, 0(a0) → UART 'T'
                (0xFFFFFu32 << 12) | (7u32 << 7) | LUI,  // lui t2, 0xFFFFF
                i(-1, 7, 0, 7, ADDI),                    // t2 = 0xFFFFFFFF
                r(0b0000011, 0, 7, 0, 0, 0x0B),          // maskirq t2
                r(0b0000010, 0, 0, 0, 0, 0x0B),          // retirq
            ];
            let mut payload = vec![0u8; 0x120];
            for (i, w) in main.iter().enumerate() {
                payload[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            for (i, w) in handler.iter().enumerate() {
                let off = 0x100 + i * 4;
                payload[off..off + 4].copy_from_slice(&w.to_le_bytes());
            }
            let elf = make_elf(&payload);
            let mut mem = map();
            crate::elf::load_elf(&elf, &mut mem).expect("load elf");

            let mut cpu = RtlLinkedCpu::from_files(&soc_files, "rv32_soc").expect("compile");
            cpu.reset();
            let mut machine = crate::machine::Machine::new(Box::new(cpu), mem, 5000);
            let result = machine.run().expect("machine run");
            assert!(
                result.halted,
                "CPU RTL harus trap setelah retirq ke ebreak: {:?}",
                result
            );
            assert_eq!(
                result.pc & !3,
                0x8000_001c,
                "retirq harus pulang ke ebreak (0x1C)"
            );
            // Handler IRQ timer menulis 'T' → console dari UART RTL.
            assert_eq!(
                result.console, b"T",
                "console dari handler IRQ timer: {:?}",
                result.console
            );
        });
    }

    #[test]
    fn test_rtl_cpu_reset_pc_starts_at_reset_vector() {
        with_big_stack(|| {
            let mut cpu =
                RtlLinkedCpu::from_files(&cpu_files(), "rv32_bus_wrapper").expect("compile");
            cpu.reset();
            // Belum ada fetch → pc() = 0; setelah 1 step, fetch pertama di 0x80000000.
            let mut mem = map();
            let step = cpu.step(&mut mem).expect("step");
            assert!(matches!(step, CpuStep::InstructionExecuted { .. }));
            assert_eq!(cpu.pc() & !3, 0x8000_0000, "fetch pertama = PROGADDR_RESET");
        });
    }

    /// E2E R4 — Direct RTL Device INTERRUPT: `uart_console` menaikkan `irq_tx`
    /// (bit 3, level) setelah tulis byte; picorv32 (ENABLE_IRQ) masuk handler
    /// di PROGADDR_IRQ=0x80000100, handler menulis 'B' + maskirq + retirq,
    /// main lanjut ke ebreak. Verifikasi: console "AB" (byte dari UART RTL
    /// biasa + dari handler IRQ) + trap di alamat benar (retirq pulang ke
    /// instruksi setelah waitirq).
    #[test]
    fn test_rtl_cpu_irq_uart_tx() {
        with_big_stack(|| {
            let soc_files = soc_files();
            // main @0x80000000: unmask IRQ 3 (maskirq), tulis 'A' ke UART,
            // waitirq (blokir sampai IRQ), ebreak.
            let main = [
                0xFFFFF2B7u32, // lui t0, 0xFFFFF
                0xFF728293u32, // addi t0, t0, -9          → t0 = 0xFFFFFFF7
                0x0602800Bu32, // maskirq t0               → unmask bit 3
                0x10000537u32, // lui a0, 0x10000          → a0 = UART base
                0x04100313u32, // addi t1, zero, 0x41      → t1 = 'A'
                0x00650023u32, // sb t1, 0(a0)             → UART 'A'
                0x0800000Bu32, // waitirq                  (0x80000018)
                0x00100073u32, // ebreak                   (0x8000001C)
            ];
            // handler @0x80000100: tulis 'B', mask semua IRQ, retirq.
            let handler = [
                0x10000537u32, // lui a0, 0x10000
                0x04200313u32, // addi t1, zero, 0x42      → t1 = 'B'
                0x00650023u32, // sb t1, 0(a0)             → UART 'B'
                0xFFFFF3B7u32, // lui t2, 0xFFFFF
                0xFFF38393u32, // addi t2, t2, -1          → t2 = 0xFFFFFFFF
                0x0603800Bu32, // maskirq t2               → mask semua IRQ
                0x0400000Bu32, // retirq                   → pulang ke 0x8000001C
            ];
            let mut payload = vec![0u8; 0x11c]; // main 0x00..0x20, pad, handler 0x100..0x11c
            for (i, w) in main.iter().enumerate() {
                payload[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            for (i, w) in handler.iter().enumerate() {
                let off = 0x100 + i * 4;
                payload[off..off + 4].copy_from_slice(&w.to_le_bytes());
            }
            let elf = make_elf(&payload);
            let mut mem = map();
            crate::elf::load_elf(&elf, &mut mem).expect("load elf");

            let mut cpu = RtlLinkedCpu::from_files(&soc_files, "rv32_soc").expect("compile");
            cpu.reset();
            let mut machine = crate::machine::Machine::new(Box::new(cpu), mem, 5000);
            let result = machine.run().expect("machine run");
            assert!(
                result.halted,
                "CPU RTL harus trap setelah retirq ke ebreak: {:?}",
                result
            );
            assert_eq!(
                result.pc & !3,
                0x8000_001c,
                "retirq harus pulang ke ebreak (0x1C)"
            );
            // Byte dari main ('A') + handler IRQ ('B') — keduanya dari UART RTL.
            assert_eq!(
                result.console, b"AB",
                "console dari UART RTL (main + handler IRQ): {:?}",
                result.console
            );
        });
    }

    /// E2E R4 — UART RX (input HOST → device RTL, bidirectional): byte 'H'
    /// di-inject host (`push_uart_input`) → UART RTL meng-latch (edge `rx_wr`)
    /// → `irq_rx` (bit 5) → handler baca UART_BASE+8 (read-clear) → simpan ke
    /// RAM + echo ke UART TX → retirq. Verifikasi: console "H" (echo), RAM
    /// menyimpan 0x48 ('H') yang DIBACA dari device RTL, trap di ebreak.
    #[test]
    fn test_rtl_cpu_uart_rx_bidirectional() {
        with_big_stack(|| {
            let soc_files = soc_files();
            // main @0x80000000: unmask bit 5 (UART RX) → waitirq → ebreak.
            let main = [
                0xFFFFF2B7u32,                  // lui t0, 0xFFFFF
                0xFDF28293u32,                  // addi t0, t0, -33 → 0xFFFFEFDF (unmask bit 5 + 12)
                r(0b0000011, 0, 5, 0, 0, 0x0B), // maskirq t0
                0x0800_000B,                    // waitirq              (0x8000000C)
                0x0010_0073,                    // ebreak               (0x80000010)
            ];
            // handler @0x80000100: baca UART_BASE+8 (rx_byte, read-clear),
            // simpan ke RAM, echo ke UART TX, mask semua, retirq.
            let handler = [
                (0x10000u32 << 12) | (10u32 << 7) | LUI, // lui a0, 0x10000 → UART base
                i(8, 10, 2, 6, LW),                      // lw t1, 8(a0) → rx_byte
                (0x80000u32 << 12) | (11u32 << 7) | LUI, // lui a1, 0x80000 → RAM base
                s(4, 6, 11, 2, SW),                      // sw t1, 4(a1) → ram[0x80000004]
                s(0, 6, 10, 0, 0x23),                    // sb t1, 0(a0) → echo UART TX
                (0xFFFFFu32 << 12) | (7u32 << 7) | LUI,  // lui t2, 0xFFFFF
                i(-1, 7, 0, 7, ADDI),                    // t2 = 0xFFFFFFFF
                r(0b0000011, 0, 7, 0, 0, 0x0B),          // maskirq t2
                r(0b0000010, 0, 0, 0, 0, 0x0B),          // retirq
            ];
            let mut payload = vec![0u8; 0x124];
            for (i, w) in main.iter().enumerate() {
                payload[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            for (i, w) in handler.iter().enumerate() {
                let off = 0x100 + i * 4;
                payload[off..off + 4].copy_from_slice(&w.to_le_bytes());
            }
            let elf = make_elf(&payload);
            let mut mem = map();
            crate::elf::load_elf(&elf, &mut mem).expect("load elf");

            let mut cpu = RtlLinkedCpu::from_files(&soc_files, "rv32_soc").expect("compile");
            cpu.reset();
            // Input host → device RTL: 'H' di-inject sebelum run.
            cpu.push_uart_input(b"H");
            let mut machine = crate::machine::Machine::new(Box::new(cpu), mem, 5000);
            let result = machine.run().expect("machine run");
            assert!(
                result.halted,
                "CPU RTL harus trap setelah retirq ke ebreak: {:?}",
                result
            );
            assert_eq!(
                result.pc & !3,
                0x8000_0010,
                "retirq harus pulang ke ebreak (0x10)"
            );
            // Echo TX: byte RX dibaca handler lalu ditulis ke UART TX → console.
            assert_eq!(result.console, b"H", "echo UART TX: {:?}", result.console);
            // Byte yang DIBACA dari device RTL (UART_BASE+8) tersimpan di RAM.
            let rx = machine.mem.read(0x8000_0004, 4).expect("read");
            assert_eq!(
                rx, 0x48,
                "RAM harus berisi byte RX 0x48 ('H') dari UART RTL"
            );
        });
    }
}
