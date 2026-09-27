# EMULATOR.md — Mivon: Hardware-Software Emulator

> **Visi**: Mivon bukan "QEMU yang ditulis ulang dalam Rust" — itu terlalu kecil.
> Mivon adalah **Hardware-Software Emulator**: mesin virtual dibangun **langsung dari
> source HDL user** (RTL adalah machine model-nya), mampu boot dan menjalankan
> **OS nyata** (Linux/Windows) dari media yang **disediakan user** (ISO/disk/kernel).
> OS image tidak dibundel Mivon.

Status: **Desain** (belum implementasi). Berlaku bersama DESIGN.md, ROADMAP.md,
SYNTHESIS.md, MIVON-HDL.md, dan AUDIT.md.

---

## 1. Ringkasan Eksekutif

| Pertanyaan | Jawaban |
|---|---|
| Mesin berasal dari mana? | **Source HDL user** (`.mv`/`.sv`/`.v`/`.svh`/`.vh`; VHDL/SystemC menyusul) |
| OS berasal dari mana? | **User** (ISO/raw image/kernel+initrd/ELF) — Mivon tidak menyediakan OS |
| Engine eksekusi CPU? | **Full Rust** — Interpreter + JIT (Cranelift), tanpa dependensi QEMU |
| Struktur engine? | **Dua engine terpisah**: Mivon RTL Engine + Mivon Machine Engine, disatukan lewat **co-simulation** |
| Akurasi? | **Dual-mode**: `functional` (cepat) ↔ `cycle-accurate` (RTL asli), bisa dipilih per-device |
| Mode operasi CLI? | `rtl` · `sim` · `emu` · `hybrid` · `coemu` |
| Target ISA pertama? | **RISC-V** (sumber SoC di repo: `cva6/`, `openc910/`, `opentitan/`) |
| Pembeda utama vs emulator lain? | **Direct RTL Device** + **debugger lintas-lapisan** (OS → bus → RTL → signal → baris source) |
| ISO Windows? | Mungkin — bertahap via mesin x86-64 (UEFI/ACPI/APIC), setelah RISC-V/ARM terbukti |

Prinsip inti: **"Berikan RTL-nya. Mivon membangun mesin virtual dari hardware tersebut."**

```
                 ┌──────────────────────────────────────┐
                 │              MIVON                    │
                 │ Hardware + Software Emulator          │
                 └──────────────────────────────────────┘
                                │
             ┌──────────────────┼──────────────────┐
             │                  │                  │
             ▼                  ▼                  ▼
       HDL Frontend        Hardware Model       OS Runtime
       .mv/.sv/.v          RTL / Netlist         Linux
       .svh/.vh            Device Model          Windows
             │                  │                  │
             └──────────────┬───┴──────────────────┘
                            ▼
                    Mivon Machine Model
                            │
              ┌─────────────┼─────────────┐
              ▼             ▼             ▼
            CPU           Memory          Bus
              │             │             │
              └─────────────┼─────────────┘
                            ▼
                     Device Fabric
                            │
             ┌──────────────┼──────────────┐
             ▼              ▼              ▼
           PCIe           UART          Network
             │              │              │
             └──────────────┼──────────────┘
                            ▼
                       Guest OS
                   Linux / Windows
```

---

## 2. Perbedaan Fundamental dengan QEMU

**QEMU** mulai dari model hardware yang sudah ditulis sebagai software (device model
dalam C). **Mivon** mulai dari RTL:

```
RTL (.sv/.v/.svh/.vh/.mv)
      │
      ▼
Mivon HDL Compiler
      │
      ▼
Elaborated Hardware
      │
      ▼
Hardware IR (MHIR)
      │
      ▼
Mivon Emulator
      │
      ▼
Real OS
```

Jika user punya `cpu.sv`, `cache.sv`, `axi.sv`, `uart.sv`, `plic.sv`, `clint.sv`,
`memory.sv`, `soc.sv` — Mivon **tidak** berkata "saya punya model CPU virtual
bernama X". Mivon berkata: **"Berikan RTL-nya. Saya bangun mesin virtual
berdasarkan hardware tersebut."**

Konsekuensi: bug di RTL muncul di Mivon; di QEMU tidak akan pernah.

---

## 3. Prinsip Desain

1. **OS-agnostic**: Mivon tidak membundel OS. Media boot (ISO/raw/kernel) dari user.
   Syarat hanya: ISA & machine cocok dengan media.
2. **HDL-native**: semua device boleh berasal dari RTL user. Device native
   (16550, PLIC, virtio) adalah fallback/percepatan, bukan keharusan.
3. **Full-Rust**: seluruh engine (termasuk JIT CPU) di Rust/Cranelift. Tidak ada
   dependensi eksekusi ke QEMU.
4. **Dua engine, jangan satu monster**: Mivon RTL Engine (akurasi) dan Mivon
   Machine Engine (kecepatan) adalah entitas terpisah, disatukan oleh
   co-simulation. Satu engine yang mencoba melakukan semuanya = resep monster
   compiler yang makan RAM.
5. **Dual-mode akurasi per-device**: `execution_mode = RTL | JIT | native`
   bisa berbeda untuk CPU, peripheral, dan glue logic dalam satu run.
6. **Sandbox**: OS tamu tidak pernah menyentuh host secara langsung — semua
   resource host lewat lapisan virtual + sandbox.
7. **Deterministik**: seed tetap → eksekusi identik; replay trace untuk bug.
8. **Bertahap dari aset yang ada**: reuse cycle-fusion, DAG-parallel, JIT,
   mivon-sir/netlist, parallel/distributed framework.

---

## 4. MHIR — Mivon Hardware IR (Jantung Mivon)

MHIR adalah bagian terpenting. Bukan sekadar netlist — MHIR adalah representasi
**hardware yang sudah diekstraksi** namun **tetap menunjuk balik ke RTL source**.

```
RTL → Parser → Elaboration → MHIR
                               ├── Module
                               ├── Port
                               ├── Signal
                               ├── Register
                               ├── Memory
                               ├── Process
                               ├── Clock
                               ├── Reset
                               ├── Bus
                               ├── Interrupt
                               ├── Address Map
                               └── Device
```

### 4.1 Contoh ekstraksi

```systemverilog
module uart (
    input  logic clk,
    input  logic rst,
    input  logic [7:0] data,
    output logic tx
);
```

menjadi:

```
Device: UART
  Clock:   clk
  Reset:   rst
  Register: DATA
  Output:   TX
  Width:    8-bit
```

### 4.2 Back-pointer ke source (jangan buang RTL semantics)

Setiap node MHIR membawa asal source:

```
MHIR node
   ├── source file
   ├── line
   ├── column
   ├── module
   └── process
```

Inilah yang memungkinkan debugger lintas-lapisan (lihat §16):

```
Guest OS → CPU instruction → MMIO → UART → RTL signal → uart.sv:143
```

### 4.3 Relasi dengan IR yang ada

| Lapisan | Struktur | Status |
|---|---|---|
| AST | `mivon-ast` (Design) | ✅ ada |
| IR elaborasi | `mivon-ir` (IrDesign) | ✅ ada |
| Sintesis/netlist | `mivon-sir`, `mivon-netlist` | ✅ ada |
| **MHIR** | `mivon-emu::mhir` (baru) — ekstraksi register/device/address-map **+** back-pointer | 🆕 dibangun di atas IrDesign + netlist |

MHIR tidak menggantikan IrDesign — ia **meninggikan** abstraksinya: IrDesign
tetap dipakai RTL Engine; MHIR dipakai Machine Engine dan debugger.

---

## 5. Mivon Machine Definition

Dari MHIR, Mivon membangun **Machine Definition**:

```
Machine
├── CPU        → ISA, registers, privilege, interrupt
├── Memory     → RAM, ROM, MMIO
├── Bus        → AXI, APB, custom
├── Interrupt Controller
├── Timer
├── UART
├── DMA
├── PCIe
├── Network
├── Storage
└── Firmware
```

OS tamu melihatnya sebagai **komputer sungguhan**.

```rust
struct MachineDef {
    name: Symbol,
    cpu: CpuDesc,                // ISA, mode, reset vector
    memory_map: Vec<MemoryRegion>,
    bus: BusTopology,
    devices: Vec<DeviceInstance>, // dari RTL / software / native
    interrupts: InterruptTopology,
    firmware: Option<FirmwareDesc>,
    boot: BootDesc,
}
```

---

## 6. Dua Jalur Eksekusi — Dua Engine Terpisah

### Jalur A — RTL-accurate (Mivon RTL Engine)

Untuk: debugging RTL, verification, waveform, assertion, timing, X/Z propagation,
signal tracing, cycle accuracy.

```
RTL → Elaboration → Hardware IR → RTL Simulator → Cycle / Event Simulation
CPU → ALU → Register File → Cache → AXI → DRAM   (cycle-by-cycle)
```

= engine event-driven/cycle-based yang sudah ada (`SimulationEngine`, Tier A/B).

### Jalur B — OS Emulation (Mivon Machine Engine)

Untuk: Linux, Windows, bootloader, kernel, driver, filesystem, networking, aplikasi.

```
RTL → Elaboration → Hardware Extraction → Executable Hardware Model
      → Mivon Machine → Guest OS
```

Mivon **tidak** menjalankan setiap gate RTL untuk setiap instruksi CPU. RTL
`always_ff @(posedge clk)` diekstraksi menjadi model eksekusi yang jauh lebih cepat.

### 6.1 Co-Simulation

```
                 Mivon
                   │
        ┌──────────┴──────────┐
        ▼                     ▼
   RTL Engine            Machine Engine
   cycle/event              JIT
        │                     │
        └──────────┬──────────┘
                   ▼
              Co-Simulation
```

Satu device bisa berkata `execution_mode = RTL`, CPU `execution_mode = JIT`,
peripheral lain `execution_mode = native` — dalam satu run yang sama.

---

## 7. Engine Detail

### 7.1 Mivon RTL Engine

| Tier | Deskripsi | Status |
|---|---|---|
| **Tier A** | Event-driven, IEEE 1800 13-region scheduler | ✅ ada |
| **Tier B** | Cycle-based compiled 2-state per clock domain (reuse cycle-fusion + DAG + JIT eval) | 🆕 |

### 7.2 Mivon Machine Engine — CPU 4 mode

```
Mivon CPU Engine
│
├── Interpreter   — paling lambat, paling mudah di-debug
│                    instruction → decode → execute → memory → interrupt
│                    untuk: debugging CPU, bring-up, verification
├── JIT           — untuk Linux/Windows
│                    guest instructions → Mivon Decoder → IR → Native Code
│                    (x86-64/ARM host; Cranelift)
├── RTL-linked    — CPU itu sendiri berasal dari RTL (cycle-accurate, Tier A)
└── Hybrid        — JIT untuk komputasi, RTL-linked saat masuk hardware tertentu
```

**Hybrid adalah senjata utama**:

```
              Mivon
                 │
       ┌─────────┴─────────┐
       │                   │
   Fast Path           Accurate Path
       │                   │
      JIT                 RTL
       │                   │
       ▼                   ▼
    CPU code          Peripheral
       │                   │
       └─────────┬─────────┘
                 ▼
             OS state
```

OS menjalankan kernel + aplikasi (JIT). Saat masuk MMIO → UART RTL, Mivon
**berpindah ke model RTL** untuk device tersebut, lalu kembali ke JIT.

#### RTL-linked — implementasi saat ini (mode 3, `mivon emu --rtl-cpu`)

Mesin (CPU) dibangun **murni dari RTL user (.sv/.v)** — bukan model software
Rust ala QEMU. Register file, ALU, dan kontrol dieksekusi oleh Mivon RTL
Engine (Tier A); sisi Rust hanya menyediakan memori + orkestrasi bus.

- `RtlLinkedCpu` (`crates/mivon-emu/src/cpu/rtl.rs`) — implementasi `CpuCore`
  yang mengkompilasi file RTL CPU (parser + elaborator), meresolusi port bus,
  dan menggerakkan clock. Kontrak bus wajib (picorv32-style):
  `clk, resetn, mem_valid, mem_instr, mem_addr[31:0], mem_wdata[31:0],
  mem_wstrb[3:0], mem_ready, mem_rdata[31:0], trap`.
- `Machine` (`crates/mivon-emu/src/machine.rs`) — loop eksekusi: step CPU RTL
  + layani transaksi bus (`mem_valid`/`mem_ready`, strobe per-byte untuk
  store) sampai trap (ebreak/ecall/ilegal) atau `max_steps`.
- Driver clock: clk ditulis **di dalam time step** via event terjadwal
  (`SdfDelayedWrite`) — tulis langsung sebelum `step_cycle()` tidak pernah
  terdeteksi sebagai edge (engine mengambil snapshot Preponed di awal time
  step, transisi 0→1 sudah "settled" → posedge tidak membangunkan proses
  Sequential).
- Konstruksi CPU RTL: `RtlLinkedCpu::from_files(&[wrapper, core], top)`;
  reset (`resetn` rendah beberapa cycle) → boot di `PROGADDR_RESET`.
- Parameter override instance (`#(.PROGADDR_RESET(32'h8000_0000))`) di-
  elaborasi dan di-fold ke IR (`reg_pc <= 0x80000000`).

Contoh nyata (picorv32.v dari GitHub, `examples/rtl/`):

```shell
mivon emu examples/rtl/rv32_bus_wrapper.sv examples/rtl/picorv32.v \
  --config emu_ram.meu \
  --rtl-cpu examples/rtl/rv32_bus_wrapper.sv --rtl-cpu examples/rtl/picorv32.v \
  --rtl-cpu-top rv32_bus_wrapper --run --max-steps 200
# → halted (trap cause=11) after N instr / M cycles — pc=0x...
```

`emu_ram.meu` (region RAM host, field di root TOML):

```toml
ram = { base = 0x80000000, size = 0x10000 }
```

E2E terverifikasi: program bare-metal (ADDI/LUI/SW/LW/ADD/SW/ebreak) di-
jalankan oleh picorv32 RTL — hasil komputasi (42+42=84) tersimpan di RAM oleh
RTL, bukan host (`test_rtl_cpu_runs_elf_program`).

**Direct RTL Device (R4)**: top `rv32_soc.sv` menginstansiasi picorv32 +
`uart_console.sv` + decoder MMIO (`0x1000_0000`, decode DI RTL).

- **MMIO write**: store CPU ke MMIO di-latch UART RTL (`tx_byte`/`tx_done`);
  host TIDAK menjawab txn MMIO (`serve()` mengecek `mmio_sel`) — decoder RTL
  yang memberi ack (OR internal `cpu_mem_ready`). Byte UART ditangkap host
  (`uart_tx_done` pulse) → `MachineResult.console` → summary CLI
  `— console: [ABC] (3 bytes)`. Rust hanya membaca sinyal output — logika
  UART murni RTL.
- **MMIO read**: `cpu_mem_rdata` = mux RTL — MMIO → register UART
  (`UART_BASE+0` = tx_byte, `UART_BASE+4` = `tx_count`), non-MMIO → rdata
  host (RAM). Host tidak men-drive `mem_rdata` saat MMIO. Status register
  monotonik (`tx_count` naik tiap tulis) → verifikasi deterministik.
- **Interrupt device (R4)**: `uart_console` menaikkan `irq_tx` (level, bit 3)
  `IRQ_DELAY` (=16) cycle setelah `tx_done` — tunda memberi CPU waktu
  mencapai `waitirq` sebelum IRQ tiba (deterministik, tidak bergantung timing
  pipeline). picorv32 di-instansiasi dengan `ENABLE_IRQ=1` dan
  `PROGADDR_IRQ=0x80000100` (handler di RAM). Handshake murni RTL:
  `irq[3]` (level) → `irq_pending` → CPU masuk handler (return addr di q0,
  IRQ vector di q1) → `eoi[3]` pulse di `irq_state[1]` → UART menurunkan
  `irq_tx` (ack). Custom instr picorv32 dipakai: `maskirq` (unmask/mask),
  `waitirq` (blokir sampai IRQ, pulang ke instruksi setelahnya), `retirq`.
- **Timer device (R4)**: `timer_console` (TIMER_BASE `0x10001000`) — load
  countdown 32-bit via MMIO write → `count` turun 1/cycle → saat transisi
  1→0 menaikkan `irq_timer` (level, bit 4) **tanpa aksi CPU** (interrupt
  device-initiated). Ack via `eoi[4]` atau reload. SoC kini punya DUA device
  RTL (UART bit 3 + timer bit 4), decoder MMIO 0x1000_0000–0x1000_1fff dengan
  sub-decode `uart_sel`/`timer_sel` dan mux rdata berprioritas DI RTL.

Verifikasi e2e: `test_rtl_cpu_mmio_uart_console` (CPU tulis 'A','B','C' ke
0x10000000 → console "ABC"; store RAM biasa tetap bekerja),
`test_rtl_cpu_mmio_read_status` (CPU baca `tx_count` 0→1→2 → simpan ke RAM →
diverifikasi host), `test_rtl_cpu_irq_uart_tx` (main unmask IRQ 3 →
tulis 'A' → `waitirq`; IRQ UART masuk handler di 0x80000100 → handler tulis
'B' + `maskirq` semua + `retirq` → main lanjut ke ebreak. Console "AB" —
byte dari main DAN dari handler IRQ, keduanya dari UART RTL; retirq pulang
tepat ke instruksi setelah `waitirq`), dan `test_rtl_cpu_irq_timer`
(load timer 64 → countdown → IRQ bit 4 device-initiated → handler tulis
'T' ke UART + `retirq` → ebreak; console "T").

### 7.3 Protokol co-sim MMIO (transaksi bus)

```
CPU (JIT)                Dispatcher              RTL Engine (Tier A/B)
    │  MMIO write 0x10000000   │                       │
    ├─────────────────────────►│  transaction mulai     │
    │                          ├───────────────────────►│
    │                          │  jalankan RTL sampai   │
    │                          │  bus transaction       │
    │                          │  selesai / respon      │
    │                          │◄───────────────────────┤
    │◄─────────────────────────┤  selesai + data        │
    │  resume translation      │                       │
```

- Mode functional: latensi transaksi konfigurable (default 0 / dari model bus).
- Mode cycle-accurate: RTL maju per cycle; transaksi menunggu handshake bus asli.

---

## 8. Mode Operasi CLI

| Mode | Tujuan | Engine aktif |
|---|---|---|
| `rtl` | Akurasi RTL murni | RTL Engine (Tier A) |
| `sim` | Event/cycle simulation | RTL Engine (Tier A/B) |
| `emu` | OS emulation (CPU JIT/interpreter) | Machine Engine |
| `hybrid` | JIT + RTL (per-device) | Keduanya (co-sim) |
| `coemu` | Hardware + OS co-emulation penuh | Keduanya (co-sim penuh) |

```shell
mivon run --mode rtl design.mv
mivon run --mode hybrid soc.mv --disk linux.img
mivon run --mode coemu --rtl soc.sv --firmware opensbi.bin --disk rootfs.img
```

---

## 9. Device ABI

OS tidak peduli hardware berasal dari SystemVerilog — OS hanya melihat CPU,
Memory, PCI, UART, Storage, Network, Interrupt, Timer. Maka Mivon mendefinisikan
**kontrak device**:

```
Device
├── identity    — nama, vendor, versi, jenis
├── MMIO        — region alamat, read/write callback
├── IRQ         — line, trigger level/edge
├── DMA         — master port ke memory
├── reset       — perilaku saat reset
├── clock       — domain, frekuensi
├── state       — akses state internal (debug)
├── snapshot    — serialize/deserialize state
└── migration   — pindah antar host/thread
```

### 9.1 Tiga sumber device

```
UART
 ├── RTL implementation   — uart.sv, via Direct RTL Device
 ├── Mivon software model — implementasi Rust native (fallback)
 └── host terminal        — terhubung ke console host (pty/stdio/socket)
```

```rust
pub trait Device: Send {
    fn identity(&self) -> &DeviceIdentity;
    fn mmio(&mut self, addr: u64, write: bool, size: u8, val: u64) -> MmioResult;
    fn irq(&self) -> &[IrqLine];
    fn reset(&mut self);
    fn clock_domain(&self) -> ClockDomainId;
    fn snapshot(&self) -> Vec<u8>;
    fn restore(&mut self, data: &[u8]);
}
```

---

## 10. Direct RTL Device — Pembeda Utama Mivon

Fitur yang menjadi pembeda utama. User punya `uart.sv`, Mivon mendeteksi
`module uart`, user mendefinisikan:

```
MMIO: 0x10000000
IRQ:  5
```

Mivon membangun:

```
Guest CPU
   │  store 0x10000000
   ▼
AXI
   │
   ▼
UART RTL
   │
   ▼
TX
   │
   ▼
Host Terminal
```

Bukan sekadar "UART emulator" — tetapi:

```
OS → virtual bus → actual RTL-derived device
```

Implementasi: anotasi RTL **di atas instance** (R0.5, sudah jalan):

```systemverilog
(* mivon_region = "mmio", base = "0x10000000", size = "0x1000" *)
(* mivon_irq = "5" *)
uart u_uart (.clk(clk), /* ... */);
```

Parser menempatkan atribut ke `ModuleInstance.attrs` → elaborator →
`IrInstance.attrs` → MHIR (`MhirDevice.mmio`/`irq` + `address_map`).
`base`/`size` menerima hex `0x...` atau desimal; `mivon_region` tanpa
`base`/`size` lengkap diabaikan (bukan region 0 yang menyesatkan). Nilai
konfigurasi lain (`--addr`, `[[devices]]` `.meu`) **menimpa** anotasi untuk
nama yang sama. Alternatif tanpa menyentuh RTL: `[[devices]]` di config
`.meu` (lihat §20).

---

## 11. Memory Subsystem

### 11.1 Guest memory berlapis

```
Guest Memory
├── RAM
├── ROM
├── MMIO
├── Shared Memory
└── DMA Memory
```

### 11.2 Backend

```
Anonymous memory
mmap
Huge pages
File-backed memory
Shared memory
```

### 11.3 Alur akses

```
Guest Physical Address → Mivon MMU → Memory Map → RAM / Device
```

- RAM → host mmap (zero-copy, byte-addressable, ukuran konfigurable).
- MMU walk (page table) dilakukan **fungsional di software** (softmmu/TLB),
  bukan di RTL — page-table walk di RTL = pembunuh performa #1.
- Koherensi DMA: master DMA di RTL menulis ke **backing store yang sama**
  (mmap) → koherensi by construction; notify untuk invalidasi TLB CPU.

---

## 12. OS Services & Boot Flow

### 12.1 Linux (target pertama)

```
Mivon
 ├── RISC-V CPU RTL (cva6/C910) — atau picorv32/Ibex untuk bare-metal
 ├── AXI
 ├── CLINT
 ├── PLIC
 ├── UART
 ├── RAM
 └── VirtIO
       │
       ▼
    OpenSBI
       │
       ▼
     Linux
       │
       ▼
   userspace
```

Setelah boot: `uname -a`, `ls`, `ip addr`, `cat /proc/cpuinfo`, `mount`,
`dmesg` — semua harus bekerja jika device model + kernel support benar.

### 12.2 Windows (bertahap, jauh lebih sulit)

Bukan karena Windows sakral, tetapi kebutuhan device/firmware jauh lebih kompleks:

```
UEFI → ACPI → CPU → PCIe → APIC → HPET/Timer → RAM → Storage → Network
     → Display → USB → Interrupt
```

Phase bertahap (jangan langsung "jalankan seluruh Windows"):

| Phase | Target |
|---|---|
| Phase 1 | Windows bootloader |
| Phase 2 | Windows kernel |
| Phase 3 | Basic device initialization |
| Phase 4 | Safe Mode |
| Phase 5 | Normal desktop |

---

## 13. Keamanan & Sandbox

**OS nyata tidak boleh langsung menyentuh host.** Ini prinsip keamanan wajib.

```
Jangan:  Windows guest → Host filesystem langsung
```

```
Guest
 ↓
Virtual Device
 ↓
Mivon Sandbox
 ↓
Host Resource
```

Contoh filesystem:

```
Guest NTFS → Virtual Disk → Mivon Storage Backend → qcow-like / raw / sparse image
```

Host tetap terlindungi: guest hanya melihat device virtual; akses host
(terminal, file, network) selalu lewat lapisan sandbox + izin eksplisit user.

---

## 14. Snapshot Engine

```
Machine
├── CPU state
├── RAM
├── Device state
├── Interrupt state
├── DMA state
├── Timer
└── RTL state
```

```shell
mivon snapshot create
mivon snapshot restore
```

**Status (fase R5, slice pertama) — terimplementasi** di crate `mivon-emu`:

- `Machine::snapshot()` / `Machine::restore()` — state CPU (blob per-ISA via
  `CpuCore::snapshot/restore`) + seluruh region memori + counter langkah
  kumulatif. Restore MENOLAK memory map yang tidak persis cocok
  (nama/base/size) — state tak dipindahkan diam-diam ke RAM yang salah.
- Format file `MIVSNAP1` (`mivon-emu/src/snapshot.rs`): little-endian,
  **sparse per halaman 4 KB** (halaman nol tidak ditulis → RAM 2 GB berisi
  sedikit data = file kecil), atomik (temp+rename), magic+versi → error
  jelas untuk file rusak/versi beda.
- CPU yang didukung: **interpreter RV32** (`Rv32Cpu`), **x86** (`X86Cpu`,
  boot ISO), dan **Direct RTL CPU** (`RtlLinkedCpu`) — state engine RTL
  lewat `SimCheckpoint` (SIM-17/18: sinyal/waktu/RNG/process map/UVM/
  coverage/signal history) + state bus/host. Checkpoint tidak memuat
  antrean event engine → snapshot hanya boleh saat idle antar langkah
  (`SimulationEngine::pending_runtime_events` menolak dan menyebut
  antrean mana); restore me-reset antrean tsb bersih. Target restore harus
  dari design/RAM yang sama (signals count + nama/base/size dicek).
- CLI: `mivon emu ... --snapshot-save <file>` (setelah run) /
  `--snapshot-load <file>` (sebelum run → lanjut dari state tersimpan).
- Deterministik: resume = eksekusi identik dengan run penuh (test
  `test_machine_snapshot_resume_deterministic`).

```shell
mivon emu --config ram.meu --load-elf prog.elf --run --max-steps 2 \
  --snapshot-save mid.snap
mivon emu --config ram.meu --load-elf prog.elf --snapshot-load mid.snap \
  --run --max-steps 1000
```

Belum (lanjutan R5): machine-level `mivon snapshot --tag`, snapshot device
state terpisah dari engine.

Snapshot juga alat debugging RTL:

```
Boot Linux → Crash → Restore snapshot → Change RTL → Replay
```

Sangat berguna untuk hardware development: ubah RTL, replay dari titik crash
tanpa boot ulang.

---

## 15. Deterministic Execution & Replay

```
Deterministic Mode
  Seed = 12345
  CPU → Device → Interrupt → DMA → Timer
  → eksekusi IDENTIK setiap run
```

```shell
mivon replay trace.bin
```

Trace berisi: instruksi, MMIO, interrupt, DMA, timer events. Replay mengulang
bug secara deterministik — bernilai tinggi untuk hardware verification.

---

## 16. Time Engine

Mivon tidak boleh bergantung hanya pada wall-clock host. Gunakan
**Mivon Virtual Time**:

```
0, 1 ns, 2 ns, 3 ns, ...
```

CPU execution bisa memakai **instruction count**; semuanya disinkronkan:

```
Virtual Time
   ├── CPU
   ├── Timer
   ├── DMA
   ├── UART
   └── RTL
```

- Mode functional: CPU maju per instruksi, RTL maju saat transaksi MMIO.
- Mode cycle-accurate: semua maju per cycle clock domain (virtual time = time
  sim RTL).

---

## 17. Cross-Layer Debugger — Identitas Mivon

Fitur paling "Mivon": hubungan debugging end-to-end:

```
Linux application → syscall → driver → MMIO → PCI/AXI → RTL module
→ always_ff → signal
```

Debugger Mivon menunjukkan:

```
Guest:    PID 421
Instruction: 0x80203410
Memory:      0x10000000
Device:      UART
Bus:         AXI4
RTL:         uart.sv:143
Signal:      tx_valid = 1
```

Developer hardware melihat **software → bus → RTL → signal** dalam satu
debugger. Ini didukung oleh back-pointer MHIR (§4.2) + breakpoint di semua
lapisan (instruksi CPU, MMIO, bus transaction, RTL signal).

---

## 18. Arsitektur Software Mivon

```
mivon/
│
├── compiler/      lexer, parser, elaborator, resolver, optimizer
├── hdl/           sv, verilog, systemverilog, mivon_hdl
├── ir/            hwir (MHIR), rtl-ir, machine-ir
├── emulator/      cpu, memory, bus, interrupt, timer, scheduler
├── devices/       uart, virtio, pci, storage, network, usb, display
├── rtl_runtime/   process, signal, clock, reset, event
├── os/            linux, windows, firmware
├── jit/           decoder, optimizer, backend
├── debug/         debugger, waveform, trace, replay
└── snapshot/
```

### 18.1 Pemetaan ke workspace crates yang ada

| Konsep desain | Crate/area yang ada | Status |
|---|---|---|
| compiler + hdl | `mivon-parser`, `mivon-compiler`, `mivon-elaboration` | ✅ |
| ir → hwir (MHIR) | `mivon-ir` + **baru** `mivon-emu::mhir` | 🆕 |
| ir → rtl-ir | `mivon-sir`, `mivon-netlist` | ✅ |
| emulator (rtl) | `mivon-simulator` (engine, scheduler) | ✅ |
| emulator (machine) | **baru** `mivon-emu` (cpu, memory, bus, devices) | 🆕 |
| jit | Cranelift (`jit` feature) | ✅ (perlu decoder ISA) |
| rtl_runtime | `mivon-simulator` (state, value, event) | ✅ |
| debug | `mivon-simulator::debugger` | ✅ (perlu lapisan lintas) |
| snapshot | checkpoint (`SIM-17/18`) + **baru** machine snapshot | 🆕 |

Aturan 1 file = 1 tanggung jawab tetap berlaku; `mivon-emu` adalah crate baru
yang memakai API `mivon-api`/`mivon-simulator`/`mivon-sir`.

---

## 19. Antarmuka Antar-Lapisan (API ringkas)

```rust
// crate baru: crates/mivon-emu/
pub mod mhir;      // Mivon Hardware IR: ekstraksi + back-pointer
pub mod machine;   // MachineDef, builder dari MHIR
pub mod mem;       // MemoryPort, RamRegion, MmioBackend, softmmu/TLB
pub mod cpu;       // CpuCore trait, Interpreter, JIT (Cranelift), RtlLinked
pub mod devices;   // Device trait + 16550, PLIC, CLINT, virtio-mmio, virtio-blk/net
pub mod cosim;     // co-simulation dispatcher (MMIO trap, DMA notify)
pub mod time;      // Mivon Virtual Time
pub mod sandbox;   // sandbox resource access
pub mod snapshot;  // machine snapshot create/restore
pub mod replay;    // trace + deterministic replay
pub mod debug;     // cross-layer debugger

pub trait CpuCore {
    fn reset(&mut self);
    fn step(&mut self, mem: &mut dyn MemoryPort) -> Result<CpuStep, CpuFault>;
    fn pc(&self) -> u64;
    fn set_pc(&mut self, addr: u64);
    fn raise_interrupt(&mut self, irq: u32, level: bool);
    fn read_reg(&self, idx: usize) -> u64;
    fn isa(&self) -> Isa;
}

pub trait MemoryPort {
    fn read(&self, addr: u64, size: u8) -> Result<u64, AccessFault>;
    fn write(&mut self, addr: u64, size: u8, val: u64) -> Result<(), AccessFault>;
    fn region_of(&self, addr: u64) -> Option<RegionRef>; // RAM | ROM | Mmio | Unmapped
}

pub enum ExecutionMode { Rtl, Jit, Native, Hybrid }   // per-device

pub struct Emulator {
    machine: MachineDef,
    mem: Arc<dyn MemoryPort>,
    cpu: Box<dyn CpuCore>,
    rtl: RtlBackend,
    devices: DeviceTable,       // masing-masing dgn execution_mode
    time: VirtualTime,
    mode: RunMode,              // rtl | sim | emu | hybrid | coemu
}
```

---

## 20. CLI & Project File

```shell
mivon run --mode rtl design.mv
mivon run --mode sim soc.sv -T 1_000_000
mivon run --mode emu --soc cva6 --kernel vmlinux --initrd rootfs.cpio
mivon run --mode hybrid soc.mv --disk linux.img
mivon run --mode coemu --rtl soc.sv --firmware opensbi.bin --disk rootfs.img

mivon emu --dump-memory-map chip.mivon
mivon emu --dump-dtb chip.mivon
mivon snapshot create --tag booted
mivon snapshot restore --tag booted
mivon replay trace.bin

# Interpreter RISC-V32 (tanpa --rtl-cpu): ELF dijalankan sampai ebreak
mivon emu --config ram.meu --load-elf prog.elf --run --max-steps 1000000
# + snapshot (EMULATOR.md §14): simpan tengah jalan → lanjut nanti
mivon emu --config ram.meu --load-elf prog.elf --run --max-steps 2 \
  --snapshot-save mid.snap
mivon emu --config ram.meu --load-elf prog.elf --snapshot-load mid.snap \
  --run --max-steps 1000000
```

Konfigurasi emulator = **file TOML terpisah** (default ekstensi `.meu`),
dimuat via `--config` — **BUKAN** section di project file `.mivon`
(ekstensi/direktori `.mivon` dipakai MICD dan file list — tidak boleh bentrok):

```toml
# soc.meu
top = "ariane_soc"
mode = "coemu"                 # rtl | sim | emu | hybrid | coemu
accuracy = "functional"        # functional | cycle-accurate
cpu = "riscv64"                # auto-detect bila kosong
console = "pty"                # stdio | pty | socket:<path>
block = ["rootfs.img"]
iso  = "debian-riscv64.iso"    # opsional, sebagai CD-ROM
firmware = "opensbi.bin"       # opsional; default boot flow bila kosong
dtb = "board.dts"              # opsional; auto-generate bila kosong
seed = 12345                   # deterministic mode

[ram]
base = 0x80000000             # TOML integer hex didukung
size = 0x40000000             # 1GB

[[devices]]                    # Direct RTL Device
name = "u_uart"
rtl = "uart.sv"
mmio = 0x10000000
size = 0x1000
irq = 5
```

```shell
mivon emu --config soc.meu rtl/ ...

# Direct RTL CPU (mode 3) — mesin dari RTL .sv/.v, bukan interpreter:
mivon emu wrapper.sv picorv32.v --config emu_ram.meu \
  --rtl-cpu wrapper.sv --rtl-cpu picorv32.v --rtl-cpu-top rv32_bus_wrapper \
  --run --max-steps 10000
```

---

## 20.5 Status Implementasi (2026-08-16)

**R0 — SEBAGIAN SELESAI ✅** (crate `mivon-emu`, CLI `mivon emu`):

| Item R0 | Status |
|---|---|
| `mivon-emu` crate (mhir: types/backptr/extract + dump) | ✅ 18 unit test |
| Ekstraksi clock/reset/register (FF inference)/memory/device | ✅ |
| Back-pointer instance (line/col) + signal (scan source) | ✅ |
| `apply_address_map` (`--addr NAME=BASE:SIZE`, match instance/module) | ✅ |
| CLI `mivon emu --dump-mhir / --dump-memory-map` | ✅ |
| Config emulator file TOML terpisah (`.meu`, `--config`) — top/ram/devices/seed | ✅ 7 unit test |
| Memory subsystem: `MemoryPort` + `RamRegion` (mmap) + `MemoryMap` decode | ✅ 8 unit test |
| ELF loader (ELF32/64 LE, PT_LOAD + bss) ke MemoryPort | ✅ 6 unit test |
| CLI `--load-elf` + `--dump-memory` (hex dump) + `--config` | ✅ |
| Anotasi `(* mivon_region *)` / `(* mivon_irq *)` dari AST | ✅ R0.5 — parser (`ModuleInstance.attrs`) → elaborator (`IrInstance.attrs`) → MHIR (`MhirDevice.mmio`/`irq` + `address_map`); `--dump-memory-map` langsung menampilkan region tanpa `--addr`; config/CLI menimpa anotasi; 4 test parser (value/bare/line-col akurat/non-instance) + 3 test MHIR + e2e `examples/rtl/rv32_soc.sv` (u_uart 0x10000000 irq3, u_timer 0x10001000 irq4) |
| CPU interpreter RISC-V32 (R2) — `cpu/riscv32.rs`: RV32IM + Zicsr, trap/
  interrupt berprioritas/mret, 13+ test end-to-end | ✅ |
| **Direct RTL CPU (mode 3, §7.2)** — `cpu/rtl.rs` `RtlLinkedCpu` + `machine.rs`
  `Machine`: mesin dijalankan dari RTL .sv/.v (picorv32.v dari GitHub,
  `examples/rtl/`), bukan interpreter. CLI `--rtl-cpu/--rtl-cpu-top/--run/
  --max-steps`; e2e program bare-metal → hasil di RAM oleh RTL | ✅ |
| **Direct RTL Device (R4)** — `rv32_soc.sv` + `uart_console.sv`
  (examples/rtl/): decode MMIO (0x10000000) DI RTL, UART TX RTL → byte
  ditangkap host (`mmio_sel`/`uart_tx_done`/`uart_tx_byte`), console host di
  `MachineResult.console`/`--run` summary. **MMIO write**: CPU tulis 'ABC'
  → console "ABC" (test `test_rtl_cpu_mmio_uart_console`). **MMIO read**: status
  register `tx_count` (UART_BASE+4, mux `cpu_mem_rdata` DI RTL) — CPU baca
  0→1→2 disimpan ke RAM, diverifikasi (test `test_rtl_cpu_mmio_read_status`).
  Rust hanya membaca sinyal output UART — logika UART murni RTL | ✅ store+read |
| Interrupt device RTL (IRQ → CPU) — `uart_console` `irq_tx` level (bit 3,
  delay 16 cycle) + `timer_console` `irq_timer` level (bit 4, device-initiated,
  countdown 0x10001000); picorv32 `ENABLE_IRQ=1` + `PROGADDR_IRQ=0x80000100`;
  handshake `eoi[3]`/`eoi[4]` ack di RTL; program bare-metal
  `maskirq`+`waitirq`+`retirq` → console "AB" (UART) dan "T" (timer),
  retirq pulang ke instruksi setelah waitirq (test `test_rtl_cpu_irq_uart_tx`,
  `test_rtl_cpu_irq_timer`) | ✅ interrupt device (UART + timer) |
| **Snapshot mesin (R5 slice, §14)** — `Machine::snapshot/restore`; format `MIVSNAP1` sparse halaman 4 KB (RAM 2 GB → file kecil), atomik, magic+versi; blob CPU per-ISA via `CpuCore::snapshot/restore` (RV32 + x86; Direct RTL ditolak *sebelum* run); restore tolak memory map tak cocok; counter kumulatif lintas run; CLI `--snapshot-save`/`--snapshot-load` | ✅ |
| **Interpreter `--run` ELF (R1/R2)** — `mivon emu --run --load-elf prog.elf` tanpa `--rtl-cpu` menjalankan ELF32 di `Rv32Cpu` sampai `ebreak`; `ebreak` tak pernah jadi `CpuStep::Trap` (trap internal = lompat `mtvec`, desain sengaja) → `Machine` berhenti via `CpuCore::halt_status()` (setara sinyal `trap` Direct RTL CPU); ELF64 ditolak (RV64 menyusul); `mivon emu` tanpa target `.sv` kini valid untuk jalur ini | ✅ |
| **Snapshot Direct RTL CPU (R5, §14)** — `RtlLinkedCpu::snapshot/restore`: `SimCheckpoint` engine (SIM-17/18) lewat `SimulationEngine::checkpoint()/restore_checkpoint()` (refactor dari `save/load_checkpoint` — IO vs state terpisah) + state bus (`cycle`/`last_pc`/`served*`/console/`pending_rx`); guard `pending_runtime_events()` menolak snapshot bila engine tak idle (antrean event/NBA/wait/fork terisi — pesan menyebut antreannya); foreign ticker (VPI/VHPI per step) di-reset restore; e2e deterministik `test_rtl_cpu_snapshot_restore_resume_deterministic` (3 instr → snapshot → resume → hasil identik dgn run penuh) | ✅ |
| Co-sim bus cycle-accurate + mode `hybrid` | ⏳ |

Verifikasi (2026-09-27): `cargo test --workspace` **2688 pass, 0 fail**;
`cargo clippy --workspace --all-targets --all-features` **0 warning**
(full rebuild, `--all-features`); `cargo fmt --check` bersih.

**Bug fix mivon utama (global)**:
1. `flatten_instances` mengonsumsi `top.sub_instances` tanpa mengembalikan →
   `IrDesign.top.sub_instances` selalu kosong → hierarchy tree (melab `--tree`,
   debugger, GUI outline) kosong dan distributed partitioner selalu
   single-partition. Fix: clone daftar instance sebelum flatten, kembalikan
   setelah selesai (flatten.rs).
2. **Const-fold `case` dengan label sinyal** (elaborator stmt.rs): `case
   (1'b1)` / `case (KONST)` dengan label SINYAL (idiom `(* parallel_case *)
   case (1'b1) sel: ...`) di-const-fold saat elaborasi → label sinyal gagal
   const_eval → jatuh ke `default` secara statis → cabang sinyal TIDAK pernah
   dieksekusi (picorv32 `decoded_imm` selalu X → CPU tidak mengeksekusi
   instruksi apa pun). Fix: fold hanya bila case expr KONSTAN dan SEMUA label
   KONSTAN; selain itu case dievaluasi runtime.
3. **`Expr::Paren` hilang dari `collect_sensitivity`** (util/signal_analysis.rs):
   ekspresi `(sig)` membuat sensitivity `assign`/`always_comb` kosong → proses
   tidak re-trigger saat sinyal dalam kurung berubah (bus picorv32 `mem_addr`
   tidak pernah update). Fix: `Expr::Paren(inner)` → sensitif ke inner; sama
   untuk `resolve_expr_signal` (lvalue).
4. Engine step/debugger: `run()` memanggil `initialize_time_zero()` setiap
   re-run (setelah kompaksi event, `push_event(0)` underflow) → guard hanya di
   time 0; `StepMode::StepCycle` break SEBELUM `state.time += 1` → step tidak
   pernah maju waktu → break setelah increment (core.rs).
5. Interpreter `Rv32Cpu`: `csrrw` (op=1) menulis `zimm` (field rs1 sebagai
   angka) bukan nilai register `regs[rs1]` → mtvec salah → trap melompat ke
   alamat salah (riscv32.rs).
6. **`MULH`/`MULHSU` sign-extension** (riscv32.rs): `a as i64` pada u32
   zero-extends (bukan sign-extend) → MULH(-1,-1) PANIC overflow i64 & hasil
   salah; MULHSU juga salah. Fix: sign-extend ke i128
   (`a as u32 as i32 as i128`) + gunakan i128 agar tak overflow.
7. JIT `check_interrupts` (cpu/jit.rs): `MPIE = MIE` dibaca SETELAH `MIE=0`
   → MPIE selalu 0 (salah). Fix: baca MPIE sebelum clear MIE (urutan benar).
8. x86 `INC`/`DEC` OF flag (cpu/x86.rs): logika `ff /0+/1` & `fe` (8-bit)
   keliru — INC OF hanya saat hasil == INT_MIN, DEC OF saat hasil == INT_MAX.
   Sebelumnya terbalik (INC di 0x7fff→0x8000 tak set OF; DEC set OF di 0↔-1).
   Fix konsisten dgn grup `40-4f` yang sudah benar.
9. x86 CPUID leaf 0 vendor string: ECX `0x3c65746e` salah 1 byte
   (0x3c='<', harus 0x6c='l') → vendor `"GenuineInte<"` bukan
   `"GenuineIntel"` → pencocokan string vendor boot bisa gagal. Fix
   `0x6c65746e`; test vendor utuh.

Verifikasi: `cargo test --workspace` **2659 test pass, 0 fail** (18 skip,
termasuk 87 test `mivon-emu`: e2e picorv32 RTL + e2e MMIO UART console
store/read + e2e interrupt device UART (bit 3) + timer device-initiated
(bit 4) + MULH sign-extension + x86 INC/DEC OF).

---

## 20.6 Status R6 x86 ISO boot (2026-08-30)

**Boot ISO Ubuntu nyata** (`--boot-iso ubuntu-26.04-desktop-amd64.iso`) diperbaiki
dari "halt di INT 13h AH=4B" menjadi **menjalankan GRUB kernel di protected mode
tanpa fault** (puluhan–ratusan juta step).

Jalur boot yang benar (bukan MBR hybrid):
`El Torito catalog (LBA 666) → boot image cdboot (LBA 667, no-emul, 2048-unit)
→ cdboot baca boot file (bi_file=667, 31662 B) via AH=42 blok 2048
→ LZMA decompress kernel → GRUB kernel pmode` → Linux (⏳, butuh JIT).

Perbaikan bug + fitur sesi ini (mivon-emu, `mivon emu --boot-iso`):

| Item | Status |
|---|---|
| `iso.rs` — parser El Torito (volume descriptors, boot catalog, boot entry no-emul) + `load_boot_image` (DL=0xE0, `cd_drive` di-set) | ✅ baru, 2 test |
| `INT 13h AH=42` read drive CD (DL==cd_drive): LBA & count dalam blok **2048-byte** (CD), bukan 512 (HDD) — via `X86Disk::read_bytes` | ✅ fix |
| `INT 13h AH=4B AL=01` Get CD-ROM Information → isi CDRP (`media_type=0`, `drive_no=0xE0`) di DS:SI — GRUB biosdisk mengenali CD no-emul | ✅ fix |
| `SHLD/SHRD` (0F A4/A5/AC/AD, imm/cl, 16/32-bit) | ✅ baru, 1 test |
| `ea16` mod=0 rm=6 (`[disp16]` absolute) memakai **DS** bukan SS (menyalahi x86; boot code `mov [abs16]` salah segmen) | ✅ fix |
| DAP `INT 13h AH=42` LBA dibaca **64-bit** (dword tinggi @+12 ikut), bukan 32-bit | ✅ fix |
| `X86Cpu.console_output()` di-wire → `MachineResult.console` — BIOS output (INT 10h teletype `"GRUB "`) kini tampil di CLI | ✅ fix, 1 test |
| Mirror **VGA text buffer** (0xB8000+) di `write8/16/32` + `vga_text()` (untuk console pasca-pmode/kernel) | ✅ baru |
| `iso_boot.meu` RAM 32 MB → **2 GB** (GRUB tulis ~1.06 GB selama decompress) | ✅ config |
| `MIVON_NO_ANIM=1` — nonaktifkan animasi pipeline terminal (untuk session gdb/profiling) | ✅ guard |
| `[profile.prof]` (inherits release, `debug=2`) — biner cepat + info Dwarf penuh untuk gdb membaca state guest | ✅ Cargo |

Profil dengan **gdb** (sampling, `prof` build): thread emulasi tunggal,
hot path = `Machine::run → step → exec_op2 → exec_arith_group → read8 → mem.read`
(akses memori ~40% sampel, decode ~30%). State guest saat sampling:
`pmode=true, cs=8, steps=23-31M, ip 0x1f4908b→0x2e92a13` (walk ~2 B/step)
dengan `EAX=0x1000001` konstan — GRUB sedang menyalin/melakukan relokasi image
terhadap buffer decompress 16 MB. Kesimpulan: boot **bukan** macet — murni
kecepatan interpreter. Langkah berikut (R3): JIT / block-translation untuk
x86 (konversi blok panas e.g. loop copy/relokasi).

### Optimasi interpreter x86 (2026-08-31) — +45% / 6.4M→9.3M instr-s

Dari profil gdb/tracex, diterapkan 2 optimasi di hot path:

1. **Akses memori bulk** (`mem.rs` + `x86.rs`): `MemoryPort` dapat `read_exact`
   / `write_exact` (default = per-byte, non-breaking); `MemoryMap` override
   dengan slice-copy. Dipakai `read16/32`, `write16/32`, string ops
   (`movs/cmps/stos/lods`), dan copy buffer INT 13h AH=42 — bukan lagi
   per-byte (region lookup + vtable per byte).
2. **Prefetch cache instruksi** (`X86Cpu.pf_*`, 16-byte buffer): `fetch8`
   tidak lagi 1 access memori per byte; dibaca bulk 16 byte, di-invalidasi
   pada lompatan. Akses memori fetch turun drastis.

Hasil terukur (`tracex compare` cuman sulit karena tracex belum final; ukur
langsung wall-time): **200M step boot ISO = 31.1s → 21.4-23.0s (~1.4-1.5x)**,
9.3M instr/s (release LTO). Verifikasi: seluruh suite hijau (**2683 pass**,
mivon-emu 97 test).

### tracex (CATATAN.md) — hasil uji dengan sudo

`kernel.perf_event_paranoid=-1` (sudo) → HW counters aktif:
- `tracex stat` ✅ — IPC **0.60** (mixed), cache-miss **22.8%** 🔴 (interpreter
  cache-hostile: guest RAM + kode dispatch), branch-miss **8.3%** 🔴 (big
  opcode match). Arah optimasi JIT ganjil sebab itu.
- `tracex inspect <PID>` ⚠️ — men-sampling SEMUA thread (9 idle = noise 90% S),
  simbol pecah pada release LTO (`GCC_except_table...`). Pakai `prof` build
  atau filter thread running.
- `tracex profile` ⚠️ — mode dev; ringkasan tidak muncul walau child selesai
  (sumo wait 60s). `tracex run/analyze/replay/query` jalan (syscall-level).
- `tracex analyze mivon.trx` ✅ — breakdown syscall (mmap 30, dll).

Verifikasi: `cargo test --workspace` pass (mivon-emu **97 test**, +10 baru).

### Investigasi crash GRUB biosdisk trampolin (2026-09-08)

**Gejala**: boot berhenti di tahap pemilahan region nol. GRUB sukses:
`El Torito → cdboot → LZMA decompress → entry kernel @0x424fe41a`
(step 8,558,021, bytes `55 89 e5 56` = prologue valid). Lalu `grub_bios_interrupt`
(PATOK 0x909c) → prot_to_real (0x82d2) → INT 13h AH=42 (CD read, host stub)
→ real_to_prot (0x830c) → epilogue @0x9163 `ret` pop **0x313b44** dari stack
(alih-alih return CALL @0x424fdbd3 yang disimpan di `[0xf75c]`) → eksekusi
region NOL (`00 00` = `add [al]`,al? — opcode 0x00) selamanya, pc walk 2 B/step.

**Bukti** (`MIVON_X86_DBG` + `MIVON_X86_CALLS`, env-gated di `X86Cpu::step`/
handler call/ret):
- Epilogue 0x915d-0x9163 pop 6 reg (edi/esi/ebx/eax/ecx/ebp) + ret; slot stack
  berisi DATA (`0x4c42430a` "CBL\n", `0x4f4d2e53` "S.MO") bukan register simpanan
  → sp epilogue 4-8 byte terlalu tinggi dari frame yang benar.
- Frame pmode (ret@[0xf75c], 7 push) di-restore ke sp=0x7f740 (cocok save area
  [0x90f3]); `ret` @0x8326 pop `0x9126` (benar); penyimpangan muncul di
  antara restore-sp dan epilogue.
- IVT real mode (0:0x40-0x84) KOSONG → GRUB tidak install-IVT; `int` selalu
  stub host.
- `0x9083` memuat `36 ff 5f 06` = `lcall *%ss:0x6(%edi)` (**ff /3 far call
  belum diimplementasi**) — jalur itu tidak dieksekusi sebelum crash, tapi
  potensi blocker berikutnya.

**Yang diperbaiki sesi ini**:
- `exec_int` (0xcd/0xcc): real mode + IVT terisi → interrupt frame
  (push FLAGS/CS/IP, clear IF) + dispatch ke handler guest; IVT kosong →
  stub host (perilaku lama). Ini semantik x86 yang benar — GRUB yang
  meng-install vektor sendiri akan bekerja.
- Instrumentasi debug env-gated `MIVON_X86_DBG` (per-step state: pc/cs/ip/
  pmode/cr0/sp/gpr + 8 byte kode) dan `MIVON_X86_CALLS` (call/ret + return
  address) — siap dipakai untuk sesi lanjutan.
- `X86Disk::read_bytes`, prefetch instruksi, bulk memory (lihat §Optimasi).

**Sesi lanjutan (2026-09-21)** — trampolin teratasi + `ff /3` call far:
- `ff /3` (CALL FAR m16:16/m16:32) diimplementasi di `exec_ff_group`
  (`crates/mivon-emu/src/cpu/x86.rs`): baca off:seg dari operand memori
  (real mode `[ea]`+`[ea+2]`, pmode `[ea]`+`[ea+4]`), push frame return
  (CS lalu IP/EIP — konsisten dgn frame `int`/`iret`), lompat + recompute
  `pmode`. 2 test baru (real-mode `ff 1e 00 90` → cs=0x1234 ip=0x2000,
  ret frame 0x7c04/0x0000 di stack; pmode `ff 1d <disp32>` → cs=0x8
  eip=0x100000, ret frame push32). Sebelumnya `ff /3` → `halt("ff /N
  belum didukung")` (potensi blocker di `0x9083` `lcall *%ss:0x6(%edi)`).
  Catatan: ModRM `ff 36` = **PUSH** [disp16] (reg=6), bukan call far —
  encoding benar utk `/3` dgn `[disp16]` = `ff 1e`, `[disp32]` pmode =
  `ff 1d`.
- Nama file ISO di test diperbarui `ubuntu-26.04` → `ubuntu-26.04.1
  -desktop-amd64.iso` → 2 test e2e (MBR execute + GRUB boot.img execute)
  hijau lagi.
- Verifikasi e2e `mivon emu --boot-iso ubuntu-26.04.1-desktop-amd64.iso
  --config iso_boot.meu --run --max-steps 200000000` (release): **tidak
  ada fault** — pc=0x170192d1 @200M step (sebelumnya crash region-nol di
  step ~8.5M). Titik crash trampolin biosdisk sudah dilewati; GRUB lanjut
  relokasi image. Console BIOS masih kosong (GRUB pakai VGA text, belum
  teletype) → tahap berikut: JIT / block-translation (R3) utk relokasi.

**Sesi lanjutan (2026-09-21) — crash BUKAN macet: epilogue biosdisk 2**:

- 8 sampel `MIVON_X86_PROGRESS` (tiap 25M, register membeku identik)
  semula tampak "GRUB lambat relokasi" — ternyata **crash epilogue
  BIOS (jalur kedua)**: ret @0x9163 pop **0x313b44** (data file GRUB,
  bukan alamat) → eksekusi region kosong (`00 00` = add [eax],al),
  pc walk 2 B/step selamanya. Pola IDENTIK dgn crash 2026-09-08, hanya
  bergeser ke step **8,623,675** (sempat lewat 8.5M berkat `ff /3`+int).
- Peta trampolin GRUB nyata (dump `probe_tramp`): variabel handler di
  `[0x9041]=0x82d2` (real→prot; dipanggil real handler @0x9123
  `66 ff d0` call eax) dan `[0x9045]=0x8327` (prot→real; dipanggil
  pmode @0x90e6). Fungsi trampolin pertama (INT 13h stub) **sukses**
  (ret → 0xbc16); jalur kedua (INT 13h AH=42 read berikutnya) **crash**.
- `ff d0` terverifikasi benar = `ff /2` CALL NEAR eax (modrm d0: mod=11
  reg=2 rm=0) — bukan inc (`ff c0`). Decoder akurat.
- Dump stack epilogue jalur-2: retaddr benar `0x9126` ditulis
  real_to_prot (`89 04 24` @0x8313) ke `[0x7f740]`; epilogue pop 6+ret
  mulai `[0x7f748]` → **frame bergeser 8 byte**; slot register abret
  berisi nilai state saat itu (`0x087ed815`, `0x101f061a`, `0x1000001`,
  `"CBL\n"`, `"S.MO"`) bukan simpanan; ret @`[0x7f760]` = 0x313b44.
- Tool baru: `MIVON_X86_PROGRESS=1` (state tiap 25M step, release boot
  panjang), `MIVON_X86_DBG_WIN="from:to"` (window trace env-driven),
  contoh `probe_tramp` (dump trampolin + stack + variabel).
- Fast path bulk `rep movs`/`rep stos` (n>1, count ≥ 32, tanpa VGA):
  salin/isi utuh via chunk 4KB `read/write_exact` (MemoryMap slice-copy),
  hemat per-iterasi perbyte (infra R3 relokasi). Fix semantik: `rep`
  dengan count 0 = no-op (si/di tetap). 3 unit test (movs/stos bulk +
  count-0). Boot 10M step deterministik identik (pc=0x005b3bd0) — tidak
  regress.
- **Root cause epilogue 2 TERBUKTI (write-trace `MIVON_X86_WTRACE`)**:
  INT 13h AH=42 CD read (step 8,623,612) menulis **96,256 byte
  (0x68000..0x7F800, 47 sektor×2048)** — **menimpa frame stack pmode
  grub_bios_interrupt (0x7f744..0x7f760)** yg baru di-push prologue
  (8,623,542-552). Slot register frame + retaddr (0x424fdbd8, caller
  kernel) jadi data buffer → epilogue pop data + ret 0x313b44 → region
  kosong. Bukan bug decoder; di hardware GRUB menempatkan buffer tanpa
  menimpa frame.
- **FIX root cause (2026-09-21)**: real-mode addressing 16-bit pada
  INT 13h AH=42 buffer write — **offset me-wrap per segmen 64KB** (byte
  ke-65537 kembali ke awal segmen, bukan linear lanjut). Tanpa wrap, read
  CD besar (47×2048=96KB dari seg 0x6800) menembus ke 0x7F800 dan menimpa
  stack pmode → crash epilogue. Dengan wrap: buffer tetap dalam segmen
  (0x68000..0x78000), frame aman. 1 unit test
  (`test_int13_dap_segment_wrap_16bit`). **Efek: boot lewat step
  8,647,411 (lewat crash 8,623,675) — halt baru: opcode 0f a3 (BT)
  belum didukung.**
- **BT/BTS/BTR/BTC (0f a3/ab/b3/bb) + grup 0f ba /4-7 diimplementasi**
  (CF = bit; bts/btr/btc = set/clear/toggle) — 2 unit test. GRUB kernel
  sekarang lanjut melewati titik halt BT pertama.
- **Hasil boot setelah fix (release, deterministic)**: 15M–200M step
  TANPA fault — melewati crash epilogue 2 (8,623,675) & BT halt
  (8,647,411). Console kini berisi 2 byte (LF+CR) — GRUB mulai
  mencetak. Namun INT13 hanya 7× total (terakhir 8,623,934); sejak itu
  GRUB berputar membosankan di loop prot↔real ↔ kernel real-mode
  0xd0xx: pc=0xd1e1 `test dl,dl; jz` dengan dl=0x20 & sp statis
  0x7f5a4 — spin menunggu variabel/pointer yang tak berubah
  (kemungkinan routine print string / input yang macet). Menghabiskan
  31M+ step tanpa kemajuan selanjutnya.
- Tool baru (debug, env-gated, off default): `MIVON_X86_WTRACE=addr:len`
  — trace tulis CPU (write8/16/32 + bulk INT13 + fast path movs/stos)
  dengan step/pc/addr/val; window parse hex "0x..." maupun desimal.
- Langkah berikut: trace penuh loop 0xd1e1 (& caller 0x424f...) — cari
  instruksi yang seharusnya memajukan pointer/char (termasuk dlm print
  string, mungkin VGA text 0xB8000), dan/atau cek menunggu input
  (INT 16h keyboard stub) yang tidak pernah selesai.

**Sesi lanjutan (2026-09-22)**:
- Loop 0xd1e1 TERIDENTIFIKASI = traversal **string printf GRUB biosdisk**
  (`8a 13 mov dl,[ebx]; test dl,dl; jz; cmp dl,'%'; lea eax,[ebx+1];
  mov ebx,eax; jmp`) — string di 0x424fe55d = `"failure reading sector
  0x%llx from \`%s'"` dst, NUL-terminated SEMPURNA; ebx maju +1/iterasi;
  loop normal, bukan spin. GRUB kernel hidup pasca-crash; 200M step:
  pc bervariasi (0x830c/0x8352/0xf168/0x82d5/0x90d6/0xf224/0x90a2),
  halted=false, console [LF,CR] (=2 byte), **VGA text (0xB8000) masih
  kosong** → GRUB belum mencetak menu/welcome. Kemungkinan masih tahap
  panjang (module load / init runtime) atau menunggu sesuatu.
- probe_tramp: dump string 0x424fe540 + VGA text + run 200M.
- Langkah berikut: cek apakah GRUB menunggu input (INT 16h / keyboard
  buffer) — tambah dukungan INT 16h AH=00/AH=01 (jika stub), dan cek
  kenapa VGA text belum terisi; ukur lintasan pc per 10M step utk
  membedakan "aktif bergerak" vs "siklus periodik".

**Tidak teratasi / langkah berikut**:
- Akar 4-byte drift stack di jalur prot_to_real↔real_to_prot belum
  terisolasi instruksi-demi-instruksi — kini TIDAK memblock boot (crash
  trampolin teratasi); diff per-instruksi vs QEMU opsional utk bukti
  formal (window 0x82d2-0x8326).
- Referensi differential QEMU
  (`qemu-system-i386 -d in_asm,cpu`) tidak sampai trampolin (SeaBIOS idle
  di boot menu headless — perlu `-nographic -no-reboot` + opsi menu off /
  input). Bochs tak bisa (plugin display rusak).
- Rekomendasi: (a) diff per-instruksi vs QEMU pada window prot_to_real
  (0x82d2-0x8326) — cari instruksi yang menghasilkan sp berbeda (opsional
  pasca-trampolin-hijau); (b) `ff /3` + `iret` frame penuh — ✅ selesai;
  (c) GRUB lanjut ke relokasi → **JIT (R3) block-translation loop
  copy/relokasi** (target berikut).

---

## 21. Roadmap — Urutan Implementasi

Urutan paling masuk akal (dari desain user):

```
MHIR → Memory/Bus → CPU interpreter → device model → Linux boot
     → JIT → RTL device bridge → deterministic replay
     → Windows/UEFI → full hybrid co-emulation
```

| Fase | Isi | Milestone / bukti sukses |
|---|---|---|
| **R0** | **MHIR** (`mivon-emu::mhir`): ekstraksi register/device/address-map dari IrDesign+netlist + back-pointer source; anotasi `(* mivon_region *)`; `[emu]` parse | `--dump-memory-map` benar; bare-metal ELF jalan di cva6 lewat Tier A |
| **R1** | **Memory/Bus**: `mem::RamRegion` (mmap), `MemoryPort`, decode bus; ELF loader | Bare-metal ELF jalan; akses RAM zero-copy |
| **R2** | **CPU interpreter** (RISC-V32/64, benar dulu) + `CpuCore` trait; CLINT/PLIC + UART native; boot flow OpenSBI; DTB builder; virtio-mmio + virtio-blk | ✅ RV32IM+Zicsr interpreter (riscv32.rs); ✅ RTL-linked CPU (mode 3, picorv32 boot bare-metal); ⏳ RV64 + vmlinux/initrd boot di cva6 |
| **R3** | **JIT** (Tier C): basic-block translation via Cranelift + softmmu/TLB + MMIO trap + interrupt delivery | Boot Linux < 30 s; `$` shell; `uname -a`; ISO Linux RISC-V boot |
| **R4** | **RTL device bridge** (Direct RTL Device end-to-end): UART RTL via bus co-sim; Tier B (cycle-based compiled) untuk peripheral RTL | ✅ store+read `0x10000000` → `uart_console.sv` RTL → host console + status register (`rv32_soc`, 4 test e2e); ✅ interrupt device UART bit 3 + timer bit 4 device-initiated (picorv32 `ENABLE_IRQ` → handler `PROGADDR_IRQ` → `eoi` ack → `retirq`, console "AB"/"T"); ⏳ mode `hybrid` |
| **R5** | **Deterministic replay** + snapshot penuh (machine state); Virtual Time lengkap | `mivon replay trace.bin` reproduksi bug; snapshot boot < 1 s |
| **R6** | **Windows/UEFI**: mesin x86-64 (translation ISA ketiga) + chipset (PIC/APIC/ACPI minimal) → phase bootloader → kernel → device init → Safe Mode | Windows bootloader + kernel (functional); desktop = R6 lanjutan |
| **R7** | **Full hybrid co-emulation**: multi-core SMP, per-region accuracy, distribusi lintas host, VHDL/SystemC frontend | SMP Linux boot; mode `coemu` penuh; SoC VHDL boot |

**Catatan scope**: R0–R4 adalah jalur kritis (Linux RISC-V + Direct RTL Device —
identitas Mivon). R5–R7 pararel opsional. Windows (**R6**) = investasi terbesar,
baru realistis setelah JIT terbukti di RISC-V dan ARM64.

---

## 22. Risiko & Mitigasi

| Risiko | Dampak | Mitigasi |
|---|---|---|
| MHIR ekstraksi tidak lengkap (register/device tak terdeteksi) | Machine salah | Anotasi eksplisit + deteksi struktural; fallback ke native device |
| Performa JIT < target | Boot lambat | Codegen flat, 2-state, domain-parallel; ukur di R3 |
| Translation CPU sulit (trap, privilege, atomics) | R3 molor | Interpreter benar dulu, translate per-block bertahap; test differential vs Tier A |
| Interrupt/timing OS sensitif | Hang/gagal boot | CLINT tick berbasis instruksi; verifikasi bertahap: bare-metal → initramfs → full Linux |
| SoC RTL kompleks (cva6) tak ter-elaborasi penuh | R0 tersendat | Target bergantian: picorv32 (✅) → Ibex → cva6; subset yang dibutuhkan boot |
| Sandbox bocor (guest akses host langsung) | Keamanan | Semua akses host lewat `mivon-emu::sandbox` + izin eksplisit; audit |
| Non-determinisme (wall-clock, host timer) | Replay gagal | Virtual Time murni + seed; host hanya untuk I/O |
| ISO x86 = scope besar | Windows molor | Eksplisit di R6; jalur alternatif: Linux RISC-V + bare-metal menutup mayoritas verifikasi produk |

---

## 23. Benchmark Target

| Benchmark | Target |
|---|---|
| Tier B eval rate (cva6-scale) | ≥ 100 MHz cycle (single-thread) |
| JIT IPC vs interpreter | ≥ 5–10× |
| Boot Linux (vmlinux+initrd, cva6) | < 30 s (functional) |
| Boot dari ISO Linux RISC-V | < 60 s |
| Snapshot save/restore | < 1 s |
| Replay determinism | bit-identik antar run (seed sama) |

---

## 24. Kaitannya dengan Fitur Mivon yang Ada

| Aset mivon | Dipakai untuk |
|---|---|
| `SimulationEngine` (13-region scheduler) | RTL Engine (Tier A), backend MMIO co-sim |
| `ClockDomainAnalysis` + cycle fusion | Tier B (per-domain eval) |
| `SimulationDag` + parallel eval | Topological order + paralelisme Tier B |
| Cranelift JIT (`jit` feature) | JIT CPU + JIT eval Tier B |
| `mivon-sir` / netlist (FF inference) | MHIR extraction, deteksi core, memory map |
| Distributed sim (master/slave) | R7: SMP lintas host |
| Foreign loader (VHPI/PLI/DPI, dlopen) | SystemC bridge (R7) |
| `picorv32` compile+sim ✅, `cva6/`, `openc910/` di repo | Target uji CPU / boot Linux |
| Checkpoint (`SIM-17/18`) | Dasar snapshot engine (R5) |
| Coverage, SDF, UPF | Mode cycle-accurate: regression + timing + power |

---

## 25. Keputusan yang Sudah Diambil

1. **Mivon = Hardware-Software Emulator**, bukan "QEMU dalam Rust".
2. **OS tidak dibundel** — media boot (ISO/raw/kernel) sepenuhnya dari user.
3. **Dua engine terpisah** (RTL Engine + Machine Engine) + co-simulation;
   `execution_mode` per-device (`RTL | JIT | native`).
4. **MHIR adalah jantung** — ekstraksi hardware + back-pointer source.
5. **Direct RTL Device** dan **cross-layer debugger** adalah pembeda utama.
6. **Engine full-Rust** — Interpreter + JIT (Cranelift), tanpa QEMU.
7. **5 mode operasi**: `rtl` · `sim` · `emu` · `hybrid` · `coemu`.
8. **Dual-mode akurasi**: functional ↔ cycle-accurate, per-device.
9. **Sandbox**: OS tamu tidak pernah menyentuh host langsung.
10. **Deterministik**: seed + Virtual Time + `mivon replay trace.bin`.
11. **Multi-ISA bertahap**: RISC-V (R0–R4) → ARM64 → x86-64 (R6, jalur Windows).
12. **Urutan implementasi**: MHIR → Memory/Bus → CPU interpreter → device model
    → Linux boot → JIT → RTL device bridge → deterministic replay → Windows/UEFI
    → full hybrid co-emulation.
