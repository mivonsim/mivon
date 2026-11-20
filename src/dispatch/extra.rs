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
                println!("Batch summary: no active batch runs");
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_memcheck(a: &crate::cli::MmemcheckArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_tbgen(a: &crate::cli::MtbgenArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_verif_dashboard(a: &crate::cli::MdashboardArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_license(a: &crate::cli::MlicenseArgs) -> ! {
    let result: Result<(), SimError> = (|| {
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
                vault
                    .verify(std::path::Path::new(file))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Verified: {}", file);
            }
            crate::cli::MvaultCmd::List => {
                for entry in vault.list() {
                    println!("{} ({})", entry.path, entry.permissions.owner);
                }
            }
            crate::cli::MvaultCmd::Summary => {
                println!("Vault: {} files", vault.list().len());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_ipxact(a: &crate::cli::MipxactArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::ipxact::IpxactComponent;
        use std::path::Path;

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
                let ports: Vec<(String, String, Option<u32>)> = inputs
                    .as_deref()
                    .unwrap_or("")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        let mut parts = s.splitn(2, '=');
                        let name = parts.next().unwrap_or("").to_string();
                        let width = parts.next().and_then(|w| w.parse::<u32>().ok());
                        (
                            name,
                            width.map(|w| format!("{}", w)).unwrap_or_default(),
                            width.map(|w| w - 1),
                        )
                    })
                    .collect();
                let _ = outputs;
                let component = IpxactComponent::from_module(&module, &ports);
                let xml = component.to_xml();
                if let Some(path) = output {
                    component.save_xml(Path::new(&path)).map_err(|e| {
                        SimError::with_diag(DiagCode::IoError, e)
                    })?;
                    println!("{}", xml);
                } else {
                    println!("{}", xml);
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

pub(crate) fn dispatch_web_dashboard(a: &crate::cli::MwebArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::sim_dashboard::WebDashboard;
        use mivon_api::tools::sim_server::ServerConfig;
        let cfg = ServerConfig {
            max_total_sessions: a.max_sessions.max(1),
            ..Default::default()
        };
        let dash = WebDashboard::new(cfg);
        dash.server()
            .set_runner(std::sync::Arc::new(run_sim_session));
        if a.print_html {
            print!("{}", dash.render_html());
            return Ok(());
        }
        println!("Mivon simulation dashboard: http://{}/", a.addr);
        dash.serve(&a.addr)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, e))
    })();
    exit_tool(result);
}

/// Runner sesi dashboard: elaborasi + jalankan engine, kembalikan waktu
/// simulasi akhir (cycle). Error dikembalikan sebagai teks agar status sesi
/// menjadi `Failed` (bukan mematikan worker).
fn run_sim_session(files: &[String], max_time: u64) -> Result<u64, String> {
    use mivon_elaboration::elaborator::ElaborateMode;
    let (_session, _design, ir) =
        mivon_api::tools::open_elaborated(files, &[], &[], None, ElaborateMode::StrictSimulation)
            .map_err(|e| e.to_string())?;
    let mut engine = mivon_api::simulator::SimulationEngine::new(ir, max_time);
    engine.run().map_err(|e| e.to_string())?;
    Ok(engine.state.time)
}

pub(crate) fn dispatch_mdb(a: &crate::cli::MdbArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::design_db::DesignDatabase;
        use mivon_core::intern::Symbol;
        use std::path::Path;

        let cmd = a.command.as_deref().unwrap_or("");
        match cmd {
            "record" => {
                let top = a.top.as_deref().ok_or_else(|| {
                    SimError::with_diag(DiagCode::IoError, "--mdb-top required")
                })?;
                let entry = mivon_api::tools::design_db::DesignEntry::new(
                    Symbol::intern(top),
                    a.source_files.as_deref().unwrap_or("").split(',').map(|s| s.to_string()).collect(),
                    a.signal_count,
                    a.process_count,
                    a.checksum.clone().unwrap_or_default(),
                );
                let mut db_store = DesignDatabase::load(Path::new(&a.db))
                    .unwrap_or_else(|_| DesignDatabase::new());
                db_store.record(entry.clone());
                db_store.save(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("save db: {e}"))
                })?;
                println!(
                    "Recorded design '{}': {} signals, {} processes, checksum={}",
                    a.top.as_deref().unwrap_or(""), a.signal_count, a.process_count,
                    a.checksum.as_deref().unwrap_or("")
                );
            }
            "get" => {
                let top = a.top.as_deref().ok_or_else(|| {
                    SimError::with_diag(DiagCode::IoError, "--mdb-top required")
                })?;
                let db_store = DesignDatabase::load(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("load db: {e}"))
                })?;
                let got = db_store
                    .get(&Symbol::intern(top))
                    .ok_or_else(|| SimError::with_diag(DiagCode::IoError, format!("design '{}' not found", top)))?;
                println!(
                    "top={} signals={} processes={} checksum={} compiled_at={}",
                    got.top, got.signal_count, got.process_count, got.checksum, got.compiled_at
                );
            }
            "list" => {
                let db_store = DesignDatabase::load(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("load db: {e}"))
                })?;
                let all = db_store.get_all();
                if a.json {
                    let json_str =
                        serde_json::to_string_pretty(&all)
                            .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
                    println!("{json_str}");
                } else {
                    for (top, entry) in &all {
                        println!(
                            "{}: {} signals, {} processes",
                            top, entry.signal_count, entry.process_count
                        );
                    }
                }
            }
            "update" => {
                let top = a.top.as_deref().ok_or_else(|| {
                    SimError::with_diag(DiagCode::IoError, "--mdb-top required")
                })?;
                let entry = mivon_api::tools::design_db::DesignEntry::new(
                    Symbol::intern(top),
                    a.source_files.as_deref().unwrap_or("").split(',').map(|s| s.to_string()).collect(),
                    a.signal_count,
                    a.process_count,
                    a.checksum.clone().unwrap_or_default(),
                );
                let mut db_store = DesignDatabase::load(Path::new(&a.db))
                    .unwrap_or_else(|_| DesignDatabase::new());
                let changed = db_store.update(entry.clone());
                if changed {
                    db_store.save(Path::new(&a.db)).map_err(|e| {
                        SimError::with_diag(DiagCode::IoError, format!("save db: {e}"))
                    })?;
                    println!("Updated design '{}': checksum={}", a.top.as_deref().unwrap_or(""), a.checksum.as_deref().unwrap_or(""));
                } else {
                    println!("Design '{}' unchanged (checksum identical)", a.top.as_deref().unwrap_or(""));
                }
            }
            "remove" => {
                let top = a.top.as_deref().ok_or_else(|| {
                    SimError::with_diag(DiagCode::IoError, "--mdb-top required")
                })?;
                let mut db_store = DesignDatabase::load(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("load db: {e}"))
                })?;
                let removed = db_store.remove(&Symbol::intern(top));
                if removed {
                    db_store.save(Path::new(&a.db)).map_err(|e| {
                        SimError::with_diag(DiagCode::IoError, format!("save db: {e}"))
                    })?;
                    println!("Removed design '{}'", a.top.as_deref().unwrap_or(""));
                } else {
                    println!("Design '{}' not found", a.top.as_deref().unwrap_or(""));
                }
            }
            "clear" => {
                let mut db_store = DesignDatabase::load(Path::new(&a.db))
                    .unwrap_or_else(|_| DesignDatabase::new());
                db_store.clear();
                db_store.save(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("save db: {e}"))
                })?;
                println!("Cleared all design entries");
            }
            "save" => {
                let db_store = DesignDatabase::load(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("load db: {e}"))
                })?;
                db_store.save(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("save db: {e}"))
                })?;
                println!("Saved design database to {}", a.db);
            }
            "load" => {
                let path = a.path.clone().ok_or_else(|| {
                    SimError::with_diag(DiagCode::IoError, "--mdb-path required for load")
                })?;
                let loaded = DesignDatabase::load(Path::new(&path)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("load db: {e}"))
                })?;
                let all = loaded.get_all();
                println!("Loaded {} design entries from {}", all.len(), path);
                for (top, entry) in &all {
                    println!(
                        "  {}: {} signals, {} processes, checksum={}",
                        top, entry.signal_count, entry.process_count, entry.checksum
                    );
                }
            }
            "get-all" => {
                let db_store = DesignDatabase::load(Path::new(&a.db)).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("load db: {e}"))
                })?;
                let all = db_store.get_all();
                if a.json {
                    let json_str =
                        serde_json::to_string_pretty(&all)
                            .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
                    println!("{json_str}");
                } else {
                    for (top, entry) in &all {
                        println!(
                            "{}: {} signals, {} processes, checksum={}",
                            top, entry.signal_count, entry.process_count, entry.checksum
                        );
                    }
                }
            }
            _ => {
                return Err(SimError::with_diag(
                    DiagCode::IoError,
                    format!("unknown mdb subcommand: {}", cmd),
                ))
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

/// mfarm — Simulation farm orchestration (distributed job scheduler).
pub(crate) fn dispatch_mfarm(a: &crate::cli::MfarmArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::sim_farm::{FarmConfig, Priority, SimFarm};
        use mivon_core::error::SimError;

        let mut farm = SimFarm::new(FarmConfig::default());

        match &a.cmd {
            crate::cli::MfarmCmd::Submit {
                name,
                files,
                max_time,
                priority,
            } => {
                let priority = match priority.as_str() {
                    "critical" => Priority::Critical,
                    "high" => Priority::High,
                    "normal" => Priority::Normal,
                    "low" => Priority::Low,
                    "background" => Priority::Background,
                    _ => Priority::Normal,
                };
                let id = farm.submit(name, files.clone(), *max_time, priority);
                println!("Submitted job '{}' (id={})", name, id);
            }
            crate::cli::MfarmCmd::RegisterWorker {
                id,
                addr,
                capacity,
            } => {
                farm.register_worker(id, addr, *capacity);
                println!("Registered worker '{}' at {} (capacity={})", id, addr, capacity);
            }
            crate::cli::MfarmCmd::CompleteJob { id } => {
                if farm.complete_job(id) {
                    println!("Job '{}' completed", id);
                } else {
                    println!("Job '{}' not found", id);
                }
            }
            crate::cli::MfarmCmd::FailJob { id, reason } => {
                farm.fail_job(id, reason);
                println!("Job '{}' failed: {}", id, reason);
            }
            crate::cli::MfarmCmd::CancelJob { id } => {
                if farm.cancel_job(id) {
                    println!("Job '{}' cancelled", id);
                } else {
                    println!("Job '{}' not found or cannot cancel", id);
                }
            }
            crate::cli::MfarmCmd::RemoveJob { id } => {
                if farm.remove_job(id) {
                    println!("Job '{}' removed", id);
                } else {
                    println!("Job '{}' not found", id);
                }
            }
            crate::cli::MfarmCmd::RemoveWorker { id } => {
                if farm.remove_worker(id) {
                    println!("Worker '{}' removed", id);
                } else {
                    println!("Worker '{}' not found", id);
                }
            }
            crate::cli::MfarmCmd::ClearJobs => {
                farm.clear_jobs();
                println!("Cleared all jobs");
            }
            crate::cli::MfarmCmd::ClearWorkers => {
                farm.clear_workers();
                println!("Cleared all workers");
            }
            crate::cli::MfarmCmd::Summary => {
                println!("{}", farm.summary());
            }
            crate::cli::MfarmCmd::GetJob { id } => {
                if let Some(job) = farm.get_job(id) {
                    println!(
                        "Job '{}': status={:?}, files={:?}, max_time={}",
                        id, job.status, job.source_files, job.max_time
                    );
                } else {
                    println!("Job '{}' not found", id);
                }
            }
            crate::cli::MfarmCmd::GetWorker { id } => {
                if let Some(worker) = farm.get_worker(id) {
                    println!(
                        "Worker '{}': addr={}, status={:?}, capacity={}, current_load={}",
                        worker.id, worker.addr, worker.status, worker.capacity, worker.current_load
                    );
                } else {
                    println!("Worker '{}' not found", id);
                }
            }
            crate::cli::MfarmCmd::ListJobs { status } => {
                let status_filter = status.as_deref().map(|s| match s {
                    "pending" => mivon_api::tools::sim_farm::JobStatus::Pending,
                    "queued" => mivon_api::tools::sim_farm::JobStatus::Queued,
                    "running" => mivon_api::tools::sim_farm::JobStatus::Running,
                    "completed" => mivon_api::tools::sim_farm::JobStatus::Completed,
                    "failed" => mivon_api::tools::sim_farm::JobStatus::Failed(String::new()),
                    "cancelled" => mivon_api::tools::sim_farm::JobStatus::Cancelled,
                    "retry" => mivon_api::tools::sim_farm::JobStatus::Retry(String::new()),
                    _ => mivon_api::tools::sim_farm::JobStatus::Queued,
                });
                let jobs = farm.list_jobs(status_filter.as_ref());
                for job in jobs {
                    println!("Job '{}': status={:?}", job.id, job.status);
                }
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

/// mcdb — Collaborative debug session management.
pub(crate) fn dispatch_mcdb(a: &crate::cli::MdbArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::collab_debug::{CollabDebugManager, DebugSession};
        use std::path::Path;

        let cmd = a.command.as_deref().unwrap_or("");
        match cmd {
            "create" => {
                let session_id = a.top.as_deref().ok_or_else(|| {
                    SimError::with_diag(DiagCode::IoError, "--mcdb-session required")
                })?;
                let owner = a.source_files.as_deref().unwrap_or("").to_string();
                let mut manager = CollabDebugManager::load(Path::new(&a.db));
                if manager.create_session(session_id, &owner) {
                    manager.save(Path::new(&a.db)).map_err(|e| {
                        SimError::with_diag(DiagCode::IoError, format!("save db: {e}"))
                    })?;
                    println!("Created debug session '{}' (owner={})", session_id, owner);
                } else {
                    println!("Session '{}' already exists", session_id);
                }
            }
            _ => {
                return Err(SimError::with_diag(
                    DiagCode::IoError,
                    format!("unknown mcdb subcommand: {}", cmd),
                ))
            }
        }
        Ok(())
    })();
    exit_tool(result);
}
