//! Adaptor dispatch untuk tool manajemen desain & kualitas.
//!
//! Mencakup: design_repo, project (workspace), sdc, equiv_check,
//! regression, eco, cov_closure. Tiap fungsi hanya menerjemahkan
//! cli::*Args menjadi mivon_api::tools::* lalu exit_tool().

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;

use super::exit_tool;
pub(crate) fn dispatch_design_repo(a: &crate::cli::MdesignRepoArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::design_repo::{DesignFileInfo, DesignRepository};
        match &a.cmd {
            crate::cli::MdesignRepoCmd::Init { root } => {
                let _repo = DesignRepository::open(std::path::PathBuf::from(root));
                println!("Design repository initialized at {}", root);
            }
            crate::cli::MdesignRepoCmd::Commit {
                author,
                message,
                files,
            } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                let file_infos: Vec<DesignFileInfo> = files
                    .iter()
                    .map(|f| DesignFileInfo {
                        path: f.clone(),
                        checksum: "auto".to_string(),
                        size: 0,
                    })
                    .collect();
                let commit = repo.commit(author, message, file_infos);
                println!("Committed: {} by {}", commit.hash, commit.author);
            }
            crate::cli::MdesignRepoCmd::Log { max } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                let commits = repo.log(*max);
                for c in &commits {
                    println!("{}: {} ({})", &c.hash[..12], c.message, c.author);
                }
            }
            crate::cli::MdesignRepoCmd::Tag {
                name,
                commit,
                description,
            } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                let tag = repo
                    .tag(name, commit, description)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Tagged: {} -> {}", tag.name, tag.commit_hash);
            }
            crate::cli::MdesignRepoCmd::Diff { a, b } => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                if let Some(diffs) = repo.diff(a, b) {
                    for d in &diffs {
                        println!("{}", d);
                    }
                } else {
                    println!("Commits not found");
                }
            }
            crate::cli::MdesignRepoCmd::Summary => {
                let repo = DesignRepository::open(std::path::PathBuf::from("."));
                println!("{}", repo.summary());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_project(a: &crate::cli::MprojectArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::project::{ProjectEntry, WorkspaceConfig};
        let config_path = std::path::Path::new(".mivon/workspace.toml");
        let mut config = if config_path.exists() {
            WorkspaceConfig::load(config_path)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
        } else {
            WorkspaceConfig::default_config()
        };
        match &a.cmd {
            crate::cli::MprojectCmd::Init { root } => {
                let _ = std::fs::create_dir_all(format!("{}/.mivon", root));
                config
                    .save(config_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Workspace initialized at {}", root);
            }
            crate::cli::MprojectCmd::Add {
                name,
                path,
                top,
                depends,
            } => {
                let deps: Vec<String> = depends
                    .as_ref()
                    .map(|d| d.split(',').map(|s| s.trim().to_string()).collect())
                    .unwrap_or_default();
                config.add_project(ProjectEntry {
                    name: name.clone(),
                    path: path.clone(),
                    top: top.clone(),
                    depends: deps,
                    incdirs: Vec::new(),
                    defines: Vec::new(),
                    features: Vec::new(),
                });
                config
                    .save(config_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Project '{}' added", name);
            }
            crate::cli::MprojectCmd::Remove { name } => {
                if config.remove_project(name) {
                    config
                        .save(config_path)
                        .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                    println!("Project '{}' removed", name);
                } else {
                    println!("Project '{}' not found", name);
                }
            }
            crate::cli::MprojectCmd::List => {
                for p in &config.projects {
                    println!(
                        "{}: {} (top: {})",
                        p.name,
                        p.path,
                        p.top.as_deref().unwrap_or("-")
                    );
                }
            }
            crate::cli::MprojectCmd::Analyze => {
                let analysis = config.analyze(std::path::Path::new("."));
                println!("Dependency order: {:?}", analysis.dependency_order);
                if !analysis.errors.is_empty() {
                    println!("Errors: {:?}", analysis.errors);
                }
            }
            crate::cli::MprojectCmd::Summary => {
                println!("Workspace: {} projects", config.projects.len());
                for p in &config.projects {
                    println!("  {} -> {}", p.name, p.path);
                }
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_sdc(a: &crate::cli::MsdcArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::sdc::SdcDocument;
        let doc = SdcDocument::load(std::path::Path::new(&a.file))
            .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
        if a.clocks_only {
            for c in doc.clocks() {
                println!("{:?}", c);
            }
        } else if a.json {
            let json = serde_json::to_string_pretty(&doc)
                .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
            println!("{}", json);
        } else {
            println!("{}", doc.summary());
            for c in &doc.constraints {
                println!("  {:?}", c);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_equiv_check(a: &crate::cli::MequivCheckArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::equiv_check::EquivChecker;
        let checker = EquivChecker::new(&a.method);
        // Load golden and impl from JSON files
        let golden_content = std::fs::read_to_string(&a.golden)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, format!("{}: {}", a.golden, e)))?;
        let impl_content = std::fs::read_to_string(&a.impl_file).map_err(|e| {
            SimError::with_diag(DiagCode::IoError, format!("{}: {}", a.impl_file, e))
        })?;
        let golden: Vec<(String, Vec<u64>)> = serde_json::from_str(&golden_content)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
        let impl_vals: Vec<(String, Vec<u64>)> = serde_json::from_str(&impl_content)
            .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
        let mapping = Vec::new();
        let result = checker.check_combinational(&mapping, &golden, &impl_vals);
        if result.equivalent {
            println!(
                "✓ EQUIVALENT (method: {}, time: {}ms)",
                result.method, result.proof_time_ms
            );
        } else {
            println!(
                "✗ NOT EQUIVALENT (method: {}, time: {}ms)",
                result.method, result.proof_time_ms
            );
            if let Some(ce) = &result.counter_example {
                println!("  Counter-example at cycle {}", ce.cycle);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_regression(a: &crate::cli::MregressionArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::regression::{RegressionDb, RegressionRun, TestResult};
        use std::time::{SystemTime, UNIX_EPOCH};
        let db_path = std::path::Path::new(".mivon/regression.json");
        let mut db = if db_path.exists() {
            RegressionDb::load(db_path).map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
        } else {
            RegressionDb::new()
        };
        match &a.cmd {
            crate::cli::MregressionCmd::Record {
                input,
                branch,
                commit,
            } => {
                let content = std::fs::read_to_string(input).map_err(|e| {
                    SimError::with_diag(DiagCode::IoError, format!("{}: {}", input, e))
                })?;
                let results: Vec<TestResult> = serde_json::from_str(&content)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e.to_string()))?;
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let run = RegressionRun {
                    id: format!("run-{}", ts),
                    timestamp: ts,
                    branch: branch.clone(),
                    commit: commit.clone(),
                    results,
                    total_duration_ms: 0,
                };
                db.record(run);
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Regression run recorded");
            }
            crate::cli::MregressionCmd::Summary => {
                println!("{}", db.summary());
            }
            crate::cli::MregressionCmd::Flaky => {
                let flaky = db.flaky_tests();
                if flaky.is_empty() {
                    println!("No flaky tests detected");
                } else {
                    for (name, rate) in &flaky {
                        println!("  {}: {:.0}% pass rate", name, rate * 100.0);
                    }
                }
            }
            crate::cli::MregressionCmd::Trend => {
                let trend = db.trend();
                println!("Trend: {:?}", trend);
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_eco(a: &crate::cli::MecoArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::eco::{EcoDb, EcoSeverity, EcoStatus};
        let db_path = std::path::Path::new(".mivon/eco.json");
        let mut db = if db_path.exists() {
            EcoDb::load(db_path).map_err(|e| SimError::with_diag(DiagCode::IoError, e))?
        } else {
            EcoDb::new()
        };
        match &a.cmd {
            crate::cli::MecoCmd::Create {
                title,
                description,
                severity,
                author,
            } => {
                let sev = match severity.as_str() {
                    "critical" => EcoSeverity::Critical,
                    "major" => EcoSeverity::Major,
                    "minor" => EcoSeverity::Minor,
                    "cosmetic" => EcoSeverity::Cosmetic,
                    _ => {
                        return Err(SimError::with_diag(
                            DiagCode::InvalidSyntax,
                            format!(
                                "severity '{}' tidak dikenal (critical/major/minor/cosmetic)",
                                severity
                            ),
                        ))
                    }
                };
                let id = db.create(title, description, sev, author);
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("ECO created: {}", id);
            }
            crate::cli::MecoCmd::List { status, severity } => {
                for e in &db.entries {
                    if let Some(s) = status {
                        if format!("{:?}", e.status).to_lowercase() != s.to_lowercase() {
                            continue;
                        }
                    }
                    if let Some(s) = severity {
                        if format!("{:?}", e.severity).to_lowercase() != s.to_lowercase() {
                            continue;
                        }
                    }
                    println!("{}: {} [{:?}] ({:?})", e.id, e.title, e.severity, e.status);
                }
            }
            crate::cli::MecoCmd::Transition { id, new_status } => {
                let status = match new_status.as_str() {
                    "draft" => EcoStatus::Draft,
                    "submitted" => EcoStatus::Submitted,
                    "reviewed" => EcoStatus::Reviewed,
                    "approved" => EcoStatus::Approved,
                    "implemented" => EcoStatus::Implemented,
                    "verified" => EcoStatus::Verified,
                    "closed" => EcoStatus::Closed,
                    "rejected" => EcoStatus::Rejected,
                    _ => {
                        return Err(SimError::with_diag(
                            DiagCode::InvalidSyntax,
                            format!("status '{}' tidak dikenal", new_status),
                        ))
                    }
                };
                db.transition(id, status)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("ECO {} transitioned to {}", id, new_status);
            }
            crate::cli::MecoCmd::Comment { id, author, text } => {
                db.comment(id, author, text)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                db.save(db_path)
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("Comment added to {}", id);
            }
            crate::cli::MecoCmd::Summary => {
                println!("{}", db.summary());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_cov_closure(a: &crate::cli::McovClosureArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::cov_closure::CoverageClosure;
        match &a.cmd {
            crate::cli::McovClosureCmd::Analyze { input } => {
                let cc = CoverageClosure::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                println!("{}", cc.summary());
                println!("\nCoverage by type:");
                for (t, pct) in cc.coverage_by_type() {
                    println!("  {}: {:.1}%", t, pct);
                }
            }
            crate::cli::McovClosureCmd::Critical { input } => {
                let cc = CoverageClosure::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let critical = cc.critical_tests();
                if critical.is_empty() {
                    println!("No critical tests");
                } else {
                    for tc in &critical {
                        println!("  {} (unique: {})", tc.test_name, tc.unique_points.len());
                    }
                }
            }
            crate::cli::McovClosureCmd::Uncovered { input } => {
                let cc = CoverageClosure::load(std::path::Path::new(input))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let uncovered = cc.find_uncovered();
                if uncovered.is_empty() {
                    println!("All points covered!");
                } else {
                    for p in &uncovered {
                        println!("  {} ({}:{})", p.id, p.file, p.line);
                    }
                }
            }
        }
        Ok(())
    })();
    exit_tool(result);
}

pub(crate) fn dispatch_requirements(a: &crate::cli::MrequirementsArgs) -> ! {
    let result: Result<(), SimError> = (|| {
        use mivon_api::tools::requirements::{load_mappings, load_results, RequirementsReport};
        use std::path::Path;
        match &a.cmd {
            crate::cli::MrequirementsCmd::Report {
                mappings,
                results,
                format,
                output,
            } => {
                let maps = load_mappings(Path::new(mappings))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let res = load_results(Path::new(results))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let report = RequirementsReport::generate(&maps, &res);
                let text = match format.as_str() {
                    "json" => report.to_json(),
                    "csv" => report.to_csv(),
                    other => {
                        return Err(SimError::with_diag(
                            DiagCode::InvalidSyntax,
                            format!("format \"{}\" tidak dikenal (csv|json)", other),
                        ))
                    }
                };
                match output {
                    Some(path) => std::fs::write(path, &text).map_err(|e| {
                        SimError::with_diag(DiagCode::IoError, format!("{}: {}", path, e))
                    })?,
                    None => print!("{}", text),
                }
            }
            crate::cli::MrequirementsCmd::Summary { mappings, results } => {
                let maps = load_mappings(Path::new(mappings))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let res = load_results(Path::new(results))
                    .map_err(|e| SimError::with_diag(DiagCode::IoError, e))?;
                let report = RequirementsReport::generate(&maps, &res);
                println!("{}", report.summary());
            }
        }
        Ok(())
    })();
    exit_tool(result);
}
