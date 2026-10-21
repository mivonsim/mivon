//! Adaptor dispatch subcommand `emu` (EMULATOR.md) + helper snapshot.
//!
//! Jalur: boot-iso x86, MHIR/address map, ELF loader, Direct RTL CPU,
//! interpreter RV32, hex dump memori, dan snapshot mesin (R5).
//! Dipisah dari main.rs agar entrypoint tetap tipis (pola anim_glue).

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;

use super::exit_tool;

pub(crate) fn dispatch_emu(a: &crate::cli::EmuArgs) -> ! {
    use mivon_api::emu::config::EmuConfig;
    use mivon_api::emu::cpu::{CpuCore, RtlLinkedCpu};
    use mivon_api::emu::machine::Machine;
    use mivon_api::emu::mem::{MemoryMap, MemoryPort, RamRegion, RegionKind};
    use mivon_api::emu::mhir::types::AddressRegion;
    use mivon_core::intern::Symbol;
    use mivon_elaboration::elaborator::ElaborateMode;

    let result: Result<(), SimError> = (|| {
        // ── Konfigurasi emulator dari file TOML terpisah (`--config`, .meu) ──
        // BUKAN section di project .mivon — ekstensi .mivon dipakai MICD
        // dan file list; konfigurasi emulator hidup di file sendiri.
        let cfg: EmuConfig = match &a.config {
            Some(path) => EmuConfig::load_file(std::path::Path::new(path))
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?,
            None => EmuConfig::default(),
        };

        // ── Memory map (R1): config ram → region RAM host (mmap) ──
        // Dibuat lebih awal agar bisa dipakai jalur --boot-iso (x86) juga.
        let mut memmap = MemoryMap::new();
        if let Some(ram) = cfg.ram {
            let (base, size) = (ram.base, ram.size);
            let region = RamRegion::new(Symbol::intern("ram"), base, size, RegionKind::Ram, true)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
            memmap
                .add(region)
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
        }

        // ── Boot ISO x86 real-mode (R6): interpreter x86 + ISO sebagai disk.
        // Tanpa RTL — jalur boot BIOS: MBR → INT 13h → El Torito → GRUB. ──
        if let Some(iso_path) = &a.boot_iso {
            let mut out = String::new();
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--boot-iso butuh region RAM — definisikan ram = { base, size } di config .meu (mis. 0x0:0x100000)",
                ));
            }
            let bytes = {
                let mut f = std::fs::File::open(iso_path).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("{}: {}", iso_path, e))
                })?;
                // Jalur boot CD yang benar: El Torito boot catalog → boot image
                // (cdboot/GRUB) — BUKAN MBR. Booting MBR hybrid = jalur USB/HDD
                // yang berujung salah unit LBA saat GRUB baca filesystem CD.
                let eltorito = mivon_api::emu::iso::parse_eltorito(&mut f)
                    .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                if eltorito.entry.media_type != 0 {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!(
                            "boot image ISO media type {} bukan no-emulation (0)",
                            eltorito.entry.media_type
                        ),
                    ));
                }
                // no-emul: BIOS muat boot image (cdboot, ~512-2048 byte).
                mivon_api::emu::iso::read_boot_image(&mut f, &eltorito.entry, 0x10000)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
            };
            if bytes.len() < 512 {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "ISO terlalu kecil (min 512 byte)",
                ));
            }
            // Boot image CD (El Torito no-emul) ke 0x7c00, DL = 0xE0
            let mut cpu = mivon_api::emu::cpu::x86::X86Cpu::new();
            cpu.disk = Some(Box::new(
                mivon_api::emu::cpu::x86::FileDisk::open(iso_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?,
            ));
            cpu.load_boot_image(&mut memmap, &bytes, 0xE0)
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e.reason))?;
            let max_steps = a.max_steps.unwrap_or(50_000);
            let mut machine = Machine::new(Box::new(cpu), memmap, max_steps);
            emu_snapshot_preflight(a, &machine)?;
            if let Some(line) = emu_snapshot_load(a, &mut machine)? {
                out.push_str(&line);
            }
            let result = match machine.run() {
                Ok(result) => {
                    out.push_str(&format!("\n{}", result.summary()));
                    out.push_str(&format!(
                        "\nBIOS console: {}\n",
                        String::from_utf8_lossy(&result.console)
                    ));
                    result
                }
                Err(e) => {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("x86 fault: {:?}", e),
                    ))
                }
            };
            if let Some(line) = emu_snapshot_save(a, &machine)? {
                out.push_str(&line);
            }
            // ── Window display (opsional) ──
            if let Some(ref win_arg) = a.window {
                let cfg = mivon_api::emu::display::DisplayConfig::from_wh(win_arg)
                    .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                let mut disp = mivon_api::emu::display::VgaDisplay::new(cfg);
                disp.open_window()
                    .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                // Render console output ke VGA display
                let console_str = String::from_utf8_lossy(&result.console);
                let mut col = 0;
                let mut row = 0;
                for ch in console_str.bytes() {
                    match ch {
                        b'\n' => {
                            col = 0;
                            row += 1;
                            if row >= 25 {
                                disp.scroll_up();
                                row = 24;
                            }
                        }
                        b'\r' => col = 0,
                        b'\t' => col = (col + 8) & !7,
                        _ => {
                            if col < 80 && row < 25 {
                                disp.write_char(col, row, ch, 0x0A);
                                col += 1;
                            }
                        }
                    }
                }
                disp.update();
                // Tunggu user tekan key atau tutup jendela
                eprintln!("[Mivon] Window aktif. Tekan ESC atau tutup jendela untuk keluar.");
                loop {
                    if let Some(ch) = disp.poll_input() {
                        if ch == '\x1b' {
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                return Ok(());
            }
            print!("{}", out);
            return Ok(());
        }

        // Top untuk open_elaborated (targets): --top > config > --rtl-cpu-top
        // (saat mesin dijalankan dari RTL, top SoC sudah jelas dari flag CPU).
        let top = a
            .top
            .as_deref()
            .or(cfg.top.as_deref())
            .or(a.rtl_cpu_top.as_deref());
        // MHIR butuh RTL target (.sv/.v). Jalur interpreter (`--run` +
        // `--load-elf`) jalan TANPA file source → elaborasi dilewati bila
        // tak ada target (dump/`--addr` tetap butuh target → error jelas).
        let mhir = if a.targets.is_empty() {
            if a.dump_mhir || a.dump_memory_map || !a.addr.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--dump-mhir/--dump-memory-map/--addr butuh target RTL (.sv) — berikan \
                     file source (jalur interpreter cukup --config + --load-elf + --run)",
                ));
            }
            if !a.run && a.load_elf.is_none() && a.dump_memory.is_none() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "tidak ada yang dijalankan — berikan target RTL (.sv), atau \
                     --run --load-elf <elf> (interpreter RV32), atau --boot-iso <iso>",
                ));
            }
            None
        } else {
            let (_session, _design, ir) = mivon_api::tools::open_elaborated(
                &a.targets,
                &a.incdirs,
                &a.defines,
                top,
                ElaborateMode::StrictSimulation,
            )?;
            let mut m = mivon_api::emu::mhir::extract(&ir);

            // ── Address map: --addr + [emu] devices (Direct RTL Device) ──
            let mut entries: Vec<(Symbol, AddressRegion)> = Vec::new();
            for s in &a.addr {
                let Some((name, rest)) = s.split_once('=') else {
                    eprintln!("warning: --addr '{}': format NAME=BASE:SIZE", s);
                    continue;
                };
                let Some((base_s, size_s)) = rest.split_once(':') else {
                    eprintln!("warning: --addr '{}': format NAME=BASE:SIZE", s);
                    continue;
                };
                let parse_hex = |v: &str| u64::from_str_radix(v.trim_start_matches("0x"), 16);
                match (parse_hex(base_s), parse_hex(size_s)) {
                    (Ok(base), Ok(size)) => {
                        entries.push((Symbol::intern(name.trim()), AddressRegion { base, size }));
                    }
                    _ => eprintln!("warning: --addr '{}': base/size harus hex (0x...)", s),
                }
            }
            for d in &cfg.devices {
                if let (Some(base), Some(size)) = (d.mmio, d.size) {
                    let name = d.name.as_deref().unwrap_or("device");
                    entries.push((Symbol::intern(name), AddressRegion { base, size }));
                }
            }
            mivon_api::emu::mhir::extract::apply_address_map(&mut m, &entries);
            Some(m)
        };

        let mut out = String::new();
        if let Some(mhir) = &mhir {
            if a.dump_memory_map {
                out.push_str(&mivon_api::emu::dump::dump_memory_map(mhir));
                if !memmap.regions.is_empty() {
                    out.push_str("\nMemory regions (host):\n");
                    for r in &memmap.regions {
                        out.push_str(&format!(
                            "  0x{:08x}-0x{:08x}  {:<12} {} ({})\n",
                            r.base,
                            r.base + r.size - 1,
                            r.name.as_str(),
                            match r.kind {
                                RegionKind::Ram => "ram",
                                RegionKind::Rom => "rom",
                                RegionKind::Mmio => "mmio",
                            },
                            r.size
                        ));
                    }
                }
            }
            // Tanpa flag → default MHIR; `--dump-memory-map` saja → map saja.
            if a.dump_mhir || !a.dump_memory_map {
                out.push_str(&mivon_api::emu::dump::dump_mhir(mhir));
            }
        }

        // ── ELF loader (R1): muat kernel/bare-metal ke memory map ──
        // `elf_entry`/`elf_class` dipakai jalur `--run` interpreter (tanpa
        // `--rtl-cpu`): PC mulai dari entry, ISA mengikuti kelas ELF.
        let mut elf_entry: Option<u64> = None;
        let mut elf_class: Option<u8> = None;
        if let Some(path) = &a.load_elf {
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--load-elf butuh region RAM — definisikan [emu] ram = { base, size } di project file .mivon",
                ));
            }
            let bytes = std::fs::read(path)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", path, e)))?;
            let (hdr, _) = mivon_api::emu::elf::parse_elf(&bytes).map_err(|e| {
                SimError::with_diag(DiagCode::InvalidSyntax, format!("ELF '{}': {}", path, e))
            })?;
            let entry = mivon_api::emu::elf::load_elf(&bytes, &mut memmap).map_err(|e| {
                SimError::with_diag(DiagCode::InvalidSyntax, format!("ELF '{}': {}", path, e))
            })?;
            elf_entry = Some(entry);
            elf_class = Some(hdr.class);
            out.push_str(&format!(
                "\nELF loaded: '{}' entry=0x{:x} ({} region(s))\n",
                path,
                entry,
                memmap.regions.len()
            ));
        }

        // ── Direct RTL CPU (EMULATOR.md §7.2 mode 3): jalankan mesin dari
        // RTL .sv/.v user, BUKAN model software Rust. Rust hanya menyediakan
        // memori + orkestrasi bus; register file/ALU/control dieksekusi engine
        // RTL mivon (picorv32-style kontrak bus). ──

        let mem_final: MemoryMap = if !a.rtl_cpu.is_empty() {
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--rtl-cpu butuh region RAM — definisikan [emu] ram = { base, size } di file .meu",
                ));
            }
            let top = a.rtl_cpu_top.as_deref().unwrap_or("rv32_bus_wrapper");
            let mut cpu = RtlLinkedCpu::from_files(&a.rtl_cpu, top)
                .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
            cpu.reset();
            let max_steps = a.max_steps.unwrap_or(10_000);
            let mut machine = Machine::new(Box::new(cpu), memmap, max_steps);
            emu_snapshot_preflight(a, &machine)?;
            if let Some(line) = emu_snapshot_load(a, &mut machine)? {
                out.push_str(&line);
            }
            match machine.run() {
                Ok(result) => {
                    // ── Window display (opsional) ──
                    if let Some(ref win_arg) = a.window {
                        let cfg = mivon_api::emu::display::DisplayConfig::from_wh(win_arg)
                            .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                        let mut disp = mivon_api::emu::display::VgaDisplay::new(cfg);
                        disp.open_window()
                            .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
                        // Render summary + console ke VGA display
                        let summary = result.summary();
                        let console_str = String::from_utf8_lossy(&result.console);
                        let full = format!("{}\n\n{}", summary, console_str);
                        let mut col = 0;
                        let mut row = 0;
                        for ch in full.bytes() {
                            match ch {
                                b'\n' => {
                                    col = 0;
                                    row += 1;
                                    if row >= 25 {
                                        disp.scroll_up();
                                        row = 24;
                                    }
                                }
                                b'\r' => col = 0,
                                b'\t' => col = (col + 8) & !7,
                                _ => {
                                    if col < 80 && row < 25 {
                                        disp.write_char(col, row, ch, 0x0A);
                                        col += 1;
                                    }
                                }
                            }
                        }
                        disp.update();
                        eprintln!(
                            "[Mivon] Window aktif. Tekan ESC atau tutup jendela untuk keluar."
                        );
                        loop {
                            if let Some(ch) = disp.poll_input() {
                                if ch == '\x1b' {
                                    break;
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        print!("{}", out);
                        return Ok(());
                    }
                    out.push_str(&format!("\n{}", result.summary()));
                }
                Err(e) => {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("RTL CPU fault: {:?}", e),
                    ))
                }
            }
            if let Some(line) = emu_snapshot_save(a, &machine)? {
                out.push_str(&line);
            }
            machine.mem
        } else if a.run {
            // ── Interpreter RISC-V32 (R1/R2): ELF jalan tanpa RTL ──
            // `--run` tanpa `--rtl-cpu` = CPU software (benar dulu untuk
            // bring-up/verifikasi — EMULATOR.md §7.2 mode Interpreter).
            let Some(entry) = elf_entry else {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--run tanpa --rtl-cpu butuh --load-elf <PATH> (interpreter RISC-V32) \
                     — atau jalankan CPU dari RTL dengan --rtl-cpu <file.sv>",
                ));
            };
            if elf_class == Some(2) {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "ELF64 (RV64): interpreter RV64 belum tersedia (fase R2) — pakai \
                     --rtl-cpu (Direct RTL CPU) atau tunggu JIT (R3)",
                ));
            }
            if memmap.regions.is_empty() {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--run butuh region RAM — definisikan ram = { base, size } di config .meu",
                ));
            }
            let mut cpu = mivon_api::emu::cpu::Rv32Cpu::new();
            cpu.set_pc(entry);
            let max_steps = a.max_steps.unwrap_or(1_000_000);
            let mut machine = Machine::new(Box::new(cpu), memmap, max_steps);
            emu_snapshot_preflight(a, &machine)?;
            if let Some(line) = emu_snapshot_load(a, &mut machine)? {
                out.push_str(&line);
            }
            match machine.run() {
                Ok(result) => out.push_str(&format!("\n{}", result.summary())),
                Err(e) => {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        format!("interpreter fault: {}", e),
                    ))
                }
            }
            if let Some(line) = emu_snapshot_save(a, &machine)? {
                out.push_str(&line);
            }
            machine.mem
        } else {
            memmap
        };

        // ── Hex dump memori guest ──
        if let Some(dm) = &a.dump_memory {
            let Some((addr_s, len_s)) = dm.split_once(':') else {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--dump-memory format ADDR:LEN (hex)",
                ));
            };
            let parse_hex = |v: &str| u64::from_str_radix(v.trim_start_matches("0x"), 16);
            let (Ok(addr), Some(len)) = (
                parse_hex(addr_s),
                parse_hex(len_s).ok().and_then(|l| usize::try_from(l).ok()),
            ) else {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    "--dump-memory: ADDR dan LEN harus hex",
                ));
            };
            out.push_str(&format!("\nMemory @0x{:x} ({} bytes):\n", addr, len));
            let mut cur = addr;
            let mut remain = len;
            while remain > 0 {
                let n = remain.min(16);
                let mut line = format!("0x{:08x}: ", cur);
                let mut hexs: Vec<String> = Vec::new();
                let mut ascii = String::new();
                for i in 0..n {
                    match mem_final.read(cur + i as u64, 1) {
                        Ok(b) => {
                            hexs.push(format!("{:02x}", b));
                            ascii.push(if (32..=126).contains(&(b as u8)) {
                                b as u8 as char
                            } else {
                                '.'
                            });
                        }
                        Err(_) => {
                            hexs.push("??".into());
                            ascii.push('?');
                        }
                    }
                }
                line.push_str(&hexs.join(" "));
                line.push_str(&format!("  {}", ascii));
                out.push_str(&line);
                out.push('\n');
                cur += n as u64;
                remain -= n;
            }
        }

        print!("{}", out);
        Ok(())
    })();
    exit_tool(result);
}

// ── Snapshot mesin (EMULATOR.md §14, fase R5) — helper CLI `mivon emu` ──

/// `--snapshot-save`: pastikan CPU mendukung snapshot SEBELUM run — gagal
/// cepat, jangan buang waktu run panjang lalu error di akhir.
fn emu_snapshot_preflight(
    a: &crate::cli::EmuArgs,
    machine: &mivon_api::emu::machine::Machine,
) -> Result<(), SimError> {
    if a.snapshot_save.is_some() {
        machine.cpu.snapshot().map_err(|e| {
            SimError::with_diag(DiagCode::InvalidSyntax, format!("--snapshot-save: {}", e))
        })?;
    }
    Ok(())
}

/// `--snapshot-load`: muat file + restore SEBELUM run. Kembalikan baris
/// output bila flag dipakai.
fn emu_snapshot_load(
    a: &crate::cli::EmuArgs,
    machine: &mut mivon_api::emu::machine::Machine,
) -> Result<Option<String>, SimError> {
    let Some(path) = a.snapshot_load.as_deref() else {
        return Ok(None);
    };
    let snap = mivon_api::emu::snapshot::MachineSnapshot::load_file(std::path::Path::new(path))
        .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
    machine.restore(&snap).map_err(|e| {
        SimError::with_diag(DiagCode::InvalidSyntax, format!("--snapshot-load: {}", e))
    })?;
    Ok(Some(format!(
        "\nsnapshot loaded: '{}' ({} instr / {} cycle kumulatif)\n",
        path, snap.steps, snap.cycles
    )))
}

/// `--snapshot-save`: simpan SETELAH run (CPU + memori sparse + counter).
fn emu_snapshot_save(
    a: &crate::cli::EmuArgs,
    machine: &mivon_api::emu::machine::Machine,
) -> Result<Option<String>, SimError> {
    let Some(path) = a.snapshot_save.as_deref() else {
        return Ok(None);
    };
    let snap = machine.snapshot().map_err(|e| {
        SimError::with_diag(DiagCode::InvalidSyntax, format!("--snapshot-save: {}", e))
    })?;
    let nbytes = snap.to_bytes().len();
    snap.save_file(std::path::Path::new(path))
        .map_err(|e| SimError::with_diag(DiagCode::InvalidSyntax, e))?;
    Ok(Some(format!(
        "\nsnapshot saved: '{}' ({} byte sparse, {} instr / {} cycle kumulatif)\n",
        path, nbytes, snap.steps, snap.cycles
    )))
}
