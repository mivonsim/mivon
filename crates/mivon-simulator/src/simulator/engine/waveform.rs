/// Waveform and display output management for SimulationEngine.
/// Contains VCD/FST dump timing, monitor checking, and strobe processing.
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_ir::*;
use std::io::Write;

use super::SimulationEngine;

impl SimulationEngine {
    /// Write VCD time header for current simulation time step.
    pub(crate) fn dump_vcd_time(&mut self) -> Result<(), SimError> {
        if let Some(ref mut vcd) = self.vcd {
            vcd.write_time_header(self.state.time)?;
        }
        Ok(())
    }

    /// Write FST time header for current simulation time step.
    pub(crate) fn dump_fst_time(&mut self) -> Result<(), SimError> {
        if let Some(ref mut fst) = self.fst {
            fst.write_time_header(self.state.time)?;
        }
        Ok(())
    }

    /// Check $monitor and $fmonitor for value changes and print/output them.
    pub(crate) fn check_monitor(&mut self) -> Result<(), SimError> {
        if let Some(ref args) = self.monitor_args.clone() {
            let new_vals: Vec<LogicVec> = args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            let changed = match self.monitor_last_values {
                Some(ref old) => new_vals != *old,
                None => true,
            };
            if changed {
                let msg = self.format_display(args);
                print!("{}", msg);
                self.monitor_last_values = Some(new_vals);
            }
        }
        let fmonitor: Vec<(u32, Vec<IrExpr>, Vec<LogicVec>)> = self
            .fmonitor_map
            .iter()
            .map(|(h, (args, last))| (*h, args.clone(), last.clone()))
            .collect();
        for (handle, args, last) in fmonitor {
            let new_vals: Vec<LogicVec> = args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            if new_vals != last {
                let msg = self.format_display(&args);
                if let Some(f) = self.file_handles.get_mut(&handle) {
                    let _ = write!(f, "{}", msg);
                }
                self.fmonitor_map.insert(handle, (args, new_vals));
            }
        }
        Ok(())
    }

    /// Process $strobe and $fstrobe events in the postponed region.
    pub(crate) fn process_strobe(&mut self) -> Result<(), SimError> {
        let events = std::mem::take(&mut self.strobe_events);
        for (radix, args) in &events {
            let msg = self.format_display_radix(args, *radix);
            println!("{}", msg);
        }
        let fstrobe = std::mem::take(&mut self.fstrobe_events);
        for (handle, radix, args) in &fstrobe {
            let msg = self.format_display_radix(args, *radix);
            if let Some(f) = self.file_handles.get_mut(handle) {
                let _ = writeln!(f, "{}", msg);
            }
        }
        Ok(())
    }

    /// Dump current signal states to VCD.
    pub(crate) fn dump_vcd_state(&mut self) -> Result<(), SimError> {
        if let Some(ref mut vcd) = self.vcd {
            vcd.dump_state(&self.design, &self.state.signals)?;
            vcd.maybe_flush().map_err(|e| {
                SimError::with_diag(DiagCode::WaveformError, format!("VCD flush error: {}", e))
            })?;
        }
        Ok(())
    }

    /// Dump current signal states to FST.
    pub(crate) fn dump_fst_state(&mut self) -> Result<(), SimError> {
        if let Some(ref mut fst) = self.fst {
            fst.dump_state(&self.design, &self.state.signals)?;
            fst.maybe_flush().map_err(|e| {
                SimError::with_diag(DiagCode::WaveformError, format!("FST flush error: {}", e))
            })?;
        }
        Ok(())
    }

    /// Dump current signal states to CSV.
    pub(crate) fn dump_csv_state(&mut self) -> Result<(), SimError> {
        if let Some(ref mut csv) = self.csv {
            csv.dump_state(self.state.time, &self.state.signals)
                .map_err(|e| {
                    SimError::with_diag(DiagCode::WaveformError, format!("CSV dump error: {}", e))
                })?;
        }
        Ok(())
    }

    /// Record signal statistics at current time step.
    pub(crate) fn record_signal_stats(&mut self) {
        if let Some(ref mut stats) = self.signal_stats {
            stats.record(self.state.time, &self.state.signals);
        }
    }

    /// Flush and close CSV writer.
    pub fn close_csv(&mut self) -> Result<(), SimError> {
        if let Some(ref mut csv) = self.csv {
            csv.close().map_err(|e| {
                SimError::with_diag(DiagCode::WaveformError, format!("CSV close error: {}", e))
            })?;
        }
        Ok(())
    }
}
