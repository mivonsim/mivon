//! Layer komponen & sequence UVM — uvm_component, uvm_sequence_item,
//! uvm_sequence, uvm_sequencer, uvm_driver, uvm_monitor, analysis port/imp,
//! dan uvm_subscriber (F22/F24).
//! 1 file = 1 tanggung jawab: hanya hierarki komponen + handshake sequence —
//! objek dasar (uvm_object/report) di object.rs, register model di reg.rs.

use super::super::SimulationEngine;
use crate::simulator::types::*;
use crate::simulator::util::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;

impl SimulationEngine {
    pub(crate) fn execute_uvm_component_method(
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
                let parent_obj = args.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                self.uvm_object_data
                    .insert(obj_id, UvmObjectData { name: name.clone() });
                let mut cd = UvmComponentData {
                    parent: None,
                    children: Vec::new(),
                    report_verbosity: super::object::UVM_MEDIUM,
                };
                if parent_obj != 0 {
                    cd.parent = Some(parent_obj);
                    if let Some(pd) = self.uvm_component_data.get_mut(&parent_obj) {
                        pd.children.push(obj_id);
                    }
                }
                self.uvm_component_data.insert(obj_id, cd);
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_full_name" => {
                let mut names = Vec::new();
                let mut current = Some(obj_id);
                while let Some(id) = current {
                    let n = self
                        .uvm_object_data
                        .get(&id)
                        .map(|d| d.name.clone())
                        .unwrap_or_default();
                    names.push(n);
                    current = self.uvm_component_data.get(&id).and_then(|d| d.parent);
                }
                names.reverse();
                let full = names.join(".");
                Ok(string_to_logicvec(&full))
            }
            "get_parent" => {
                let pid = self
                    .uvm_component_data
                    .get(&obj_id)
                    .and_then(|d| d.parent)
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(pid as u64, 64))
            }
            "get_num_children" => {
                let n = self
                    .uvm_component_data
                    .get(&obj_id)
                    .map(|d| d.children.len() as u64)
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(n, 32))
            }
            "get_child" => {
                let idx = args.first().map(|a| a.to_u64() as usize).unwrap_or(0);
                let cid = self
                    .uvm_component_data
                    .get(&obj_id)
                    .and_then(|d| d.children.get(idx).copied())
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(cid as u64, 64))
            }
            "has_child" => {
                let name = args.first().map(logicvec_to_string).unwrap_or_default();
                let found = self
                    .uvm_component_data
                    .get(&obj_id)
                    .map(|d| {
                        d.children.iter().any(|cid| {
                            self.uvm_object_data
                                .get(cid)
                                .map(|od| od.name == name)
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                Ok(LogicVec::from_u64(if found { 1 } else { 0 }, 1))
            }
            "set_report_verbosity" => {
                let level = args
                    .first()
                    .map(|a| a.to_u64() as u32)
                    .unwrap_or(super::object::UVM_MEDIUM);
                if let Some(d) = self.uvm_component_data.get_mut(&obj_id) {
                    d.report_verbosity = level;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_report_verbosity" => {
                let level = self
                    .uvm_component_data
                    .get(&obj_id)
                    .map(|d| d.report_verbosity)
                    .unwrap_or(super::object::UVM_MEDIUM);
                Ok(LogicVec::from_u64(level as u64, 32))
            }
            // Fase UVM adalah no-op di base class (super.xxx_phase(phase)
            // tidak melakukan apa-apa di UVM asli; subclass yang override
            // dieksekusi normal via dispatch user method).
            "build_phase"
            | "connect_phase"
            | "end_of_elaboration_phase"
            | "start_of_simulation_phase"
            | "run_phase"
            | "extract_phase"
            | "check_phase"
            | "report_phase"
            | "final_phase"
            | "reset_phase"
            | "configure_phase"
            | "main_phase"
            | "shutdown_phase" => Ok(LogicVec::from_u64(1, 1)),
            _ => self.execute_uvm_report_object_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_sequence_item_method(
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
                // Review R196 NIT-8: pakai insert (parity uvm_object::new),
                // bukan or_insert_with — objek sudah di-pre-seed nama kosong
                // saat alokasi, sehingga or_insert membuat get_name()
                // mengembalikan "" senyap.
                self.uvm_object_data.insert(obj_id, UvmObjectData { name });
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
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_sequence_method(
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
                    .entry(obj_id)
                    .or_insert_with(|| UvmObjectData { name });
                Ok(LogicVec::from_u64(1, 1))
            }
            "start" => {
                // args[0] = sequencer obj_id
                let seqr_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                // Store sequencer obj_id on the sequence object's fields.
                // `m_sequencer` adalah nama standar UVM (dipakai macro
                // `uvm_create` untuk `set_sequencer(m_sequencer)`) — tanpa ini,
                // `m_sequencer != null` gagal resolve → warning RT0001.
                if let Some(obj) = self.state.get_object_mut(obj_id) {
                    let v = LogicVec::from_u64(seqr_id as u64, 64);
                    obj.fields.insert(Symbol::intern("__sequencer"), v.clone());
                    obj.fields.insert(Symbol::intern("m_sequencer"), v);
                }
                // Call body()
                if self
                    .find_method_quiet(
                        &{
                            self.state
                                .get_object(obj_id)
                                .map(|o| o.class_name.to_string())
                                .unwrap_or_default()
                        },
                        "body",
                    )
                    .is_some()
                {
                    self.execute_method(obj_id, "body", &[])?;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "body" => Ok(LogicVec::from_u64(1, 1)),
            "start_item" => {
                let item_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                // Get sequencer from stored field
                let seqr_id = self
                    .state
                    .get_object(obj_id)
                    .and_then(|o| o.fields.get(&Symbol::intern("__sequencer")))
                    .map(|v| v.to_u64() as ObjId)
                    .unwrap_or(0);
                if seqr_id != 0 {
                    self.uvm_sequencer_data
                        .entry(seqr_id)
                        .or_insert_with(|| UvmSequencerData {
                            item_queue: Vec::new(),
                            current_item: None,
                        })
                        .item_queue
                        .push(item_id);
                    // F24: release waiter getter (driver get_next_item yang
                    // block saat queue kosong) — item baru tersedia.
                    self.uvm_seq_release_getters(seqr_id)?;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "finish_item" => Ok(LogicVec::from_u64(1, 1)),
            "set_sequencer" => {
                // UVM: `set_sequencer(m_sequencer)` dipanggil oleh macro
                // `uvm_create`. Simpan sequencer pada field sequence agar
                // `m_sequencer`/`__sequencer` resolve (dan start_item tahu
                // target queue).
                let seqr_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                if let Some(obj) = self.state.get_object_mut(obj_id) {
                    let v = LogicVec::from_u64(seqr_id as u64, 64);
                    obj.fields.insert(Symbol::intern("__sequencer"), v.clone());
                    obj.fields.insert(Symbol::intern("m_sequencer"), v);
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_sequencer" => {
                let seqr_id = self
                    .state
                    .get_object(obj_id)
                    .and_then(|o| o.fields.get(&Symbol::intern("__sequencer")))
                    .cloned()
                    .unwrap_or(LogicVec::from_u64(0, 64));
                Ok(seqr_id)
            }
            "create" => {
                let name = args.first().map(logicvec_to_string).unwrap_or_default();
                // Create a new object of the sequence's type
                let class_name = self
                    .state
                    .get_object(obj_id)
                    .map(|o| o.class_name)
                    .unwrap_or_default();
                let child = self.state.alloc_object(Symbol::intern(class_name.as_str()));
                // Set name on the new object
                self.uvm_object_data
                    .entry(child)
                    .or_insert_with(|| UvmObjectData { name });
                // Initialize fields from class def
                if let Some(cls) = self.design.classes.get(&class_name) {
                    if let Some(obj) = self.state.get_object_mut(child) {
                        for field in &cls.fields {
                            obj.fields
                                .entry(field.name)
                                .or_insert_with(|| LogicVec::from_u64(0, field.width));
                        }
                    }
                }
                Ok(LogicVec::from_u64(child as u64, 64))
            }
            _ => self.execute_uvm_sequence_item_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_sequencer_method(
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
                let parent_obj = args.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                self.uvm_object_data
                    .insert(obj_id, UvmObjectData { name: name.clone() });
                let mut cd = UvmComponentData {
                    parent: None,
                    children: Vec::new(),
                    report_verbosity: super::object::UVM_MEDIUM,
                };
                if parent_obj != 0 {
                    cd.parent = Some(parent_obj);
                    if let Some(pd) = self.uvm_component_data.get_mut(&parent_obj) {
                        pd.children.push(obj_id);
                    }
                }
                self.uvm_component_data.insert(obj_id, cd);
                self.uvm_sequencer_data.insert(
                    obj_id,
                    UvmSequencerData {
                        item_queue: Vec::new(),
                        current_item: None,
                    },
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_next_item" => {
                let data = self.uvm_sequencer_data.get_mut(&obj_id).ok_or_else(|| {
                    SimError::with_diag(DiagCode::NullHandle, "sequencer not initialized")
                })?;
                let item = data.item_queue.first().copied().unwrap_or(0);
                data.current_item = data.item_queue.first().copied();
                Ok(LogicVec::from_u64(item as u64, 64))
            }
            "item_done" => {
                // F24: release item current + waiter finish_item:{item}. Jangan
                // double-pop: block.rs proceed path (get_next_item) sudah
                // meng-pop item dari queue. Hanya pop bila item MASIH terdepan
                // di queue (pola lama F17: builtin get_next_item tanpa pop).
                let done_item = self.uvm_sequencer_data.get_mut(&obj_id).and_then(|sd| {
                    let item = sd.current_item.take();
                    if let Some(it) = item {
                        if sd.item_queue.first() == Some(&it) {
                            sd.item_queue.remove(0);
                        }
                    }
                    item
                });
                if let Some(item_id) = done_item {
                    self.uvm_seq_release_finishers(obj_id, item_id)?;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            _ => self.execute_uvm_component_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_driver_method(
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
                let parent_obj = args.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                self.uvm_object_data
                    .insert(obj_id, UvmObjectData { name: name.clone() });
                let mut cd = UvmComponentData {
                    parent: None,
                    children: Vec::new(),
                    report_verbosity: super::object::UVM_MEDIUM,
                };
                if parent_obj != 0 {
                    cd.parent = Some(parent_obj);
                    if let Some(pd) = self.uvm_component_data.get_mut(&parent_obj) {
                        pd.children.push(obj_id);
                    }
                }
                self.uvm_component_data.insert(obj_id, cd);
                self.uvm_driver_data.insert(
                    obj_id,
                    UvmDriverData {
                        sequencer_id: None,
                        current_item: None,
                    },
                );
                Ok(LogicVec::from_u64(1, 1))
            }
            "set_sequencer" => {
                let seqr_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                if let Some(data) = self.uvm_driver_data.get_mut(&obj_id) {
                    data.sequencer_id = Some(seqr_id);
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_next_item" => {
                let data = self.uvm_driver_data.get(&obj_id).ok_or_else(|| {
                    SimError::with_diag(DiagCode::NullHandle, "driver not initialized")
                })?;
                let seqr_id = data.sequencer_id.unwrap_or(0);
                if seqr_id != 0 {
                    self.execute_uvm_sequencer_method(seqr_id, "get_next_item", args)
                } else {
                    Ok(LogicVec::from_u64(0, 64))
                }
            }
            "item_done" => {
                let data = self.uvm_driver_data.get(&obj_id).ok_or_else(|| {
                    self.diag_error(DiagCode::NullHandle, "driver not initialized")
                })?;
                let seqr_id = data.sequencer_id.unwrap_or(0);
                if seqr_id != 0 {
                    self.execute_uvm_sequencer_method(seqr_id, "item_done", args)
                } else {
                    Ok(LogicVec::from_u64(1, 1))
                }
            }
            _ => self.execute_uvm_component_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_monitor_method(
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
                let parent_obj = args.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                self.uvm_object_data
                    .insert(obj_id, UvmObjectData { name: name.clone() });
                let mut cd = UvmComponentData {
                    parent: None,
                    children: Vec::new(),
                    report_verbosity: super::object::UVM_MEDIUM,
                };
                if parent_obj != 0 {
                    cd.parent = Some(parent_obj);
                    if let Some(pd) = self.uvm_component_data.get_mut(&parent_obj) {
                        pd.children.push(obj_id);
                    }
                }
                self.uvm_component_data.insert(obj_id, cd);
                Ok(LogicVec::from_u64(1, 1))
            }
            _ => self.execute_uvm_component_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_analysis_port_method(
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
                self.uvm_analysis_port_data.insert(
                    obj_id,
                    UvmAnalysisPortData {
                        connections: Vec::new(),
                        name: name.clone(),
                    },
                );
                self.uvm_object_data
                    .entry(obj_id)
                    .or_insert_with(|| UvmObjectData { name });
                Ok(LogicVec::from_u64(1, 1))
            }
            "connect" => {
                let imp_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                if let Some(data) = self.uvm_analysis_port_data.get_mut(&obj_id) {
                    data.connections.push(imp_id);
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "write" => {
                let item_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                let connections = self
                    .uvm_analysis_port_data
                    .get(&obj_id)
                    .map(|d| d.connections.clone())
                    .unwrap_or_default();
                for imp_id in &connections {
                    let imp_args = vec![LogicVec::from_u64(item_id as u64, 64)];
                    self.execute_uvm_analysis_imp_method(*imp_id, "write", &imp_args)?;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_analysis_imp_method(
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
                let parent_obj = args.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                self.uvm_analysis_imp_data.insert(
                    obj_id,
                    UvmAnalysisImpData {
                        parent: Some(parent_obj),
                        name: name.clone(),
                    },
                );
                self.uvm_object_data
                    .entry(obj_id)
                    .or_insert_with(|| UvmObjectData { name });
                Ok(LogicVec::from_u64(1, 1))
            }
            "write" => {
                let item_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                let parent = self
                    .uvm_analysis_imp_data
                    .get(&obj_id)
                    .and_then(|d| d.parent)
                    .unwrap_or(0);
                let parent_name = if parent != 0 {
                    self.state
                        .get_object(parent)
                        .map(|o| o.class_name.to_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                if parent != 0
                    && !parent_name.is_empty()
                    && self.find_method_quiet(&parent_name, "write").is_some()
                {
                    let write_args = vec![LogicVec::from_u64(item_id as u64, 64)];
                    self.execute_method(parent, "write", &write_args)?;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }

    // ─── uvm_subscriber (F22) ───────────────────────────────────────────
    // Komponen penerima broadcast analysis port. `new` membangun analysis_imp
    // child secara otomatis (class `__uvm_analysis_imp`, parent = subscriber)
    // dan menyimpannya di field `analysis_imp` — pola UVM asli
    // `uvm_subscriber` punya `uvm_analysis_imp #(T) analysis_imp` internal.
    // User `class my_sub extends uvm_subscriber` cukup meng-override `write()`;
    // `aport.connect(my_sub.analysis_imp)` → `aport.write(item)` → imp
    // (execute_uvm_analysis_imp_method) → parent.write (override user).
    pub(crate) fn execute_uvm_subscriber_method(
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
                    .entry(obj_id)
                    .or_insert_with(|| UvmObjectData { name: name.clone() });
                // Analysis-imp internal: alokasi objek `__uvm_analysis_imp`
                // dengan parent = subscriber, simpan id di field analysis_imp.
                let imp_name = format!("{}_imp", if name.is_empty() { "sub" } else { &name });
                let imp_id = self
                    .state
                    .alloc_object(Symbol::intern("__uvm_analysis_imp"));
                self.uvm_analysis_imp_data.insert(
                    imp_id,
                    UvmAnalysisImpData {
                        parent: Some(obj_id),
                        name: imp_name.clone(),
                    },
                );
                self.uvm_object_data
                    .entry(imp_id)
                    .or_insert_with(|| UvmObjectData { name: imp_name });
                if let Some(obj) = self.state.get_object_mut(obj_id) {
                    obj.fields.insert(
                        Symbol::intern("analysis_imp"),
                        LogicVec::from_u64(imp_id as u64, 64),
                    );
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            // `write` tanpa override user: no-op (broadcast tetap diterima,
            // tidak error). Override user dijalankan via jalur normal method.rs
            // (find_method_in_hierarchy menemukan write di class user).
            "write" => Ok(LogicVec::from_u64(1, 1)),
            _ => self.execute_uvm_component_method(obj_id, method, args),
        }
    }
}
