//! Adaptor dispatch tool inti (lihat tools.md).
//!
//! inspect, lint, elab, sim, cov, wave, fmt, gen, prof, check, bench,
//! synth, update. Tiap fungsi hanya menerjemahkan cli::*Args menjadi
//! mivon_api::tools::* lalu exit_tool().

use crate::DEFAULT_MAX_TIME_NS;

use super::exit_tool;
pub(crate) fn dispatch_inspect(a: &crate::cli::MinspectArgs) -> ! {
    // Subcommand output bisa di posisi pertama (tools.md: `minspect stats`)
    const CMDS: [&str; 9] = [
        "stats",
        "modules",
        "hierarchy",
        "packages",
        "classes",
        "interfaces",
        "parameters",
        "deps",
        "cache",
    ];
    let (command, targets): (Option<String>, Vec<String>) = match (
        a.targets.first().map(|s| s.as_str()),
        a.targets.last().map(|s| s.as_str()),
    ) {
        // Command di posisi pertama: `minspect stats rtl/`
        (Some(first), _) if CMDS.contains(&first) => {
            (Some(first.to_string()), a.targets[1..].to_vec())
        }
        // Command di posisi terakhir: `minspect rtl/ stats`
        (_, Some(last)) if CMDS.contains(&last) && a.targets.len() > 1 => {
            let mut t = a.targets.clone();
            t.pop();
            (Some(last.to_string()), t)
        }
        _ => (None, a.targets.clone()),
    };
    let args = mivon_api::tools::inspect::InspectArgs {
        targets: &targets,
        command,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        json: a.json,
    };
    exit_tool(mivon_api::tools::inspect::run(&args));
}

pub(crate) fn dispatch_lint(a: &crate::cli::MlintArgs) -> ! {
    let args = mivon_api::tools::lint::LintArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        all: a.all,
        unused: a.unused,
        width: a.width,
        latch: a.latch,
        loop_check: a.loop_check,
        fsm: a.fsm,
        case_analysis: a.case_analysis,
        clock_gating: a.clock_gating,
        power: a.power,
        memory: a.memory,
        gate_opt: a.gate_opt,
        quiet: a.quiet,
    };
    exit_tool(mivon_api::tools::lint::run(&args));
}

pub(crate) fn dispatch_elab(a: &crate::cli::MelabArgs) -> ! {
    let args = mivon_api::tools::elab::ElabArgs {
        files: &a.files,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        tree: a.tree,
        params: a.params,
        signals: a.signals,
        reset_domain: a.reset_domain,
        from_cache: a.from_cache,
    };
    exit_tool(mivon_api::tools::elab::run(&args));
}

pub(crate) fn dispatch_sim(a: &crate::cli::MsimArgs) -> ! {
    let args = mivon_api::tools::sim::SimArgs {
        files: &a.files,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        max_time: a.max_time.unwrap_or(DEFAULT_MAX_TIME_NS),
        output: a.output.as_deref(),
        fst: a.fst,
        assertions: a.assertions,
        coverage: a.coverage,
    };
    exit_tool(mivon_api::tools::sim::run(&args));
}

pub(crate) fn dispatch_cov(a: &crate::cli::McovArgs) -> ! {
    let args = mivon_api::tools::cov::CovArgs {
        files: &a.files,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        max_time: a.max_time.unwrap_or(DEFAULT_MAX_TIME_NS),
        output: a.output.as_deref(),
        json: a.json,
        html: a.html,
        threshold: a.threshold,
    };
    exit_tool(mivon_api::tools::cov::run(&args));
}

pub(crate) fn dispatch_wave(a: &crate::cli::MwaveArgs) -> ! {
    let args = match &a.cmd {
        crate::cli::MwaveCmd::Merge { inputs, output } => mivon_api::tools::wave::WaveArgs::Merge {
            inputs: inputs.clone(),
            output: output.clone(),
        },
        crate::cli::MwaveCmd::Export {
            input,
            format,
            output,
        } => mivon_api::tools::wave::WaveArgs::Export {
            input: input.clone(),
            format: format.clone(),
            output: output.clone(),
        },
        crate::cli::MwaveCmd::Filter {
            input,
            signals,
            output,
        } => mivon_api::tools::wave::WaveArgs::Filter {
            input: input.clone(),
            signals: signals.clone(),
            output: output.clone(),
        },
        crate::cli::MwaveCmd::Compare { a, b } => mivon_api::tools::wave::WaveArgs::Compare {
            a: a.clone(),
            b: b.clone(),
        },
        crate::cli::MwaveCmd::Search { input, patterns } => {
            mivon_api::tools::wave::WaveArgs::Search {
                input: input.clone(),
                patterns: patterns.clone(),
            }
        }
        crate::cli::MwaveCmd::Tree { input } => mivon_api::tools::wave::WaveArgs::Tree {
            input: input.clone(),
        },
        crate::cli::MwaveCmd::Stats { input } => mivon_api::tools::wave::WaveArgs::Stats {
            input: input.clone(),
        },
        crate::cli::MwaveCmd::Get {
            input,
            signals,
            at,
            range,
        } => mivon_api::tools::wave::WaveArgs::Get {
            input: input.clone(),
            signals: signals.clone(),
            at: *at,
            range: *range,
        },
        crate::cli::MwaveCmd::Decode { input, proto } => mivon_api::tools::wave::WaveArgs::Decode {
            input: input.clone(),
            proto: proto.clone(),
        },
        crate::cli::MwaveCmd::Debug { input, output } => mivon_api::tools::wave::WaveArgs::Debug {
            input: input.clone(),
            output: output.clone(),
        },
    };
    exit_tool(mivon_api::tools::wave::run(&args));
}

pub(crate) fn dispatch_fmt(a: &crate::cli::MfmtArgs) -> ! {
    let args = mivon_api::tools::fmt::FmtArgs {
        files: &a.files,
        inplace: a.inplace,
        check: a.check,
        indent: a.indent,
    };
    exit_tool(mivon_api::tools::fmt::run(&args));
}

pub(crate) fn dispatch_gen(a: &crate::cli::MgenArgs) -> ! {
    let args = mivon_api::tools::gen::GenArgs {
        targets: &a.targets,
        output: a.output.clone(),
        stdout: a.stdout,
        check: a.check,
        svh_only: a.svh_only,
        sv_only: a.sv_only,
        no_check: a.no_check,
        package: a.package.clone(),
        verbose: a.verbose,
    };
    exit_tool(mivon_api::tools::gen::run(&args));
}

pub(crate) fn dispatch_prof(a: &crate::cli::MprofArgs) -> ! {
    let args = mivon_api::tools::prof::ProfArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        max_time: a.max_time.unwrap_or(DEFAULT_MAX_TIME_NS),
        cached: a.cached,
    };
    exit_tool(mivon_api::tools::prof::run(&args));
}

pub(crate) fn dispatch_check(a: &crate::cli::McheckArgs) -> ! {
    let args = mivon_api::tools::check::CheckArgs {
        targets: &a.targets,
        all: a.all,
        missing: a.missing,
        circular: a.circular,
        deps: a.deps,
        cycles: a.cycles,
        timescale: a.timescale,
        ast_diff: a.ast_diff.as_deref(),
        sv_version: a.sv_version,
    };
    exit_tool(mivon_api::tools::check::run(&args));
}

pub(crate) fn dispatch_bench(a: &crate::cli::MbenchArgs) -> ! {
    let args = mivon_api::tools::bench::BenchArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        runs: a.runs,
    };
    exit_tool(mivon_api::tools::bench::run(&args));
}

pub(crate) fn dispatch_synth(a: &crate::cli::SynthArgs) -> ! {
    let args = mivon_api::tools::synth::SynthArgs {
        targets: &a.targets,
        incdirs: &a.incdirs,
        defines: &a.defines,
        top: a.top.as_deref(),
        output: a.output.clone(),
        check_only: a.check_only,
        device: a.device.clone(),
        preset: a.preset.clone(),
        emit_mvnet: a.emit_mvnet,
        dump_sir: a.dump_sir,
        dump_sir_opt: a.dump_sir_opt,
        dump_netlist: a.dump_netlist,
        emit_netlist: a.emit_netlist,
        tech_map: a.tech_map,
        report_util: a.report_util.clone(),
        constraint: a.constraint.clone(),
        timing: a.timing,
        fsm_report: a.fsm_report,
        quiet: a.quiet,
    };
    exit_tool(mivon_api::tools::synth::run(&args));
}

pub(crate) fn dispatch_update(a: &crate::cli::UpdateArgs) -> ! {
    let check_only = matches!(a.cmd, Some(crate::cli::UpdateCmd::Check));
    let args = mivon_api::tools::update::UpdateArgs {
        check_only,
        channel: Some(a.channel.as_str()),
        version: a.version.as_deref(),
        rollback: a.rollback,
        yes: a.yes,
        manifest_url: a.manifest_url.as_deref(),
        exe_path: None,
    };
    exit_tool(mivon_api::tools::update::run(&args));
}
