//! Layer register model UVM — uvm_reg_field, uvm_reg, uvm_reg_block,
//! uvm_reg_map (F27). Method builtin untuk akses/prediksi register:
//! `set`/`get`/`get_desired`/`set_desired`, `write`/`read`/`update`/`mirror`,
//! `randomize`, `reset`, dan pemetaan offset (`configure`, `add_reg`,
//! `get_reg_by_offset`).
//! 1 file = 1 tanggung jawab: hanya register model — objek dasar di
//! object.rs, komponen/sequence di component.rs.

use super::super::SimulationEngine;
use crate::simulator::types::*;
use crate::simulator::util::*;
use mivon_core::error::SimError;
use mivon_ir::*;

impl SimulationEngine {
    pub(crate) fn execute_uvm_reg_field_method(
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
                let parent_reg = args.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                self.uvm_object_data
                    .entry(obj_id)
                    .or_insert_with(|| UvmObjectData { name });
                let mut fd = UvmRegFieldData::new();
                if parent_reg != 0 {
                    fd.parent_reg = Some(parent_reg);
                    // Register this field with the parent register
                    if let Some(rd) = self.uvm_reg_data.get_mut(&parent_reg) {
                        rd.fields.push(obj_id);
                    }
                }
                self.uvm_reg_field_data.insert(obj_id, fd);
                Ok(LogicVec::from_u64(1, 1))
            }
            "set_access" => {
                let access = args.first().map(logicvec_to_string).unwrap_or_default();
                if let Some(fd) = self.uvm_reg_field_data.get_mut(&obj_id) {
                    fd.access = access;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "set" => {
                let val = args.first().cloned().unwrap_or(LogicVec::new(1));
                if let Some(fd) = self.uvm_reg_field_data.get_mut(&obj_id) {
                    fd.value = val.clone();
                    fd.desired = val;
                    fd.modified = true;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get" => {
                let val = self
                    .uvm_reg_field_data
                    .get(&obj_id)
                    .map(|fd| fd.value.clone())
                    .unwrap_or(LogicVec::new(1));
                Ok(val)
            }
            "get_desired" => {
                let val = self
                    .uvm_reg_field_data
                    .get(&obj_id)
                    .map(|fd| fd.desired.clone())
                    .unwrap_or(LogicVec::new(1));
                Ok(val)
            }
            "set_desired" => {
                let val = args.first().cloned().unwrap_or(LogicVec::new(1));
                if let Some(fd) = self.uvm_reg_field_data.get_mut(&obj_id) {
                    fd.desired = val;
                    fd.modified = true;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "randomize" => {
                if let Some(_fd) = self.uvm_reg_field_data.get(&obj_id) {
                    let class_name = self
                        .state
                        .get_object(obj_id)
                        .map(|o| o.class_name.to_string())
                        .unwrap_or_default();
                    if !class_name.is_empty() {
                        return self.execute_randomize(obj_id, class_name.as_str());
                    }
                }
                // Fallback: randomize via engine
                let seed = self
                    .current_time
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let width = self
                    .uvm_reg_field_data
                    .get(&obj_id)
                    .map(|fd| fd.width)
                    .unwrap_or(1);
                let rv = LogicVec::from_u64(seed, width);
                if let Some(fd) = self.uvm_reg_field_data.get_mut(&obj_id) {
                    fd.value = rv.clone();
                    fd.desired = rv.clone();
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "mirror" => {
                // Read from DUT via parent register (simplified: predict from current value)
                Ok(LogicVec::from_u64(1, 1))
            }
            "predict" => {
                let val = args.first().cloned().unwrap_or(LogicVec::new(1));
                if let Some(fd) = self.uvm_reg_field_data.get_mut(&obj_id) {
                    fd.value = val;
                    fd.modified = false;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "reset" => {
                if let Some(fd) = self.uvm_reg_field_data.get_mut(&obj_id) {
                    fd.value = LogicVec::new(fd.width.max(1));
                    fd.desired = LogicVec::new(fd.width.max(1));
                    fd.modified = false;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "set_bit_pos" => {
                let pos = args.first().map(|a| a.to_u64() as usize).unwrap_or(0);
                if let Some(fd) = self.uvm_reg_field_data.get_mut(&obj_id) {
                    fd.bit_pos = pos;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_bit_pos" => {
                let pos = self
                    .uvm_reg_field_data
                    .get(&obj_id)
                    .map(|fd| fd.bit_pos as u64)
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(pos, 32))
            }
            "get_n_bits" => {
                let w = self
                    .uvm_reg_field_data
                    .get(&obj_id)
                    .map(|fd| fd.width as u64)
                    .unwrap_or(1);
                Ok(LogicVec::from_u64(w, 32))
            }
            "get_access" => {
                let access = self
                    .uvm_reg_field_data
                    .get(&obj_id)
                    .map(|fd| fd.access.clone())
                    .unwrap_or_default();
                Ok(string_to_logicvec(&access))
            }
            "is_modified" => {
                let modified = self
                    .uvm_reg_field_data
                    .get(&obj_id)
                    .map(|fd| fd.modified)
                    .unwrap_or(false);
                Ok(LogicVec::from_u64(if modified { 1 } else { 0 }, 1))
            }
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_reg_method(
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
                self.uvm_reg_data.entry(obj_id).or_insert_with(|| {
                    let mut rd = UvmRegData::new();
                    if args.len() > 1 {
                        rd.width = args[1].to_u64() as usize;
                    }
                    if args.len() > 2 {
                        rd.address = args[2].to_u64();
                    }
                    rd
                });
                Ok(LogicVec::from_u64(1, 1))
            }
            "configure" => {
                // configure(parent_block, regfile_path, offset)
                let block_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                let offset = args.get(2).map(|a| a.to_u64()).unwrap_or(0);
                if block_id != 0 {
                    if let Some(rd) = self.uvm_reg_data.get_mut(&obj_id) {
                        rd.parent_block = Some(block_id);
                        if offset != 0 && args.len() > 2 {
                            rd.address = offset;
                        }
                    }
                    // Register with parent block (use local values, not borrowed references)
                    let reg_offset = self
                        .uvm_reg_data
                        .get(&obj_id)
                        .map(|rd| rd.address)
                        .unwrap_or(offset);
                    if let Some(bd) = self.uvm_reg_block_data.get_mut(&block_id) {
                        bd.regs_by_offset.insert(reg_offset, obj_id);
                        if let Some(map_id) = bd.default_map {
                            if let Some(md) = self.uvm_reg_map_data.get_mut(&map_id) {
                                md.regs_by_offset.insert(reg_offset, obj_id);
                            }
                        }
                    }
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "write" => {
                // write(status, value, map, path): model-side write (update desired + mirror)
                let val = args.get(1).cloned().unwrap_or(LogicVec::new(32));
                if let Some(rd) = self.uvm_reg_data.get_mut(&obj_id) {
                    rd.desired = val.clone();
                    rd.value = val;
                    rd.modified = true;
                }
                // Propagate to fields
                if let Some(rd) = self.uvm_reg_data.get(&obj_id) {
                    for fid in &rd.fields {
                        if let Some(fd) = self.uvm_reg_field_data.get_mut(fid) {
                            fd.modified = true;
                        }
                    }
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "read" => {
                // read(status, map, path): model-side read (return mirrored value)
                // Also returns status in first arg (simplified: always success)
                let val = self
                    .uvm_reg_data
                    .get(&obj_id)
                    .map(|rd| rd.value.clone())
                    .unwrap_or(LogicVec::new(32));
                Ok(val)
            }
            "set" => {
                let val = args.first().cloned().unwrap_or(LogicVec::new(32));
                if let Some(rd) = self.uvm_reg_data.get_mut(&obj_id) {
                    rd.desired = val.clone();
                    rd.modified = true;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get" => {
                let val = self
                    .uvm_reg_data
                    .get(&obj_id)
                    .map(|rd| rd.value.clone())
                    .unwrap_or(LogicVec::new(32));
                Ok(val)
            }
            "get_desired" => {
                let val = self
                    .uvm_reg_data
                    .get(&obj_id)
                    .map(|rd| rd.desired.clone())
                    .unwrap_or(LogicVec::new(32));
                Ok(val)
            }
            "update" => {
                // Write modified fields/registers to DUT (bus access)
                if let Some(rd) = self.uvm_reg_data.get_mut(&obj_id) {
                    if rd.modified {
                        rd.value = rd.desired.clone();
                        rd.modified = false;
                    }
                }
                // Reset field modified flags
                if let Some(rd) = self.uvm_reg_data.get(&obj_id) {
                    for fid in &rd.fields {
                        if let Some(fd) = self.uvm_reg_field_data.get_mut(fid) {
                            fd.modified = false;
                        }
                    }
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "mirror" => {
                // Read from DUT (simplified: keep current value)
                if let Some(rd) = self.uvm_reg_data.get_mut(&obj_id) {
                    rd.modified = false;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "randomize" => {
                let class_name = self
                    .state
                    .get_object(obj_id)
                    .map(|o| o.class_name.to_string())
                    .unwrap_or_default();
                if !class_name.is_empty() {
                    return self.execute_randomize(obj_id, class_name.as_str());
                }
                // Fallback: randomize each field
                if let Some(rd) = self.uvm_reg_data.get(&obj_id) {
                    let fields = rd.fields.clone();
                    for fid in &fields {
                        self.execute_uvm_reg_field_method(*fid, "randomize", &[])?;
                    }
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "reset" => {
                if let Some(rd) = self.uvm_reg_data.get_mut(&obj_id) {
                    rd.value = LogicVec::new(rd.width.max(1));
                    rd.desired = LogicVec::new(rd.width.max(1));
                    rd.modified = false;
                }
                // Reset all fields
                if let Some(rd) = self.uvm_reg_data.get(&obj_id) {
                    let fields = rd.fields.clone();
                    for fid in &fields {
                        self.execute_uvm_reg_field_method(*fid, "reset", &[])?;
                    }
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_fields" => {
                // Return list of field object IDs
                let fields = self
                    .uvm_reg_data
                    .get(&obj_id)
                    .map(|rd| rd.fields.clone())
                    .unwrap_or_default();
                // Pack field IDs into a single LogicVec (64-bit each)
                if fields.is_empty() {
                    Ok(LogicVec::new(0))
                } else {
                    let total_width = fields.len() * 64;
                    let mut bits = Vec::with_capacity(total_width);
                    for fid in &fields {
                        let id_vec = LogicVec::from_u64(*fid as u64, 64);
                        bits.extend(id_vec.bits.iter());
                    }
                    Ok(LogicVec {
                        width: total_width,
                        bits,
                    })
                }
            }
            "get_address" => {
                let addr = self
                    .uvm_reg_data
                    .get(&obj_id)
                    .map(|rd| rd.address)
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(addr, 64))
            }
            "get_n_bits" => {
                let w = self
                    .uvm_reg_data
                    .get(&obj_id)
                    .map(|rd| rd.width as u64)
                    .unwrap_or(32);
                Ok(LogicVec::from_u64(w, 32))
            }
            "is_modified" => {
                let modified = self
                    .uvm_reg_data
                    .get(&obj_id)
                    .map(|rd| rd.modified)
                    .unwrap_or(false);
                Ok(LogicVec::from_u64(if modified { 1 } else { 0 }, 1))
            }
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_reg_block_method(
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
                self.uvm_reg_block_data.entry(obj_id).or_default();
                Ok(LogicVec::from_u64(1, 1))
            }
            "build" => {
                // Build registers: typically overridden by user class
                // Default: no-op, user's build() creates and configures registers
                Ok(LogicVec::from_u64(1, 1))
            }
            "default_map" => {
                // Get/set default address map
                let default_map = self
                    .uvm_reg_block_data
                    .get(&obj_id)
                    .and_then(|bd| bd.default_map)
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(default_map as u64, 64))
            }
            "set_default_map" => {
                let map_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                if let Some(bd) = self.uvm_reg_block_data.get_mut(&obj_id) {
                    bd.default_map = Some(map_id);
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_reg_by_offset" => {
                let offset = args.first().map(|a| a.to_u64()).unwrap_or(0);
                let reg_id = self
                    .uvm_reg_block_data
                    .get(&obj_id)
                    .and_then(|bd| bd.regs_by_offset.get(&offset).copied())
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(reg_id as u64, 64))
            }
            "get_registers" => {
                // Return all register object IDs in this block
                let regs: Vec<u64> = self
                    .uvm_reg_block_data
                    .get(&obj_id)
                    .map(|bd| bd.regs_by_offset.values().map(|&id| id as u64).collect())
                    .unwrap_or_default();
                if regs.is_empty() {
                    Ok(LogicVec::new(0))
                } else {
                    let total_width = regs.len() * 64;
                    let mut bits = Vec::with_capacity(total_width);
                    for &rid in &regs {
                        let id_vec = LogicVec::from_u64(rid, 64);
                        bits.extend(id_vec.bits.iter());
                    }
                    Ok(LogicVec {
                        width: total_width,
                        bits,
                    })
                }
            }
            "get_base_address" => {
                let addr = self
                    .uvm_reg_block_data
                    .get(&obj_id)
                    .map(|bd| bd.base_address)
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(addr, 64))
            }
            "set_base_address" => {
                let addr = args.first().map(|a| a.to_u64()).unwrap_or(0);
                if let Some(bd) = self.uvm_reg_block_data.get_mut(&obj_id) {
                    bd.base_address = addr;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            _ => self.execute_uvm_component_method(obj_id, method, args),
        }
    }

    pub(crate) fn execute_uvm_reg_map_method(
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
                self.uvm_reg_map_data.entry(obj_id).or_default();
                Ok(LogicVec::from_u64(1, 1))
            }
            "add_reg" => {
                let reg_id = args.first().map(|a| a.to_u64() as ObjId).unwrap_or(0);
                let offset = args.get(1).map(|a| a.to_u64()).unwrap_or(0);
                if let Some(md) = self.uvm_reg_map_data.get_mut(&obj_id) {
                    md.regs_by_offset.insert(offset, reg_id);
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_reg_by_offset" => {
                let offset = args.first().map(|a| a.to_u64()).unwrap_or(0);
                let reg_id = self
                    .uvm_reg_map_data
                    .get(&obj_id)
                    .and_then(|md| md.regs_by_offset.get(&offset).copied())
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(reg_id as u64, 64))
            }
            "set_base_addr" => {
                let addr = args.first().map(|a| a.to_u64()).unwrap_or(0);
                if let Some(md) = self.uvm_reg_map_data.get_mut(&obj_id) {
                    md.base_address = addr;
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            "get_base_addr" => {
                let addr = self
                    .uvm_reg_map_data
                    .get(&obj_id)
                    .map(|md| md.base_address)
                    .unwrap_or(0);
                Ok(LogicVec::from_u64(addr, 64))
            }
            _ => self.execute_uvm_object_method(obj_id, method, args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::compile_str;

    #[test]
    fn test_uvm_reg_field_set_get() {
        let design = compile_str("module top; endmodule").unwrap();
        let mut engine = SimulationEngine::new(design, 100);
        let obj_id = 1;

        // Create field
        let args = vec![string_to_logicvec("my_field")];
        assert!(engine
            .execute_uvm_reg_field_method(obj_id, "new", &args)
            .is_ok());

        // Set value and get it back
        let val = LogicVec::from_u64(0xAB, 8);
        assert!(engine
            .execute_uvm_reg_field_method(obj_id, "set", &[val.clone()])
            .is_ok());
        let got = engine.execute_uvm_reg_field_method(obj_id, "get", &[]).unwrap();
        assert_eq!(got, val);

        // Set desired and get desired
        let desired = LogicVec::from_u64(0xCD, 8);
        assert!(engine
            .execute_uvm_reg_field_method(obj_id, "set_desired", &[desired.clone()])
            .is_ok());
        let got_desired = engine
            .execute_uvm_reg_field_method(obj_id, "get_desired", &[])
            .unwrap();
        assert_eq!(got_desired, desired);
    }

    #[test]
    fn test_uvm_reg_field_reset() {
        let design = compile_str("module top; endmodule").unwrap();
        let mut engine = SimulationEngine::new(design, 100);
        let obj_id = 1;

        let args = vec![string_to_logicvec("my_field")];
        assert!(engine
            .execute_uvm_reg_field_method(obj_id, "new", &args)
            .is_ok());

        // Set a non-zero value
        let val = LogicVec::from_u64(0xFF, 8);
        assert!(engine
            .execute_uvm_reg_field_method(obj_id, "set", &[val.clone()])
            .is_ok());
        assert_eq!(
            engine.execute_uvm_reg_field_method(obj_id, "get", &[]).unwrap(),
            val
        );

        // Reset should clear the value
        assert!(engine
            .execute_uvm_reg_field_method(obj_id, "reset", &[])
            .is_ok());
        let after_reset = engine.execute_uvm_reg_field_method(obj_id, "get", &[]).unwrap();
        // After reset, value should differ from the pre-reset value
        assert_ne!(after_reset, val);
    }
}
