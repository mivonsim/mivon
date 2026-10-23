//! Bridge perintah Tcl ke pipeline mivon (FEAT-06).
//!
//! Perintah yang disediakan:
//! - `mivon compile ?-top NAME? ?-I DIR? ?-D MACRO? file...` — compile+elaborate,
//!   mengembalikan nama top module.
//! - `mivon simulate ?-T MAX?` — jalankan simulasi desain ter-compile.
//! - `mivon get ?-dec|-hex|-bin? SIGNAL` — nilai akhir sinyal.
//! - `mivon time` — waktu simulasi terakhir.
//! - `mivon signals` — daftar nama sinyal hasil simulasi.
//! - `mivon reset` — buang state desain/simulasi.
//!
//! 1 file = 1 tanggung jawab: hanya adaptor Tcl → engine, tanpa logika
//! interpreter (ada di `mod.rs`).

use super::{Interp, TclError, TclResult, Word};
use mivon_elaboration::elaborator::ElaborateMode;
use mivon_simulator::simulator::SimulationEngine;

impl Interp {
    pub(super) fn cmd_mivon(&mut self, args: &[Word]) -> TclResult {
        let sub = match args.first() {
            Some(w) => self.word_value(w)?,
            None => {
                return Err(TclError::Msg(
                    "wrong # args: should be \"mivon compile|simulate|get|time|signals|reset\""
                        .to_string(),
                ))
            }
        };
        match sub.as_str() {
            "compile" => self.bridge_compile(&args[1..]),
            "simulate" => self.bridge_simulate(&args[1..]),
            "get" => self.bridge_get(&args[1..]),
            "time" => Ok(self.sim_time.to_string()),
            "signals" => {
                let mut names: Vec<String> = self.signals.keys().cloned().collect();
                names.sort();
                Ok(super::format_list(&names))
            }
            "reset" => {
                self.design = None;
                self.signals.clear();
                self.sim_time = 0;
                Ok(String::new())
            }
            other => Err(TclError::Msg(format!(
                "unknown mivon subcommand \"{}\"",
                other
            ))),
        }
    }

    fn bridge_compile(&mut self, args: &[Word]) -> TclResult {
        let mut files: Vec<String> = Vec::new();
        let mut incdirs: Vec<String> = Vec::new();
        let mut defines: Vec<String> = Vec::new();
        let mut top: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            let flag = self.word_value(&args[i])?;
            match flag.as_str() {
                "-top" => {
                    i += 1;
                    top = Some(self.need_at(args, i, "mivon compile")?);
                }
                "-I" => {
                    i += 1;
                    incdirs.push(self.need_at(args, i, "mivon compile")?);
                }
                "-D" => {
                    i += 1;
                    defines.push(self.need_at(args, i, "mivon compile")?);
                }
                other => files.push(other.to_string()),
            }
            i += 1;
        }
        if files.is_empty() {
            return Err(TclError::Msg(
                "mivon compile: no input files".to_string(),
            ));
        }
        // Compile gagal tidak boleh meninggalkan state desain lama.
        self.design = None;
        self.signals.clear();
        self.sim_time = 0;
        let (_session, _design, ir) = crate::open_elaborated(
            &files,
            &incdirs,
            &defines,
            top.as_deref(),
            ElaborateMode::StrictSimulation,
        )
        .map_err(|e| TclError::Msg(e.to_string()))?;
        let top_name = ir.top.name.as_str().to_string();
        self.design = Some(ir);
        Ok(top_name)
    }

    fn bridge_simulate(&mut self, args: &[Word]) -> TclResult {
        let mut max_time: u64 = 1000;
        let mut i = 0;
        while i < args.len() {
            let flag = self.word_value(&args[i])?;
            if flag == "-T" {
                i += 1;
                let text = self.need_at(args, i, "mivon simulate")?;
                max_time = text.parse::<u64>().map_err(|_| {
                    TclError::Msg(format!("mivon simulate: invalid -T value \"{}\"", text))
                })?;
            } else {
                return Err(TclError::Msg(format!(
                    "mivon simulate: unknown option \"{}\"",
                    flag
                )));
            }
            i += 1;
        }
        let design = self.design.clone().ok_or_else(|| {
            TclError::Msg("mivon simulate: no design compiled (run \"mivon compile\" first)".to_string())
        })?;
        let mut engine = SimulationEngine::new(design, max_time);
        engine.run().map_err(|e| TclError::Msg(e.to_string()))?;
        let mut signals = std::collections::HashMap::new();
        for (index, info) in engine.design.top.signals.iter().enumerate() {
            signals.insert(
                info.name.as_str().to_string(),
                engine.state.read_signal(index).clone(),
            );
        }
        for (name, id) in &engine.design.hier_signal_map {
            signals.insert(name.as_str().to_string(), engine.state.read_signal(*id).clone());
        }
        self.sim_time = engine.state.time;
        let count = signals.len();
        self.signals = signals;
        Ok(count.to_string())
    }

    fn bridge_get(&mut self, args: &[Word]) -> TclResult {
        let mut format = "dec";
        let mut name: Option<String> = None;
        for w in args {
            let text = self.word_value(w)?;
            match text.as_str() {
                "-dec" => format = "dec",
                "-hex" => format = "hex",
                "-bin" => format = "bin",
                other => name = Some(other.to_string()),
            }
        }
        let name = name.ok_or_else(|| {
            TclError::Msg("wrong # args: should be \"mivon get ?-dec|-hex|-bin? SIGNAL\"".to_string())
        })?;
        let value = self.lookup_signal(&name)?.ok_or_else(|| {
            TclError::Msg(format!(
                "mivon get: signal \"{}\" not found (compile+simulate first)",
                name
            ))
        })?;
        // Nilai X/Z atau lebar >64 bit tidak punya representasi desimal/hex
        // yang jujur — jangan kembalikan angka palsu.
        if format != "bin" {
            let plain = value
                .bits
                .iter()
                .all(|b| matches!(b, mivon_core::LogicVal::Zero | mivon_core::LogicVal::One));
            if !plain {
                return Err(TclError::Msg(format!(
                    "mivon get: signal \"{}\" contains X/Z; use -bin",
                    name
                )));
            }
            if value.width > 64 {
                return Err(TclError::Msg(format!(
                    "mivon get: signal \"{}\" is {} bits (>64); use -bin",
                    name, value.width
                )));
            }
        }
        Ok(match format {
            "hex" => format!("{:x}", value.to_u64()),
            "bin" => value.to_string(),
            _ => value.to_u64().to_string(),
        })
    }

    fn lookup_signal(&self, name: &str) -> Result<Option<&mivon_core::LogicVec>, TclError> {
        if let Some(v) = self.signals.get(name) {
            return Ok(Some(v));
        }
        let suffix = format!(".{}", name);
        let mut matches: Vec<(&String, &mivon_core::LogicVec)> = self
            .signals
            .iter()
            .filter(|(k, _)| k.ends_with(&suffix))
            .collect();
        matches.sort_by(|a, b| a.0.cmp(b.0));
        match matches.len() {
            0 => Ok(None),
            1 => Ok(Some(matches[0].1)),
            _ => {
                let names: Vec<&str> = matches.iter().map(|(k, _)| k.as_str()).collect();
                Err(TclError::Msg(format!(
                    "mivon get: signal \"{}\" ambiguous: {}",
                    name,
                    names.join(", ")
                )))
            }
        }
    }

    fn need_at(&mut self, args: &[Word], index: usize, cmd: &str) -> TclResult {
        match args.get(index) {
            Some(w) => self.word_value(w),
            None => Err(TclError::Msg(format!(
                "wrong # args: missing value after option for \"{}\"",
                cmd
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Interp;

    fn write_design(source: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dut.sv");
        std::fs::write(&path, source).expect("write sv");
        let path = path.to_string_lossy().to_string();
        (dir, path)
    }

    #[test]
    fn compile_simulate_and_read_signal() {
        let (_dir, path) = write_design(
            "module dut;\n\
             logic [7:0] a;\n\
             logic [7:0] y;\n\
             assign y = a + 8'd3;\n\
             initial begin a = 8'd5; #1 $finish; end\n\
             endmodule\n",
        );
        let mut interp = Interp::new();
        let script = format!(
            "mivon compile {}\nmivon simulate -T 10\nputs [mivon get y]\nputs [mivon get -bin y]\nputs [mivon time]",
            path
        );
        interp.run(&script).expect("script");
        assert_eq!(interp.output()[0], "8");
        assert_eq!(interp.output()[1], "00001000");
        // `$finish` di #1: engine boleh melaporkan waktu 1 atau 2 tergantung
        // region terakhir yang diproses — yang penting simulasi berhenti.
        let end_time: u64 = interp.output()[2].parse().expect("time integer");
        assert!((1..=2).contains(&end_time), "end time {}", end_time);
    }

    #[test]
    fn get_without_design_is_error() {
        let mut interp = Interp::new();
        let result = interp.run("mivon get y");
        assert!(result.is_err());
    }

    #[test]
    fn get_xz_signal_requires_bin() {
        let (_dir, path) = write_design(
            "module dut;\n\
             logic [3:0] a;\n\
             initial begin #1 $finish; end\n\
             endmodule\n",
        );
        let mut interp = Interp::new();
        interp
            .run(&format!("mivon compile {}\nmivon simulate -T 5", path))
            .expect("sim");
        assert!(interp.run("mivon get a").is_err(), "X/Z must not be reported as 0");
        interp.run("puts [mivon get -bin a]").expect("bin");
        assert_eq!(interp.output(), &["xxxx".to_string()]);
    }

    #[test]
    fn reset_clears_signals() {
        let (_dir, path) = write_design(
            "module dut;\n\
             logic a;\n\
             initial begin a = 1'b1; #1 $finish; end\n\
             endmodule\n",
        );
        let mut interp = Interp::new();
        interp
            .run(&format!("mivon compile {}\nmivon simulate -T 5", path))
            .expect("sim");
        interp.run("mivon reset").expect("reset");
        assert!(interp.run("mivon get a").is_err());
    }
}
