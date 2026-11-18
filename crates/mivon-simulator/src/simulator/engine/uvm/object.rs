//! Layer objek dasar UVM — uvm_object, uvm_report_object, dan callback
//! (F16/F20). Method builtin untuk class builtin paling dasar: `new`,
//! `get_name`/`set_name`, `print`, objection, dan `uvm_report_*` yang
//! disambungkan ke severity system.
//! 1 file = 1 tanggung jawab: hanya layer objek/report/callback — komponen,
//! sequence, dan register model tinggal di component.rs / reg.rs.

use super::super::SimulationEngine;
use crate::simulator::types::*;
use crate::simulator::util::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;

/// VERIF-11: konstanta verbosity UVM (IEEE 1800 / UVM 1.2) — dipakai
/// `uvm_report_info(id, msg, verbosity)` filtering dan
/// set/get_report_verbosity(_level). Default komponen = UVM_MEDIUM.
pub const UVM_NONE: u32 = 0;
pub const UVM_LOW: u32 = 100;
pub const UVM_MEDIUM: u32 = 200;
pub const UVM_HIGH: u32 = 300;
pub const UVM_FULL: u32 = 400;

impl SimulationEngine {
    pub(crate) fn execute_uvm_callback_method(
        &mut self,
        obj_id: ObjId,
        method: &str,
        args: &[LogicVec],
    ) -> Result<LogicVec, SimError> {
        match method {
            "new" => {
                let name = if !args.is_empty() {
                    logicvec_to_string(&args[0])
                } else {
                    String::new()
                };
                self.uvm_object_data
                    .insert(obj_id, UvmObjectData { name: name.clone() });
                Ok(LogicVec::from_u64(1, 1))
            }
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }

    /// Execute uvm_callbacks#add: register a callback on a component type.
    pub(crate) fn execute_uvm_callbacks_add(
        &mut self,
        _obj_id: ObjId,
        method: &str,
        args: &[LogicVec],
    ) -> Result<LogicVec, SimError> {
        match method {
            "add" => {
                // uvm_callbacks#add(cb_obj, comp_type)
                let cb_obj_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                let comp_type = args.get(1).map(logicvec_to_string).unwrap_or_default();
                let cb_name = args.get(2).map(logicvec_to_string).unwrap_or_default();
                // Store by (component_type, cb_type_name)
                let cb_type = if let Some(obj) = self.state.get_object(cb_obj_id) {
                    obj.class_name.to_string()
                } else {
                    String::new()
                };
                let queue_key = (comp_type, cb_type.clone());
                let entry = self.callback_queues.entry(queue_key).or_insert_with(|| {
                    crate::simulator::types::UvmCallbackData {
                        cb_type_name: cb_type,
                        callbacks: Vec::new(),
                        enabled: true,
                    }
                });
                entry.callbacks.push((cb_obj_id, cb_name));
                Ok(LogicVec::from_u64(1, 1))
            }
            "delete" => {
                let cb_obj_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                let comp_type = args.get(1).map(logicvec_to_string).unwrap_or_default();
                let cb_type = if let Some(obj) = self.state.get_object(cb_obj_id) {
                    obj.class_name.to_string()
                } else {
                    String::new()
                };
                let queue_key = (comp_type, cb_type);
                if let Some(entry) = self.callback_queues.get_mut(&queue_key) {
                    entry.callbacks.retain(|(id, _)| *id != cb_obj_id);
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "display" => {
                for ((comp_type, cb_type), data) in &self.callback_queues {
                    println!(
                        "UVM_CALLBACK: {} registered on {} ({} callbacks, enabled={})",
                        cb_type,
                        comp_type,
                        data.callbacks.len(),
                        data.enabled
                    );
                    for (cb_id, _cb_name) in &data.callbacks {
                        let name = self
                            .uvm_object_data
                            .get(cb_id)
                            .map(|d| d.name.as_str())
                            .unwrap_or("unnamed");
                        println!("  - {} (obj_id={})", name, cb_id);
                    }
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            _ => Ok(LogicVec::from_u64(0, 1)),
        }
    }

    /// Invoke callbacks for a specific component type before/after method execution.
    pub(crate) fn invoke_callbacks(
        &mut self,
        comp_type: &str,
        callback_method: &str,
        args: &[LogicVec],
    ) -> Result<(), SimError> {
        // Check per-component-type, then per-parent-type (UVM callback inheritance)
        let mut visited = std::collections::HashSet::new();
        let mut current = Some(comp_type.to_string());
        while let Some(ct) = current {
            if visited.contains(&ct) {
                break;
            }
            visited.insert(ct.clone());

            // Check all callback queue entries for this component type
            let keys: Vec<(String, String)> = self
                .callback_queues
                .keys()
                .filter(|(ct_key, _)| ct_key == &ct)
                .cloned()
                .collect();

            for key in &keys {
                if let Some(data) = self.callback_queues.get(key) {
                    if !data.enabled {
                        continue;
                    }
                    // Invoke callback_method on each registered callback object
                    let cbs = data.callbacks.clone();
                    for (cb_id, _) in &cbs {
                        if self
                            .find_method_quiet(
                                &{
                                    self.state
                                        .get_object(*cb_id)
                                        .map(|o| o.class_name.to_string())
                                        .unwrap_or_default()
                                },
                                callback_method,
                            )
                            .is_some()
                        {
                            self.execute_method(*cb_id, callback_method, args)?;
                        }
                    }
                }
            }

            // Walk up component hierarchy for inherited callback registrations
            current = self
                .design
                .classes
                .get(&Symbol::intern(&ct))
                .and_then(|c| c.extends)
                .map(|s| s.to_string());
        }
        Ok(())
    }

    pub(crate) fn execute_uvm_object_method(
        &mut self,
        obj_id: ObjId,
        method: &str,
        args: &[LogicVec],
    ) -> Result<LogicVec, SimError> {
        match method {
            "new" => {
                let name = if !args.is_empty() {
                    logicvec_to_string(&args[0])
                } else {
                    String::new()
                };
                self.uvm_object_data.insert(obj_id, UvmObjectData { name });
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_name" => {
                let data = self.uvm_object_data.get(&obj_id).ok_or_else(|| {
                    SimError::with_diag(DiagCode::NullHandle, "uvm_object not initialized")
                })?;
                Ok(string_to_logicvec(&data.name))
            }
            "set_name" => {
                let name = if !args.is_empty() {
                    logicvec_to_string(&args[0])
                } else {
                    String::new()
                };
                if let Some(data) = self.uvm_object_data.get_mut(&obj_id) {
                    data.name = name;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_type_name" => {
                let class_name = self
                    .state
                    .get_object(obj_id)
                    .map(|o| o.class_name)
                    .unwrap_or_default();
                Ok(string_to_logicvec(class_name.as_str()))
            }
            "print" => {
                // VERIF-12: `print(printer)` memakai uvm_table_printer bila
                // argumen printer diberikan; tanpa argumen → format default
                // (nama + class) seperti sebelumnya.
                if let Some(printer_arg) = args.first() {
                    let printer_id = printer_arg.to_u64() as ObjId;
                    if printer_id > 0
                        && self.is_uvm_printer_hierarchy(
                            self.state
                                .get_object(printer_id)
                                .map(|o| o.class_name.as_str())
                                .unwrap_or_default(),
                        )
                    {
                        let s = self.format_uvm_object_table(obj_id);
                        println!("{}", s);
                        return Ok(LogicVec::from_u64(1, 1));
                    }
                }
                let data = self.uvm_object_data.get(&obj_id).ok_or_else(|| {
                    SimError::with_diag(DiagCode::NullHandle, "uvm_object not initialized")
                })?;
                let class_name = self
                    .state
                    .get_object(obj_id)
                    .map(|o| o.class_name)
                    .unwrap_or_default();
                println!(
                    "UVM_INFO @ {}: {} [{}]",
                    self.current_time, data.name, class_name
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            "raise_objection" => {
                self.objection_count = self.objection_count.saturating_add(1);
                // VERIF-05: per-objek + propagasi hierarki — raise pada
                // objek menaikkan count objek itu DAN semua ancestor
                // (uvm_component_data.parent chain). uvm_objection_data
                // per objek = raise langsung + propagasi dari descendants;
                // get_objection_count(obj) membacanya (semantik UVM).
                *self.uvm_objection_data.entry(obj_id).or_insert(0) += 1;
                let mut cur = self.uvm_component_data.get(&obj_id).and_then(|d| d.parent);
                while let Some(anc) = cur {
                    *self.uvm_objection_data.entry(anc).or_insert(0) += 1;
                    cur = self.uvm_component_data.get(&anc).and_then(|d| d.parent);
                }
                let name = self
                    .uvm_object_data
                    .get(&obj_id)
                    .map(|d| d.name.as_str())
                    .unwrap_or("unknown");
                println!(
                    "UVM_OBJECTION: {} raised (count={})",
                    name, self.objection_count
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            "drop_objection" => {
                let name = self
                    .uvm_object_data
                    .get(&obj_id)
                    .map(|d| d.name.as_str())
                    .unwrap_or("unknown");
                if self.objection_count > 0 {
                    self.objection_count -= 1;
                }
                // Per-objek + ancestors (saturating — drop berlebih tidak
                // menurunkan di bawah 0, sama seperti counter global).
                if let Some(c) = self.uvm_objection_data.get_mut(&obj_id) {
                    *c = c.saturating_sub(1);
                }
                let mut cur = self.uvm_component_data.get(&obj_id).and_then(|d| d.parent);
                while let Some(anc) = cur {
                    if let Some(c) = self.uvm_objection_data.get_mut(&anc) {
                        *c = c.saturating_sub(1);
                    }
                    cur = self.uvm_component_data.get(&anc).and_then(|d| d.parent);
                }
                println!(
                    "UVM_OBJECTION: {} dropped (count={})",
                    name, self.objection_count
                );
                if self.objection_count == 0 && !self.objection_triggered {
                    self.objection_triggered = true;
                    println!("UVM_PHASE: All objections dropped, ending test");
                    // F18: jalankan fase akhir (extract/check/report/final)
                    // pada tree root test SEBELUM menghentikan sim.
                    self.execute_report_phases()?;
                    self.running = false;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            // VERIF-05: query jumlah objection utk objek ini (termasuk
            // propagasi dari descendants). 0 bila tidak ada.
            "get_objection_count" => Ok(LogicVec::from_u64(
                self.uvm_objection_data.get(&obj_id).copied().unwrap_or(0),
                32,
            )),
            // Fase UVM apa pun = no-op di uvm_object (super.xxx_phase di
            // subclass user). Tanpa ini `super.end_of_elaboration_phase`
            // error RT9003 "uvm_object::end_of_elaboration_phase not
            // implemented" dan sim mati (core_ibex_base_test.sv:233).
            m if m.ends_with("_phase") => Ok(LogicVec::from_u64(1, 1)),
            _ => Err(self.diag_error(
                DiagCode::NotImplemented,
                format!("uvm_object::{} not implemented", method),
            )),
        }
    }

    pub(crate) fn execute_uvm_report_object_method(
        &mut self,
        obj_id: ObjId,
        method: &str,
        args: &[LogicVec],
    ) -> Result<LogicVec, SimError> {
        // F16: uvm_report_* disambungkan ke severity system (F14/F15) —
        // emit_severity mencetak dengan prefix severity, increment counter
        // (ringkasan akhir sim), dan untuk fatal set fatal_hit + running
        // (hentikan sim seketika + exit code CLI non-zero).
        match method {
            "uvm_report_info" => {
                let id = args.first().map(logicvec_to_string).unwrap_or_default();
                let msg = args.get(1).map(logicvec_to_string).unwrap_or_default();
                // VERIF-11: verbosity filtering — arg ke-3 = level verbosity
                // (UVM_LOW=100/MEDIUM=200/HIGH=300/FULL=400/NONE=0). Pesan
                // dicetak HANYA bila verbosity <= verbosity komponen saat ini
                // (current_this → uvm_component_data.report_verbosity, default
                // UVM_MEDIUM). Set_report_verbosity(level) mengontrolnya.
                let verbosity = args.get(2).map(|a| a.to_u64() as u32).unwrap_or(0);
                let comp_level = self
                    .current_this
                    .and_then(|id| self.uvm_component_data.get(&id))
                    .map(|d| d.report_verbosity)
                    .unwrap_or(UVM_MEDIUM);
                if verbosity > comp_level {
                    // Ditekan — tidak dicetak, tidak increment counter.
                    return Ok(LogicVec::from_u64(1, 1));
                }
                self.emit_severity(
                    "info",
                    &format!("@ {}: {} [{}]", self.current_time, msg, id),
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            "uvm_report_warning" => {
                let id = args.first().map(logicvec_to_string).unwrap_or_default();
                let msg = args.get(1).map(logicvec_to_string).unwrap_or_default();
                self.emit_severity(
                    "warning",
                    &format!("@ {}: {} [{}]", self.current_time, msg, id),
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            "uvm_report_error" => {
                let id = args.first().map(logicvec_to_string).unwrap_or_default();
                let msg = args.get(1).map(logicvec_to_string).unwrap_or_default();
                self.emit_severity(
                    "error",
                    &format!("@ {}: {} [{}]", self.current_time, msg, id),
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            "uvm_report_fatal" => {
                let id = args.first().map(logicvec_to_string).unwrap_or_default();
                let msg = args.get(1).map(logicvec_to_string).unwrap_or_default();
                self.emit_severity(
                    "fatal",
                    &format!("@ {}: {} [{}]", self.current_time, msg, id),
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            // VERIF-11: set/get_report_verbosity_level — level verbosity
            // objek (report_object level). Sama dgn set/get_report_verbosity
            // komponen; diterapkan ke uvm_component_data bila objek komponen.
            "set_report_verbosity_level" | "set_report_verbosity" => {
                let level = args
                    .first()
                    .map(|a| a.to_u64() as u32)
                    .unwrap_or(UVM_MEDIUM);
                if let Some(d) = self.uvm_component_data.get_mut(&obj_id) {
                    d.report_verbosity = level;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_report_verbosity_level" | "get_report_verbosity" => {
                let level = self
                    .uvm_component_data
                    .get(&obj_id)
                    .map(|d| d.report_verbosity)
                    .unwrap_or(UVM_MEDIUM);
                Ok(LogicVec::from_u64(level as u64, 32))
            }
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::compile_str;
    use mivon_core::Symbol;

    #[test]
    fn test_uvm_callback_register_add() {
        let design = compile_str("module top; endmodule").unwrap();
        let mut engine = SimulationEngine::new(design, 100);

        // Add a callback object to state
        let cb_obj_id = engine
            .state
            .alloc_object(Symbol::intern("my_callback"));
        engine.uvm_object_data.insert(
            cb_obj_id,
            UvmObjectData {
                name: "my_cb".to_string(),
            },
        );

        // Register callback: uvm_callbacks#add(cb_obj, comp_type, cb_name)
        let args = vec![
            LogicVec::from_u64(cb_obj_id as u64, 32),
            string_to_logicvec("my_component"),
            string_to_logicvec("my_callback_instance"),
        ];
        let result = engine.execute_uvm_callbacks_add(cb_obj_id, "add", &args);
        assert!(result.is_ok(), "callback add should succeed");

        // Verify callback was stored in callback_queues
        let key = ("my_component".to_string(), "my_callback".to_string());
        assert!(
            engine.callback_queues.contains_key(&key),
            "callback queue should contain entry for (my_component, my_callback)"
        );

        let data = engine.callback_queues.get(&key).unwrap();
        assert_eq!(data.callbacks.len(), 1);
        assert_eq!(data.callbacks[0].0, cb_obj_id);
        assert!(data.enabled);
    }

    #[test]
    fn test_uvm_callback_delete_removes_entry() {
        let design = compile_str("module top; endmodule").unwrap();
        let mut engine = SimulationEngine::new(design, 100);

        // Add a callback object to state
        let cb_obj_id = engine
            .state
            .alloc_object(Symbol::intern("my_callback"));
        engine.uvm_object_data.insert(
            cb_obj_id,
            UvmObjectData {
                name: "my_cb".to_string(),
            },
        );

        // Register then delete
        let add_args = vec![
            LogicVec::from_u64(cb_obj_id as u64, 32),
            string_to_logicvec("my_component"),
            string_to_logicvec("my_callback_instance"),
        ];
        assert!(engine
            .execute_uvm_callbacks_add(cb_obj_id, "add", &add_args)
            .is_ok());

        let del_args = vec![
            LogicVec::from_u64(cb_obj_id as u64, 32),
            string_to_logicvec("my_component"),
        ];
        assert!(engine
            .execute_uvm_callbacks_add(cb_obj_id, "delete", &del_args)
            .is_ok());

        let key = ("my_component".to_string(), "my_callback".to_string());
        if let Some(data) = engine.callback_queues.get(&key) {
            assert_eq!(
                data.callbacks.len(),
                0,
                "callback should be removed after delete"
            );
        }
    }
}
