//! Adaptor dispatch untuk tool tambahan (lihat tools.md).
//!
//! Tiap fungsi hanya menerjemahkan cli::*Args menjadi mivon_api::tools::*
//! lalu menyerahkan hasil ke super::exit_tool. Tidak ada logika tool di
//! sini — semua pekerjaan ada di crate mivon-tools.

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;

use super::exit_tool;
pub(crate) fn dispatch_batch(a: &crate::cli::MbatchArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        match &a.cmd {
            crate::cli::MbatchCmd::Run { config } => {
                use mivon_api::tools::batch::BatchConfig;
                let cfg = BatchConfig::from_file(std::path::Path::new(config))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let mut runner = cfg.to_runner();
                let results = runner.run();
                let summary = runner.summary();
                println!("{}", summary);
                for r in &results {
                    let status = match &r.status {
                        mivon_api::tools::batch::JobStatus::Completed => "✓ OK".to_string(),
                        mivon_api::tools::batch::JobStatus::Failed(e) => format!("✗ FAILED: {}", e),
                        mivon_api::tools::batch::JobStatus::Skipped => "⊘ SKIPPED".to_string(),
                        _ => "? PENDING".to_string(),
                    };
                    println!(
                        "  {}: {} ({:.2}s)",
                        r.name,
                        status,
                        r.duration.as_secs_f64()
                    );
                }
            }
            crate::cli::MbatchCmd::Status => {
                println!("Batch status: no active batch runs");
            }
            crate::cli::MbatchCmd::Summary => {
                println!("Batch summary: use 'mbatch run <config.toml>' to run");
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_memcheck(a: &crate::cli::MmemcheckArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        let binary = a.binary.clone();
        let args: Vec<String> = a.args.clone();
        let tool_result = match a.tool.as_str() {
            "valgrind" => mivon_api::tools::memcheck::run_valgrind(&binary, &args, &[])
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?,
            "heaptrack" => mivon_api::tools::memcheck::run_heaptrack(&binary, &args)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?,
            _ => {
                return Err(SimError::with_diag(
                    DiagCode::InvalidSyntax,
                    format!(
                        "tool '{}' tidak dikenal (gunakan 'valgrind' atau 'heaptrack')",
                        a.tool
                    ),
                ))
            }
        };
        println!("Tool: {}", tool_result.tool);
        println!("Exit code: {}", tool_result.exit_code);
        println!("Summary: {}", tool_result.summary);
        if tool_result.has_leaks() {
            println!("⚠ Memory leaks detected!");
        }
        if tool_result.has_errors() {
            println!("✗ {} errors found", tool_result.errors);
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_tbgen(a: &crate::cli::MtbgenArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::tbgen;
        // Konfigurasi CLI dipakai di KEDUA jalur. clock/reset None = biarkan
        // deteksi port yang menentukan (jalur --file) atau default (manual).
        let config = tbgen::TbConfig {
            clock_name: a.clock_name.clone().unwrap_or_default(),
            clock_period: a.clock_period,
            reset_name: a.reset_name.clone(),
            reset_active_high: a.reset_high,
            simulation_time: a.sim_time,
            dump_vcd: a.dump_vcd,
            include_checks: a.include_checks,
            ..tbgen::TbConfig::default()
        };
        let tb = if let Some(ref file) = a.file {
            // Jalur otomatis: port dibaca dari AST module.
            let (design, _session) =
                mivon_api::tools::open_project(&[file.clone()], &[], &[], a.module.as_deref())?;
            tbgen::generate_for_module(&design, a.module.as_deref(), &config)
                .map_err(|e| SimError::with_diag(DiagCode::ModuleNotFound, e))?
        } else {
            let inputs = parse_tb_ports(a.inputs.as_deref(), "--inputs")?;
            let outputs = parse_tb_ports(a.outputs.as_deref(), "--outputs")?;
            let in_refs: Vec<(&str, u32)> =
                inputs.iter().map(|(n, w)| (n.as_str(), *w)).collect();
            let out_refs: Vec<(&str, u32)> =
                outputs.iter().map(|(n, w)| (n.as_str(), *w)).collect();
            let ports = tbgen::ports_from_lists(&in_refs, &out_refs);
            let mut config = config.clone();
            if let Some(m) = &a.module {
                config.module_name = m.clone();
            } else {
                config.module_name = "dut".to_string();
            }
            if config.clock_name.is_empty() {
                config.clock_name = "clk".to_string();
            }
            if config.reset_name.is_none() {
                config.reset_name = Some("rst_n".to_string());
            }
            tbgen::generate_tb(&config, &ports)
        };
        if let Some(ref output) = a.output {
            std::fs::write(output, &tb).map_err(|e| {
                SimError::with_diag(DiagCode::IoError, format!("{}: {}", output, e))
            })?;
            println!("Testbench written to {}", output);
        } else {
            print!("{}", tb);
        }
        Ok(())
    })();
    exit_tool(result);
}

/// Parse `name=width,name=width` untuk --inputs/--outputs.
/// Nama kosong atau width bukan angka positif = error (sebelumnya
/// menghasilkan SV invalid `reg [7:0] ;` dengan exit 0).
fn parse_tb_ports(spec: Option<&str>, what: &str) -> Result<Vec<(String, u32)>, SimError> {
    let mut out = Vec::new();
    let Some(spec) = spec else {
        return Ok(out);
    };
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, width) = part.split_once('=').ok_or_else(|| {
            SimError::with_diag(
                DiagCode::InvalidSyntax,
                format!("{}: '{}' harus berbentuk nama=lebar", what, part),
            )
        })?;
        let name = name.trim();
        if name.is_empty() {
            return Err(SimError::with_diag(
                DiagCode::InvalidSyntax,
                format!("{}: nama port kosong pada '{}'", what, part),
            ));
        }
        let width: u32 = width.trim().parse().map_err(|_| {
            SimError::with_diag(
                DiagCode::InvalidSyntax,
                format!("{}: lebar '{}' bukan angka pada '{}'", what, width.trim(), part),
            )
        })?;
        if width == 0 {
            return Err(SimError::with_diag(
                DiagCode::InvalidSyntax,
                format!("{}: lebar 0 tidak valid pada '{}'", what, part),
            ));
        }
        out.push((name.to_string(), width));
    }
    Ok(out)
}

pub(crate) fn dispatch_verif_dashboard(a: &crate::cli::MdashboardArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::verif_dashboard::Dashboard;
        let coverage = a.coverage.as_ref().map(std::path::PathBuf::from);
        // Path yang diminta EKSPLISIT tetapi tidak ada = salah ketik, bukan
        // "tidak ada data" — jangan diamkan.
        if let Some(p) = &coverage {
            if !p.exists() {
                return Err(SimError::with_diag(
                    DiagCode::IoError,
                    format!("coverage file tidak ditemukan: {}", p.display()),
                ));
            }
        }
        let dash = Dashboard::collect(
            coverage.as_deref(),
            std::path::Path::new(&a.regression),
            std::path::Path::new(&a.bugs),
        )
        .map_err(|e| SimError::with_diag(DiagCode::DataFormatError, e))?;
        let content = if a.text {
            dash.render_text()
        } else {
            dash.render_html()
        };
        match &a.output {
            Some(out) => {
                write_report(out, &content)?;
                println!("Dashboard written to {}", out);
            }
            None => print!("{}", content),
        }
        Ok(())
    })();
    exit_tool(result);
}


/// Tulis laporan (HTML/teks) — membuat direktori induk bila perlu.
fn write_report(path: &str, content: &str) -> Result<(), SimError> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SimError::with_diag(
                    DiagCode::IoError,
                    format!("buat direktori {}: {}", parent.display(), e),
                )
            })?;
        }
    }
    std::fs::write(path, content)
        .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", path, e)))
}

pub(crate) fn dispatch_license(a: &crate::cli::MlicenseArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::license_server::{with_locked_db, LicenseServer};
        let path = std::path::Path::new(&a.db);
        let default_timeout = a.timeout.unwrap_or(300);
        let fmt_err = |e: String| SimError::with_diag(DiagCode::DataFormatError, e);
        match &a.cmd {
            // Status read-only: TIDAK membuat/menulis DB (sebelumnya `status`
            // di direktori kosong membuat licenses.json kosong).
            crate::cli::MlicenseCmd::Status => {
                if !path.exists() {
                    println!("LicenseServer: belum ada database lisensi ({})", a.db);
                    return Ok(());
                }
                let mut server = LicenseServer::load(path).map_err(fmt_err)?;
                server.refresh();
                println!("{}", server.summary());
                for l in server.active_leases() {
                    println!(
                        "  {} {}@{} feature={} expires_at={}",
                        l.id, l.user, l.host, l.feature, l.expires_at
                    );
                }
            }
            crate::cli::MlicenseCmd::Init { feature, count } => {
                if *count == 0 {
                    return Err(SimError::with_diag(
                        DiagCode::InvalidSyntax,
                        "--count harus > 0".to_string(),
                    ));
                }
                let feature = feature.clone();
                let count = *count;
                let timeout = a.timeout;
                let msg = with_locked_db(path, default_timeout, move |s| {
                    if let Some(t) = timeout {
                        s.set_heartbeat_timeout(t);
                    }
                    s.register_pool(&feature, count);
                    Ok(format!("Pool '{}' terdaftar: {} lisensi", feature, count))
                })
                .map_err(fmt_err)?;
                println!("{}", msg);
            }
            crate::cli::MlicenseCmd::Checkout {
                feature,
                user,
                host,
            } => {
                let (feature, user, host) = (feature.clone(), user.clone(), host.clone());
                let id = with_locked_db(path, default_timeout, move |s| s.checkout(&feature, &user, &host))
                    .map_err(|e| {
                        // Pool habis = resource limit; feature tak dikenal = referensi.
                        let code = if e.contains("unknown feature") {
                            DiagCode::InvalidReference
                        } else {
                            DiagCode::ResourceLimit
                        };
                        SimError::with_diag(code, e)
                    })?;
                println!("{}", id);
            }
            crate::cli::MlicenseCmd::Checkin { lease } => {
                let lease = lease.clone();
                let arg = lease.clone();
                with_locked_db(path, default_timeout, move |s| s.checkin(&arg)).map_err(|e| {
                    SimError::with_diag(DiagCode::InvalidReference, e)
                })?;
                println!("Lease {} dikembalikan", lease);
            }
            crate::cli::MlicenseCmd::Heartbeat { lease } => {
                let lease = lease.clone();
                let arg = lease.clone();
                with_locked_db(path, default_timeout, move |s| s.heartbeat(&arg)).map_err(
                    |e| SimError::with_diag(DiagCode::InvalidReference, e),
                )?;
                println!("Heartbeat lease {}", lease);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_waiver(a: &crate::cli::MwaiverArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::waiver::WaiverStore;
        let mut store = WaiverStore::new();
        let db_path = std::path::Path::new(".mivon/waivers.json");
        if db_path.exists() {
            store = WaiverStore::load(db_path)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
        }
        match &a.cmd {
            crate::cli::MwaiverCmd::Add {
                rule,
                file_pattern,
                reason,
                owner,
            } => {
                let id = store.add(rule, file_pattern.as_deref(), reason, owner);
                store
                    .save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Waiver {} added: {}", id, rule);
            }
            crate::cli::MwaiverCmd::List { rule, json } => {
                let waivers: Vec<_> = store
                    .waivers
                    .iter()
                    .filter(|w| rule.as_ref().map(|r| w.rule == *r).unwrap_or(true))
                    .collect();
                if *json {
                    let json_str = serde_json::to_string_pretty(&waivers)
                        .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
                    println!("{}", json_str);
                } else {
                    for w in &waivers {
                        println!("{}: {} ({})", w.id, w.rule, w.reason);
                    }
                }
            }
            crate::cli::MwaiverCmd::Check { rule, file } => {
                if let Some(m) = store.is_waived(rule, file.as_deref(), None) {
                    println!(
                        "✓ Waived: {} (confidence: {:.0}%)",
                        m.waiver.id,
                        m.confidence * 100.0
                    );
                } else {
                    println!("✗ Not waived: {}", rule);
                }
            }
            crate::cli::MwaiverCmd::Export { output } => {
                store
                    .save(std::path::Path::new(output))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Waivers exported to {}", output);
            }
            crate::cli::MwaiverCmd::Import { input } => {
                let imported = WaiverStore::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                for w in imported.waivers {
                    store.add(&w.rule, w.file_pattern.as_deref(), &w.reason, &w.owner);
                }
                store
                    .save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Waivers imported from {}", input);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_vault(a: &crate::cli::MvaultArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::vault::SecureVault;
        let vault = SecureVault::new();
        match &a.cmd {
            crate::cli::MvaultCmd::Register { file, user } => {
                let entry = vault
                    .register(std::path::Path::new(file), user)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!(
                    "Registered: {} (owner: {})",
                    entry.path, entry.permissions.owner
                );
            }
            crate::cli::MvaultCmd::Lock { file, user } => {
                vault
                    .lock(file, user)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Locked: {} by {}", file, user);
            }
            crate::cli::MvaultCmd::Unlock { file, user } => {
                vault
                    .unlock(file, user)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Unlocked: {} by {}", file, user);
            }
            crate::cli::MvaultCmd::Verify { file } => {
                let ok = vault
                    .verify(std::path::Path::new(file))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                if ok {
                    println!("✓ Integrity OK: {}", file);
                } else {
                    println!("✗ Integrity FAILED: {}", file);
                }
            }
            crate::cli::MvaultCmd::List => {
                let entries = vault.list();
                for e in &entries {
                    println!(
                        "{} (owner: {}, locked: {})",
                        e.path,
                        e.permissions.owner,
                        e.locked_by.as_deref().unwrap_or("no")
                    );
                }
            }
            crate::cli::MvaultCmd::Summary => {
                println!("{}", vault.summary());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_ipxact(a: &crate::cli::MipxactArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::ipxact::IpxactComponent;
        match &a.cmd {
            crate::cli::MipxactCmd::Generate {
                module,
                vendor,
                library,
                version,
                output,
                inputs,
                outputs,
            } => {
                let mut ports: Vec<(String, String, Option<u32>)> = Vec::new();
                if let Some(ref input_str) = inputs {
                    for part in input_str.split(',') {
                        if let Some((name, width)) = part.split_once('=') {
                            ports.push((
                                name.trim().to_string(),
                                "in".to_string(),
                                width.trim().parse().ok(),
                            ));
                        }
                    }
                }
                if let Some(ref output_str) = outputs {
                    for part in output_str.split(',') {
                        if let Some((name, width)) = part.split_once('=') {
                            ports.push((
                                name.trim().to_string(),
                                "out".to_string(),
                                width.trim().parse().ok(),
                            ));
                        }
                    }
                }
                let mut comp = IpxactComponent::from_module(module, &ports);
                comp.vendor = vendor.clone();
                comp.library = library.clone();
                comp.version = version.clone();
                let xml = comp.to_xml();
                if let Some(ref out) = output {
                    comp.save_xml(std::path::Path::new(out))
                        .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                    println!("IP-XACT written to {}", out);
                } else {
                    print!("{}", xml);
                }
            }
            crate::cli::MipxactCmd::Summary => {
                println!("IP-XACT: use 'mipxact generate' to create XML");
            }
        }
        Ok(())
    })();
    exit_tool(result);
}
