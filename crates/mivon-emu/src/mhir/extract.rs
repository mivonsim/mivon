//! Ekstraksi MHIR dari `IrDesign` — EMULATOR.md §4, fase R0.
//!
//! Sumber data:
//! - Clock/reset: proses `Sequential` (`always_ff`).
//! - Register: target `NonBlockingAssign` di proses `Sequential` (inferensi FF).
//! - Memory: signal dengan `array_depth > 0` (array signal → RAM/ROM).
//! - Device: `sub_instances` (instance di bawah module) + klasifikasi dari
//!   nama module; port diambil dari definisi module (bila tersedia).
//! - Back-pointer: instance punya `line/col` asli; signal/register di-resolve
//!   via `SourceLocator` dari `IrDesign.source_lines`.

use mivon_core::intern::Symbol;
use mivon_ir::{ClockEdge, IrDesign, IrLValue, IrModule, IrStmt, Process, SignalId};

use super::backptr::SourceLocator;
use super::types::*;

/// Ekstrak MHIR dari seluruh desain: module top (flattened) + definisi module
/// lain (diurutkan nama, deterministik).
pub fn extract(design: &IrDesign) -> MhirDesign {
    let locator = SourceLocator::new();
    let lines: &[String] = design.source_lines.as_deref().unwrap_or(&[]);
    let mut mhir = MhirDesign {
        top: design.top.name,
        source_file: design.source_file.clone(),
        ..Default::default()
    };

    mhir.modules
        .push(extract_module(&design.top, design, &locator, lines));

    let mut names: Vec<Symbol> = design.modules.keys().copied().collect();
    names.sort_by_key(|s| s.as_str().to_string());
    for n in names {
        if n == design.top.name {
            continue;
        }
        if let Some(m) = design.modules.get(&n) {
            mhir.modules
                .push(extract_module(m, design, &locator, lines));
        }
    }

    // Anotasi `(* mivon_region = "...", base = ..., size = ... *)` pada
    // instance (EMULATOR.md §10) → address_map, supaya `--dump-memory-map`
    // langsung menampilkannya. `apply_address_map` (CLI `--addr` / config
    // `.meu`) menimpa entri nama yang sama — config > anotasi.
    let annotated: Vec<(Symbol, AddressRegion)> = mhir
        .modules
        .iter()
        .flat_map(|m| m.devices.iter().filter_map(|d| d.mmio.map(|r| (d.name, r))))
        .collect();
    mhir.address_map.extend(annotated);
    mhir.address_map
        .sort_by_key(|(n, r)| (r.base, n.as_str().to_string()));
    mhir
}

/// Parse integer anotasi: `0x...`/`0X...` (hex) atau desimal. Selain itu None.
fn parse_int(s: &str) -> Option<u64> {
    let t = s.trim();
    match t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .map(|h| u64::from_str_radix(h, 16))
    {
        Some(r) => r.ok(),
        None => t.parse::<u64>().ok(),
    }
}

/// Baca anotasi Mivon pada satu instance (EMULATOR.md §10):
/// - `(* mivon_region = "mmio", base = "0x10000000", size = "0x1000" *)`
///   → `Some(AddressRegion)` — `base`/`size` hex atau desimal;
/// - `(* mivon_irq = "5" *)` → `Some(irq)`.
///
/// Atribut lain (milik tool sintesis/lint) diabaikan; `mivon_region` tanpa
/// `base`/`size` lengkap → dianggap tidak ada region (lebih baik None daripada
/// region 0 yang menyesatkan).
fn attr_region_irq(inst: &mivon_ir::IrInstance) -> (Option<AddressRegion>, Option<u32>) {
    let val = |k: &str| -> Option<u64> {
        inst.attrs
            .iter()
            .find(|a| a.key.as_str() == k)
            .and_then(|a| a.value.as_deref())
            .and_then(parse_int)
    };
    let region = if inst.attrs.iter().any(|a| a.key.as_str() == "mivon_region") {
        match (val("base"), val("size")) {
            (Some(base), Some(size)) if size > 0 => Some(AddressRegion { base, size }),
            _ => None,
        }
    } else {
        None
    };
    (region, val("mivon_irq").map(|v| v as u32))
}

/// Ekstrak satu module (definisi atau top flattened).
fn extract_module(
    module: &IrModule,
    design: &IrDesign,
    locator: &SourceLocator,
    lines: &[String],
) -> MhirModule {
    let mut out = MhirModule {
        name: module.name,
        signal_count: module.signals.len(),
        ..Default::default()
    };

    // ── Clock & reset dari proses Sequential ──
    let mut clock_names: Vec<Symbol> = Vec::new();
    let mut reset_names: Vec<Symbol> = Vec::new();
    for proc in &module.processes {
        if let Process::Sequential { clock, reset, .. } = proc {
            let (cname, hier) = clock_desc(module, clock);
            if !clock_names.contains(&cname) {
                clock_names.push(cname);
                out.clocks.push(ClockDesc {
                    name: cname,
                    signal_id: clock_signal_id(clock),
                    edge: match clock {
                        ClockEdge::NegEdge(_) | ClockEdge::NegEdgeHier(_) => ClockEdgeKind::NegEdge,
                        _ => ClockEdgeKind::PosEdge,
                    },
                    hier,
                });
            }
            if let Some(r) = reset {
                let rname =
                    signal_name(module, r.signal).unwrap_or_else(|| Symbol::intern("<unknown>"));
                if !reset_names.contains(&rname) {
                    reset_names.push(rname);
                    out.resets.push(ResetDesc {
                        signal: rname,
                        polarity: r.polarity,
                        async_: r.r#async,
                    });
                }
            }
        }
    }

    // ── Register (inferensi FF: target NBA di proses Sequential) ──
    let mut seen_regs: Vec<Symbol> = Vec::new();
    for proc in &module.processes {
        let (Some(clock), Some(reset)) =
            (proc_clock_name(module, proc), proc_reset_name(module, proc))
        else {
            continue;
        };
        let body = match proc {
            Process::Sequential { body, .. } => body,
            _ => continue,
        };
        for_each_nba(body, &mut |lhs| {
            for sig_id in lvalue_signal_ids(lhs) {
                let Some(sig) = module.signals.get(sig_id) else {
                    return;
                };
                if !seen_regs.contains(&sig.name) {
                    seen_regs.push(sig.name);
                    out.registers.push(MhirRegister {
                        name: sig.name,
                        width: sig.width,
                        clock: Some(clock),
                        reset: Some(reset),
                        back: locator.locate(lines, sig.name),
                    });
                }
            }
        });
    }

    // ── Memory (array signal) ──
    // Array unpacked di-flatten jadi SATU vector: `sig.width` = total bit
    // (elem_width × depth), `sig.elem_width` = lebar per elemen.
    // `array_depth` = 1 untuk signal SKALAR (bukan array) → hanya depth > 1
    // yang merupakan memory sungguhan (mis. `mem [0:1023]` → depth 1024).
    for sig in &module.signals {
        if sig.array_depth > 1 {
            let kind = if sig.is_const {
                MemoryKind::Rom
            } else {
                MemoryKind::Ram
            };
            out.memories.push(MhirMemory {
                name: sig.name,
                elem_width: if sig.elem_width > 0 {
                    sig.elem_width
                } else {
                    sig.width
                },
                depth: sig.array_depth,
                dims: sig.array_dims.clone(),
                kind,
                back: locator.locate(lines, sig.name),
            });
        }
    }

    // ── Device (instance di bawah module) ──
    // flatten.rs memakai `std::mem::take(&mut top.sub_instances)` → module top
    // FLATTENED punya sub_instances kosong. Instance asli masih ada di definisi
    // pre-flatten (`design.modules[name]`). Untuk module lain, get() mengembalikan
    // module yang sama (sub_instances sama).
    let instances = design
        .modules
        .get(&module.name)
        .map(|m| &m.sub_instances)
        .unwrap_or(&module.sub_instances);
    for inst in instances {
        let kind = DeviceKind::from_module_name(inst.module_name.as_str());
        let ports = module_ports(design, inst.module_name);
        // Anotasi Mivon pada instance (EMULATOR.md §10): region MMIO + IRQ.
        let (mmio, irq) = attr_region_irq(inst);
        out.devices.push(MhirDevice {
            name: inst.instance_name,
            module: inst.module_name,
            kind,
            ports,
            mmio,
            irq,
            back: BackPointer::known(None, inst.line, inst.col),
        });
    }

    out
}

/// Nama clock dari proses `Sequential`.
fn proc_clock_name(module: &IrModule, proc: &Process) -> Option<Symbol> {
    match proc {
        Process::Sequential { clock, .. } => {
            let (name, _hier) = clock_desc(module, clock);
            Some(name)
        }
        _ => None,
    }
}

/// Nama reset dari proses `Sequential` (None bila tanpa reset).
fn proc_reset_name(module: &IrModule, proc: &Process) -> Option<Symbol> {
    match proc {
        Process::Sequential { reset, .. } => reset
            .as_ref()
            .map(|r| signal_name(module, r.signal).unwrap_or_else(|| Symbol::intern("<unknown>"))),
        _ => None,
    }
}

fn clock_signal_id(clock: &ClockEdge) -> SignalId {
    match clock {
        ClockEdge::PosEdge(id) | ClockEdge::NegEdge(id) => *id,
        ClockEdge::PosEdgeHier(_) | ClockEdge::NegEdgeHier(_) => usize::MAX,
    }
}

/// (nama, hier) untuk clock — hierarkis (`posedge b.clk`) memakai Symbol path.
fn clock_desc(module: &IrModule, clock: &ClockEdge) -> (Symbol, Option<Symbol>) {
    match clock {
        ClockEdge::PosEdge(id) | ClockEdge::NegEdge(id) => (
            signal_name(module, *id).unwrap_or_else(|| Symbol::intern("<unknown>")),
            None,
        ),
        ClockEdge::PosEdgeHier(s) | ClockEdge::NegEdgeHier(s) => (*s, Some(*s)),
    }
}

/// Resolve SignalId → nama via daftar signal module (bounds-checked).
fn signal_name(module: &IrModule, id: SignalId) -> Option<Symbol> {
    module.signals.get(id).map(|s| s.name)
}

/// Kumpulkan target SignalId dari sebuah lvalue (semua bentuk seleksi).
fn lvalue_signal_ids(lhs: &IrLValue) -> Vec<SignalId> {
    match lhs {
        IrLValue::Signal(id, _)
        | IrLValue::RangeSelect(id, _, _)
        | IrLValue::BitSelect(id, _)
        | IrLValue::ExprPartSelect { sig_id: id, .. } => vec![*id],
        IrLValue::ArrayIndex { sig_id, .. }
        | IrLValue::ArrayRangeSelect { sig_id, .. }
        | IrLValue::ArrayBitSelect { sig_id, .. }
        | IrLValue::ObjectField { sig_id, .. } => vec![*sig_id],
        IrLValue::Concat(items) => items.iter().flat_map(lvalue_signal_ids).collect(),
        // HierRef: nama hierarkis — bukan SignalId lokal; R0: dilewati.
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } => Vec::new(),
        // VifField: target baru diketahui saat runtime (binding vif).
        IrLValue::VifField { .. } => Vec::new(),
    }
}

/// Jalankan `f` untuk setiap lvalue NonBlockingAssign di seluruh pohon stmt.
fn for_each_nba(stmts: &[IrStmt], f: &mut dyn FnMut(&IrLValue)) {
    for s in stmts {
        match s {
            IrStmt::NonBlockingAssign { lhs, .. } => f(lhs),
            IrStmt::Block { stmts: inner }
            | IrStmt::NamedBlock { stmts: inner, .. }
            | IrStmt::Delay { body: inner, .. }
            | IrStmt::Wait { body: inner, .. }
            | IrStmt::EventControl { body: inner, .. } => for_each_nba(inner, f),
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                for_each_nba(true_branch, f);
                for_each_nba(false_branch, f);
            }
            IrStmt::Case { items, default, .. } => {
                for item in items {
                    for_each_nba(&item.body, f);
                }
                for_each_nba(default, f);
            }
            IrStmt::LoopFor { body, .. }
            | IrStmt::LoopWhile { body, .. }
            | IrStmt::LoopDoWhile { body, .. }
            | IrStmt::Repeat { body, .. }
            | IrStmt::Foreach { body, .. } => for_each_nba(body, f),
            IrStmt::Fork { processes, .. } => {
                for p in processes {
                    for_each_nba(p, f);
                }
            }
            _ => {}
        }
    }
}

/// Port (nama, arah, lebar) dari definisi module (bila tersedia).
fn module_ports(design: &IrDesign, module_name: Symbol) -> Vec<PortDesc> {
    let Some(m) = design.modules.get(&module_name) else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    for &id in &m.inputs {
        if let Some(s) = m.signals.get(id) {
            ports.push(PortDesc {
                name: s.name,
                direction: PortDir::Input,
                width: s.width,
            });
        }
    }
    for &id in &m.outputs {
        if let Some(s) = m.signals.get(id) {
            ports.push(PortDesc {
                name: s.name,
                direction: PortDir::Output,
                width: s.width,
            });
        }
    }
    for &id in &m.inouts {
        if let Some(s) = m.signals.get(id) {
            ports.push(PortDesc {
                name: s.name,
                direction: PortDir::Inout,
                width: s.width,
            });
        }
    }
    ports
}

/// Terapkan peta alamat user (`--addr NAME=BASE:SIZE` / `[emu]`) ke MHIR:
/// isi `mmio` device dan `address_map` (diurutkan by base, deterministik).
pub fn apply_address_map(mhir: &mut MhirDesign, entries: &[(Symbol, AddressRegion)]) {
    for (name, region) in entries {
        // Cocokkan nama instance device dulu, lalu nama module device,
        // lalu nama memory.
        let mut matched = false;
        for m in &mut mhir.modules {
            for d in &mut m.devices {
                if d.name == *name || d.module == *name {
                    d.mmio = Some(*region);
                    matched = true;
                }
            }
        }
        if matched {
            // Ganti entri lama (mis. dari anotasi `(* mivon_region *)`) —
            // config/CLI menang, tidak ada baris ganda di address_map.
            mhir.address_map.retain(|(n, _)| n != name);
            mhir.address_map.push((*name, *region));
        }
    }
    // Deterministik: urutkan by base (tie → nama).
    mhir.address_map
        .sort_by_key(|(n, r)| (r.base, n.as_str().to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ir::IrDesign;

    /// Compile source SV → IrDesign (pipeline sama dengan mivon-api::compile_str).
    fn compile(src: &str) -> IrDesign {
        let mut pp = mivon_parser::preprocessor::Preprocessor::new();
        let preprocessed = pp.preprocess(src, None).expect("preprocess");
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
            "<test>".to_string()
        } else {
            file_line_map[0].2.clone()
        };
        let mut parser = mivon_parser::Parser::new(tokens, &first_source)
            .with_source_lines(&preprocessed)
            .with_file_line_map(file_line_map);
        let design = parser.parse_design().expect("parse");
        let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
        let mut elaborator =
            mivon_elaboration::Elaborator::with_source(design, source_lines, first_source);
        elaborator
            .elaborate(None, mivon_elaboration::ElaborateMode::StrictSimulation)
            .expect("elaborate")
    }

    const SOC: &str = r#"
module cpu_core (
    input  logic        clk,
    input  logic        rst_n,
    output logic [31:0] pc
);
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) pc <= 32'h8000_0000;
    else        pc <= pc + 4;
  end
endmodule

module uart (
    input  logic       clk,
    input  logic       rst_n,
    input  logic [7:0] data_in,
    output logic       tx
);
  logic [7:0] data_reg;
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) data_reg <= 8'h00;
    else        data_reg <= data_in;
  end
  assign tx = data_reg[0];
endmodule

module soc (
    input  logic clk,
    input  logic rst_n,
    output logic tx
);
  logic [31:0] cpu_pc;
  logic [7:0]  uart_data;
  logic [31:0] mem [0:1023];
  logic [15:0] rom [0:255];
  cpu_core u_cpu (.clk(clk), .rst_n(rst_n), .pc(cpu_pc));
  uart u_uart (.clk(clk), .rst_n(rst_n), .data_in(uart_data), .tx(tx));
  assign uart_data = cpu_pc[7:0];
endmodule
"#;

    #[test]
    fn test_extract_top_and_modules() {
        let design = compile(SOC);
        let mhir = extract(&design);
        assert_eq!(mhir.top.as_str(), "soc");
        assert!(mhir.modules.len() >= 3, "top + cpu_core + uart");
        assert!(mhir.modules.iter().any(|m| m.name.as_str() == "cpu_core"));
        assert!(mhir.modules.iter().any(|m| m.name.as_str() == "uart"));
    }

    #[test]
    fn test_extract_register_ff_inference() {
        let design = compile(SOC);
        let mhir = extract(&design);
        let cpu = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "cpu_core")
            .unwrap();
        let pc = cpu
            .registers
            .iter()
            .find(|r| r.name.as_str() == "pc")
            .expect("register pc");
        assert_eq!(pc.width, 32);
        assert_eq!(pc.clock.map(|c| c.as_str()), Some("clk"));
        assert_eq!(pc.reset.map(|r| r.as_str()), Some("rst_n"));
        // Back-pointer ter-resolve: baris deklarasi pc di cpu_core.
        assert!(pc.back.line > 0, "back-pointer pc: {:?}", pc.back);

        let uart = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "uart")
            .unwrap();
        let dr = uart
            .registers
            .iter()
            .find(|r| r.name.as_str() == "data_reg")
            .expect("register data_reg");
        assert_eq!(dr.width, 8);
        assert!(dr.back.line > 0);
    }

    #[test]
    fn test_extract_clock_and_reset() {
        let design = compile(SOC);
        let mhir = extract(&design);
        let cpu = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "cpu_core")
            .unwrap();
        assert_eq!(cpu.clocks.len(), 1);
        assert_eq!(cpu.clocks[0].name.as_str(), "clk");
        assert_eq!(cpu.clocks[0].edge, ClockEdgeKind::PosEdge);
        assert_eq!(cpu.resets.len(), 1);
        assert_eq!(cpu.resets[0].signal.as_str(), "rst_n");
        assert!(!cpu.resets[0].polarity, "rst_n = active-low");
        assert!(cpu.resets[0].async_, "negedge rst_n di sensitivity = async");
    }

    #[test]
    fn test_extract_continuous_assign_is_not_register() {
        let design = compile(SOC);
        let mhir = extract(&design);
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        // uart_data/tx hanya di-assign continuous (`assign`) → bukan register.
        // (cpu_pc TIDAK dicek: setelah flatten, port output u_cpu alias ke
        // cpu_pc sehingga register pc anak terlihat menulis cpu_pc — benar.)
        assert!(!top.registers.iter().any(|r| r.name.as_str() == "uart_data"));
        assert!(!top.registers.iter().any(|r| r.name.as_str() == "tx"));
    }

    #[test]
    fn test_extract_memory_arrays() {
        let design = compile(SOC);
        let mhir = extract(&design);
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        let mem = top
            .memories
            .iter()
            .find(|m| m.name.as_str() == "mem")
            .expect("mem");
        assert_eq!(mem.elem_width, 32);
        assert_eq!(mem.depth, 1024);
        assert_eq!(mem.kind, MemoryKind::Ram);
        let rom = top
            .memories
            .iter()
            .find(|m| m.name.as_str() == "rom")
            .expect("rom");
        assert_eq!(rom.elem_width, 16);
        assert_eq!(rom.depth, 256);
        assert_eq!(rom.kind, MemoryKind::Ram, "bukan const → Ram");
    }

    #[test]
    fn test_extract_devices_and_ports() {
        let design = compile(SOC);
        let mhir = extract(&design);
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        let cpu = top
            .devices
            .iter()
            .find(|d| d.name.as_str() == "u_cpu")
            .expect("u_cpu");
        assert_eq!(cpu.kind, DeviceKind::Cpu);
        assert_eq!(cpu.module.as_str(), "cpu_core");
        assert!(cpu.back.line > 0, "back-pointer instance");
        assert!(cpu
            .ports
            .iter()
            .any(|p| p.name.as_str() == "pc" && p.direction == PortDir::Output && p.width == 32));

        let uart = top
            .devices
            .iter()
            .find(|d| d.name.as_str() == "u_uart")
            .expect("u_uart");
        assert_eq!(uart.kind, DeviceKind::Uart);
        assert!(uart
            .ports
            .iter()
            .any(|p| p.name.as_str() == "data_in" && p.direction == PortDir::Input));
        assert!(uart
            .ports
            .iter()
            .any(|p| p.name.as_str() == "tx" && p.direction == PortDir::Output));
    }

    /// Sumber dengan anotasi Mivon (EMULATOR.md §10) pada instance.
    const ANN: &str = r#"
module uart (
    input  logic clk,
    input  logic rst_n,
    output logic tx
);
  assign tx = 1'b0;
endmodule

module soc (
    input  logic clk,
    input  logic rst_n,
    output logic tx
);
  (* mivon_region = "mmio", base = "0x10000000", size = "0x1000" *)
  (* mivon_irq = "5" *)
  uart u_uart (.clk(clk), .rst_n(rst_n), .tx(tx));
endmodule
"#;

    #[test]
    fn test_annotation_region_irq_and_address_map() {
        let design = compile(ANN);
        let mhir = extract(&design);
        assert_eq!(mhir.top.as_str(), "soc");
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        let uart = top
            .devices
            .iter()
            .find(|d| d.name.as_str() == "u_uart")
            .expect("u_uart");
        assert_eq!(
            uart.mmio,
            Some(AddressRegion {
                base: 0x1000_0000,
                size: 0x1000
            }),
            "region dari anotasi (* mivon_region *)"
        );
        assert_eq!(uart.irq, Some(5), "irq dari anotasi (* mivon_irq *)");
        // address_map terisi otomatis dari anotasi (sorted by base).
        assert!(
            mhir.address_map
                .iter()
                .any(|(n, r)| n.as_str() == "u_uart" && r.base == 0x1000_0000),
            "address_map berisi u_uart dari anotasi: {:?}",
            mhir.address_map
        );
    }

    #[test]
    fn test_annotation_overridden_by_config() {
        // Config/CLI (`--addr` / `.meu`) menimpa anotasi — satu entri, nilai
        // config menang, tidak ada baris ganda.
        let design = compile(ANN);
        let mut mhir = extract(&design);
        apply_address_map(
            &mut mhir,
            &[(
                Symbol::intern("u_uart"),
                AddressRegion {
                    base: 0x2000_0000,
                    size: 0x200,
                },
            )],
        );
        let rows: Vec<&AddressRegion> = mhir
            .address_map
            .iter()
            .filter(|(n, _)| n.as_str() == "u_uart")
            .map(|(_, r)| r)
            .collect();
        assert_eq!(rows.len(), 1, "tidak ada entri ganda");
        assert_eq!(rows[0].base, 0x2000_0000);
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        let uart = top
            .devices
            .iter()
            .find(|d| d.name.as_str() == "u_uart")
            .unwrap();
        assert_eq!(uart.mmio.map(|r| r.base), Some(0x2000_0000));
        // IRQ anotasi tidak terpengaruh override region.
        assert_eq!(uart.irq, Some(5));
    }

    #[test]
    fn test_annotation_invalid_region_ignored() {
        // `mivon_region` tanpa base/size lengkap → region None (bukan 0 yang
        // menyesatkan); `mivon_irq` non-numerik → None; atribut lain bebas.
        let src = r#"
module uart (input logic clk, input logic rst_n, output logic tx);
  assign tx = 1'b0;
endmodule
module soc (input logic clk, input logic rst_n, output logic tx);
  (* mivon_region = "mmio", junk = "abc" *)
  (* mivon_irq = "zz" *)
  (* syn_preserve *)
  uart u_uart (.clk(clk), .rst_n(rst_n), .tx(tx));
endmodule
"#;
        let design = compile(src);
        let mhir = extract(&design);
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        let uart = top
            .devices
            .iter()
            .find(|d| d.name.as_str() == "u_uart")
            .expect("u_uart tetap jadi device meski anotasi rusak");
        assert_eq!(uart.mmio, None);
        assert_eq!(uart.irq, None);
        assert!(mhir.address_map.is_empty());
    }

    #[test]
    fn test_apply_address_map() {
        let design = compile(SOC);
        let mut mhir = extract(&design);
        let entries = vec![
            (
                Symbol::intern("u_uart"),
                AddressRegion {
                    base: 0x1000_0000,
                    size: 0x1000,
                },
            ),
            (
                Symbol::intern("u_cpu"),
                AddressRegion {
                    base: 0x0000_0000,
                    size: 0x1000,
                },
            ),
        ];
        apply_address_map(&mut mhir, &entries);
        assert_eq!(mhir.address_map.len(), 2);
        // Urut by base: u_cpu (0) dulu, lalu u_uart (0x10000000).
        assert_eq!(mhir.address_map[0].0.as_str(), "u_cpu");
        assert_eq!(mhir.address_map[1].0.as_str(), "u_uart");
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        let uart = top
            .devices
            .iter()
            .find(|d| d.name.as_str() == "u_uart")
            .unwrap();
        assert_eq!(
            uart.mmio,
            Some(AddressRegion {
                base: 0x1000_0000,
                size: 0x1000
            })
        );
    }

    #[test]
    fn test_address_map_match_module_name() {
        let design = compile(SOC);
        let mut mhir = extract(&design);
        // Cocok via nama MODULE (bukan instance): uart → semua instance uart.
        apply_address_map(
            &mut mhir,
            &[(
                Symbol::intern("uart"),
                AddressRegion {
                    base: 0x2000,
                    size: 0x100,
                },
            )],
        );
        let top = mhir
            .modules
            .iter()
            .find(|m| m.name.as_str() == "soc")
            .unwrap();
        let uart = top
            .devices
            .iter()
            .find(|d| d.name.as_str() == "u_uart")
            .unwrap();
        assert_eq!(
            uart.mmio,
            Some(AddressRegion {
                base: 0x2000,
                size: 0x100
            })
        );
    }
}
