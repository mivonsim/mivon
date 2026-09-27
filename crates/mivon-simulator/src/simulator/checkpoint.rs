//! Checkpoint — save/restore simulation state for checkpoint/restart.
//!
//! Serializes runtime state (signals, RNG, UVM data, process map, coverage)
//! using a manual binary format. No serde dependency needed.
//!
//! Format: little-endian binary with length-prefixed arrays.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::Path;

use mivon_ir::*;

use crate::simulator::types::*;
use mivon_core::Symbol;

// ─── Manual Binary I/O Helpers ───

fn write_u64<W: Write>(w: &mut W, val: u64) -> io::Result<()> {
    w.write_all(&val.to_le_bytes())
}

fn read_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn write_u32<W: Write>(w: &mut W, val: u32) -> io::Result<()> {
    w.write_all(&val.to_le_bytes())
}

fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn write_usize<W: Write>(w: &mut W, val: usize) -> io::Result<()> {
    write_u64(w, val as u64)
}

fn read_usize<R: Read>(r: &mut R) -> io::Result<usize> {
    read_u64(r).map(|v| v as usize)
}

fn write_bool<W: Write>(w: &mut W, val: bool) -> io::Result<()> {
    w.write_all(&[if val { 1u8 } else { 0u8 }])
}

fn read_bool<R: Read>(r: &mut R) -> io::Result<bool> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)?;
    Ok(buf[0] != 0)
}

fn write_str<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    let bytes = s.as_bytes();
    write_usize(w, bytes.len())?;
    w.write_all(bytes)
}

fn read_str<R: Read>(r: &mut R) -> io::Result<String> {
    let len = read_usize(r)?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ─── LogicVal / LogicVec Serialization ───

fn write_logic_val<W: Write>(w: &mut W, val: &LogicVal) -> io::Result<()> {
    let byte = match val {
        LogicVal::Zero => 0u8,
        LogicVal::One => 1u8,
        LogicVal::X => 2u8,
        LogicVal::Z => 3u8,
    };
    w.write_all(&[byte])
}

fn read_logic_val<R: Read>(r: &mut R) -> io::Result<LogicVal> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)?;
    match buf[0] {
        0 => Ok(LogicVal::Zero),
        1 => Ok(LogicVal::One),
        2 => Ok(LogicVal::X),
        3 => Ok(LogicVal::Z),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid LogicVal byte",
        )),
    }
}

fn write_logic_vec<W: Write>(w: &mut W, lv: &LogicVec) -> io::Result<()> {
    write_usize(w, lv.width)?;
    write_usize(w, lv.bits.len())?;
    for b in &lv.bits {
        write_logic_val(w, b)?;
    }
    Ok(())
}

fn read_logic_vec<R: Read>(r: &mut R) -> io::Result<LogicVec> {
    let width = read_usize(r)?;
    let len = read_usize(r)?;
    let mut bits = Vec::with_capacity(len);
    for _ in 0..len {
        bits.push(read_logic_val(r)?);
    }
    Ok(LogicVec { bits, width })
}

// ─── Serialization for HashMap/Key types ───

fn write_symbol<W: Write>(w: &mut W, sym: &Symbol) -> io::Result<()> {
    write_str(w, sym.as_str())
}

fn read_symbol<R: Read>(r: &mut R) -> io::Result<Symbol> {
    let s = read_str(r)?;
    Ok(Symbol::intern(&s))
}

fn write_obj_id<W: Write>(w: &mut W, id: &ObjId) -> io::Result<()> {
    write_usize(w, *id)
}

fn read_obj_id<R: Read>(r: &mut R) -> io::Result<ObjId> {
    read_usize(r)
}

fn write_tuple_hashmap<
    K1: std::fmt::Display + std::hash::Hash + Eq,
    K2: std::fmt::Display + std::hash::Hash + Eq,
    V,
    W: Write,
>(
    w: &mut W,
    map: &HashMap<(K1, K2), V>,
    write_val: &impl Fn(&mut W, &V) -> io::Result<()>,
) -> io::Result<()> {
    write_usize(w, map.len())?;
    for ((k1, k2), v) in map {
        write_str(w, &k1.to_string())?;
        write_str(w, &k2.to_string())?;
        write_val(w, v)?;
    }
    Ok(())
}

fn read_tuple_hashmap<
    K1: std::str::FromStr + std::hash::Hash + Eq,
    K2: std::str::FromStr + std::hash::Hash + Eq,
    V,
    R: Read,
>(
    r: &mut R,
    read_val: &impl Fn(&mut R) -> io::Result<V>,
) -> io::Result<HashMap<(K1, K2), V>> {
    let len = read_usize(r)?;
    let mut map = HashMap::with_capacity(len);
    for _ in 0..len {
        let k1_str = read_str(r)?;
        let k2_str = read_str(r)?;
        let v = read_val(r)?;
        let k1: K1 = k1_str
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "parse error"))?;
        let k2: K2 = k2_str
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "parse error"))?;
        map.insert((k1, k2), v);
    }
    Ok(map)
}

fn write_usize_hashmap<V, W: Write>(
    w: &mut W,
    map: &HashMap<usize, V>,
    write_val: &impl Fn(&mut W, &V) -> io::Result<()>,
) -> io::Result<()> {
    write_usize(w, map.len())?;
    for (k, v) in map {
        write_usize(w, *k)?;
        write_val(w, v)?;
    }
    Ok(())
}

fn read_usize_hashmap<V, R: Read>(
    r: &mut R,
    read_val: &impl Fn(&mut R) -> io::Result<V>,
) -> io::Result<HashMap<usize, V>> {
    let len = read_usize(r)?;
    let mut map = HashMap::with_capacity(len);
    for _ in 0..len {
        let k = read_usize(r)?;
        let v = read_val(r)?;
        map.insert(k, v);
    }
    Ok(map)
}

fn write_symbol_hashmap<V, W: Write>(
    w: &mut W,
    map: &HashMap<Symbol, V>,
    write_val: &impl Fn(&mut W, &V) -> io::Result<()>,
) -> io::Result<()> {
    write_usize(w, map.len())?;
    for (k, v) in map {
        write_symbol(w, k)?;
        write_val(w, v)?;
    }
    Ok(())
}

fn read_symbol_hashmap<V, R: Read>(
    r: &mut R,
    read_val: &impl Fn(&mut R) -> io::Result<V>,
) -> io::Result<HashMap<Symbol, V>> {
    let len = read_usize(r)?;
    let mut map = HashMap::with_capacity(len);
    for _ in 0..len {
        let k = read_symbol(r)?;
        let v = read_val(r)?;
        map.insert(k, v);
    }
    Ok(map)
}

fn write_logicvec_deque<W: Write>(w: &mut W, deque: &VecDeque<(u64, LogicVec)>) -> io::Result<()> {
    write_usize(w, deque.len())?;
    for (time, lv) in deque {
        write_u64(w, *time)?;
        write_logic_vec(w, lv)?;
    }
    Ok(())
}

fn read_logicvec_deque<R: Read>(r: &mut R) -> io::Result<VecDeque<(u64, LogicVec)>> {
    let len = read_usize(r)?;
    let mut deque = VecDeque::with_capacity(len);
    for _ in 0..len {
        let time = read_u64(r)?;
        let lv = read_logic_vec(r)?;
        deque.push_back((time, lv));
    }
    Ok(deque)
}

// ─── ProcessInfo Serialization ───

fn write_process_info<W: Write>(w: &mut W, pi: &ProcessInfo) -> io::Result<()> {
    write_u32(w, pi.status as u32)?;
    // Skip await_continuations (regenerated on restore)
    write_usize(w, 0usize)?;
    Ok(())
}

fn read_process_info<R: Read>(r: &mut R) -> io::Result<ProcessInfo> {
    let status_code = read_u32(r)?;
    let status = match status_code {
        0 => ProcessStatus::Finished,
        1 => ProcessStatus::Running,
        2 => ProcessStatus::Waiting,
        3 => ProcessStatus::Suspended,
        4 => ProcessStatus::Killed,
        _ => ProcessStatus::Finished,
    };
    let await_count = read_usize(r)?;
    let await_continuations = Vec::with_capacity(await_count);
    for _ in 0..await_count {
        // Skip: continuations are vectors of IrStmt (not serializable simply)
        // We'll regenerate them
    }
    Ok(ProcessInfo {
        status,
        await_continuations,
    })
}

// ─── UvmData Serialization ───

// UvmObjectData serialization is handled inline in the checkpoint serialization.

// ─── SimCheckpoint ───

/// Serializable checkpoint of simulation runtime state.
pub struct SimCheckpoint {
    /// Core signal state
    pub signals: Vec<LogicVec>,
    pub next_signals: Vec<LogicVec>,
    pub changed: Vec<bool>,
    pub time: u64,
    pub current_time: u64,
    /// RNG seed (for StdRng::seed_from_u64)
    pub rng_seed: u64,
    /// Process state
    pub process_map: HashMap<ObjId, ProcessInfo>,
    /// UVM data
    pub uvm_object_data: HashMap<ObjId, UvmObjectData>,
    pub uvm_component_data: HashMap<ObjId, UvmComponentData>,
    pub uvm_config_db_data: HashMap<(String, String), LogicVec>,
    pub uvm_resource_db_data: HashMap<(String, String), LogicVec>,
    pub mailbox_queues: HashMap<usize, VecDeque<LogicVec>>,
    pub semaphore_counts: HashMap<usize, u32>,
    /// Cover data
    pub cover_hits: HashMap<Symbol, u64>,
    pub cover_total: HashMap<Symbol, u64>,
    pub cover_bins: HashMap<Symbol, HashMap<Symbol, u64>>,
    /// Signal history (truncated to signal_history_max)
    pub signal_history: HashMap<Symbol, VecDeque<(u64, LogicVec)>>,
    pub signal_history_max: usize,
}

impl SimCheckpoint {
    /// Checkpoint filename extension
    pub const EXT: &'static str = ".mckpt";

    /// Serialize to a writer.
    pub fn serialize<W: Write>(&self, w: &mut W) -> io::Result<()> {
        // Magic + version
        w.write_all(b"MCKPT")?;
        write_u32(w, 1)?; // version 1

        // Signals
        write_usize(w, self.signals.len())?;
        for s in &self.signals {
            write_logic_vec(w, s)?;
        }
        write_usize(w, self.next_signals.len())?;
        for s in &self.next_signals {
            write_logic_vec(w, s)?;
        }
        write_usize(w, self.changed.len())?;
        for c in &self.changed {
            write_bool(w, *c)?;
        }

        // Time
        write_u64(w, self.time)?;
        write_u64(w, self.current_time)?;

        // RNG seed
        write_u64(w, self.rng_seed)?;

        // Process map
        write_usize_hashmap(w, &self.process_map, &write_process_info)?;

        // UVM object data
        write_usize_hashmap(w, &self.uvm_object_data, &|w, d| write_str(w, &d.name))?;

        // UVM component data
        write_usize_hashmap(w, &self.uvm_component_data, &|w, d| {
            write_obj_id(w, &d.parent.unwrap_or(0))?;
            write_usize(w, d.children.len())?;
            for c in &d.children {
                write_obj_id(w, c)?;
            }
            write_u32(w, d.report_verbosity)?;
            Ok(())
        })?;

        // UVM config/resource DB
        write_tuple_hashmap(w, &self.uvm_config_db_data, &|w, val| {
            write_logic_vec(w, val)
        })?;
        write_tuple_hashmap(w, &self.uvm_resource_db_data, &|w, val| {
            write_logic_vec(w, val)
        })?;

        // Mailbox queues
        write_usize_hashmap(w, &self.mailbox_queues, &|w, deque| {
            write_usize(w, deque.len())?;
            for val in deque {
                write_logic_vec(w, val)?;
            }
            Ok(())
        })?;

        // Semaphore counts
        write_usize(w, self.semaphore_counts.len())?;
        for (k, v) in &self.semaphore_counts {
            write_usize(w, *k)?;
            write_u32(w, *v)?;
        }

        // Cover data
        write_symbol_hashmap(w, &self.cover_hits, &|w, v| write_u64(w, *v))?;
        write_symbol_hashmap(w, &self.cover_total, &|w, v| write_u64(w, *v))?;
        write_usize(w, self.cover_bins.len())?;
        for (k, inner) in &self.cover_bins {
            write_symbol(w, k)?;
            write_symbol_hashmap(w, inner, &|w, v| write_u64(w, *v))?;
        }

        // Signal history
        write_usize(w, self.signal_history_max)?;
        write_usize(w, self.signal_history.len())?;
        for (sym, deque) in &self.signal_history {
            write_symbol(w, sym)?;
            write_logicvec_deque(w, deque)?;
        }

        Ok(())
    }

    /// Deserialize from a reader.
    pub fn deserialize<R: Read>(r: &mut R) -> io::Result<Self> {
        // Magic check
        let mut magic = [0u8; 5];
        r.read_exact(&mut magic)?;
        if &magic != b"MCKPT" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad checkpoint magic",
            ));
        }
        let _version = read_u32(r)?;

        // Signals
        let sig_len = read_usize(r)?;
        let mut signals = Vec::with_capacity(sig_len);
        for _ in 0..sig_len {
            signals.push(read_logic_vec(r)?);
        }
        let ns_len = read_usize(r)?;
        let mut next_signals = Vec::with_capacity(ns_len);
        for _ in 0..ns_len {
            next_signals.push(read_logic_vec(r)?);
        }
        let ch_len = read_usize(r)?;
        let mut changed = Vec::with_capacity(ch_len);
        for _ in 0..ch_len {
            changed.push(read_bool(r)?);
        }

        let time = read_u64(r)?;
        let current_time = read_u64(r)?;
        let rng_seed = read_u64(r)?;

        // Process map
        let process_map = read_usize_hashmap(r, &|r| read_process_info(r))?;

        // UVM object data
        let uvm_object_data = read_usize_hashmap(r, &|r| {
            let name = read_str(r)?;
            Ok(UvmObjectData { name })
        })?;

        // UVM component data
        let uvm_component_data = read_usize_hashmap(r, &|r| {
            let parent_id = read_obj_id(r)?;
            let parent = if parent_id == 0 {
                None
            } else {
                Some(parent_id)
            };
            let child_len = read_usize(r)?;
            let mut children = Vec::with_capacity(child_len);
            for _ in 0..child_len {
                children.push(read_obj_id(r)?);
            }
            let report_verbosity = read_u32(r)?;
            Ok(UvmComponentData {
                parent,
                children,
                report_verbosity,
            })
        })?;

        // UVM config/resource DB
        let uvm_config_db_data = read_tuple_hashmap(r, &|r| read_logic_vec(r))?;
        let uvm_resource_db_data = read_tuple_hashmap(r, &|r| read_logic_vec(r))?;

        // Mailbox queues
        let mailbox_queues = read_usize_hashmap(r, &|r| {
            let len = read_usize(r)?;
            let mut deque = VecDeque::with_capacity(len);
            for _ in 0..len {
                deque.push_back(read_logic_vec(r)?);
            }
            Ok(deque)
        })?;

        // Semaphore counts
        let sem_len = read_usize(r)?;
        let mut semaphore_counts = HashMap::with_capacity(sem_len);
        for _ in 0..sem_len {
            let k = read_usize(r)?;
            let v = read_u32(r)?;
            semaphore_counts.insert(k, v);
        }

        // Cover data
        let cover_hits = read_symbol_hashmap(r, &|r| read_u64(r))?;
        let cover_total = read_symbol_hashmap(r, &|r| read_u64(r))?;
        let bins_len = read_usize(r)?;
        let mut cover_bins = HashMap::with_capacity(bins_len);
        for _ in 0..bins_len {
            let k = read_symbol(r)?;
            let inner = read_symbol_hashmap(r, &|r| read_u64(r))?;
            cover_bins.insert(k, inner);
        }

        // Signal history
        let signal_history_max = read_usize(r)?;
        let hist_len = read_usize(r)?;
        let mut signal_history = HashMap::with_capacity(hist_len);
        for _ in 0..hist_len {
            let sym = read_symbol(r)?;
            let deque = read_logicvec_deque(r)?;
            signal_history.insert(sym, deque);
        }

        Ok(SimCheckpoint {
            signals,
            next_signals,
            changed,
            time,
            current_time,
            rng_seed,
            process_map,
            uvm_object_data,
            uvm_component_data,
            uvm_config_db_data,
            uvm_resource_db_data,
            mailbox_queues,
            semaphore_counts,
            cover_hits,
            cover_total,
            cover_bins,
            signal_history,
            signal_history_max,
        })
    }

    /// Save checkpoint to a file.
    pub fn save_to_file(&self, path: &Path) -> io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        self.serialize(&mut writer)?;
        writer.flush()?;
        Ok(())
    }

    /// Load checkpoint from a file.
    pub fn load_from_file(path: &Path) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let mut reader = std::io::BufReader::new(file);
        Self::deserialize(&mut reader)
    }
}

// ─── SimulationEngine save/restore methods ───

impl crate::simulator::engine::SimulationEngine {
    /// Kumpulkan state runtime → `SimCheckpoint` (TANPA IO). Dasar
    /// `save_checkpoint` dan snapshot Direct RTL CPU (`mivon-emu::cpu::rtl`).
    /// Menangkap: sinyal, waktu, RNG, process map, UVM data, coverage,
    /// signal history.
    /// Snapshot seluruh state engine ke `SimCheckpoint` (blok field besar —
    /// dibiarkan via let-and-return demi keterbacaan field list).
    #[allow(clippy::let_and_return)]
    pub fn checkpoint(&self) -> SimCheckpoint {
        // Collect state from SimulationState
        let signals = self.state.signals.clone();
        let next_signals = self.state.next_signals.clone();
        let changed = self.state.changed.clone();

        // RNG: save current seed for StdRng::seed_from_u64
        // We use a deterministic seed derived from current time
        let rng_seed = self
            .current_time
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);

        // UVM data
        let uvm_object_data = self.uvm_object_data.clone();
        let uvm_component_data = self.uvm_component_data.clone();
        let uvm_config_db_data = self.uvm_config_db_data.clone();
        let uvm_resource_db_data = self.uvm_resource_db_data.clone();

        // Mailbox & semaphore
        let mailbox_queues = self.mailbox_queues.clone();
        let semaphore_counts = self.semaphore_counts.clone();

        // Cover data
        let cover_hits = self.cover_hits.clone();
        let cover_total = self.cover_total.clone();
        let cover_bins = self.cover_bins.clone();

        // Signal history (dump memory entries — disk spill file persists separately)
        let signal_history = self.signal_history.dump_memory_entries();
        let signal_history_max = 100_000;

        let checkpoint = SimCheckpoint {
            signals,
            next_signals,
            changed,
            time: self.state.time,
            current_time: self.current_time,
            rng_seed,
            process_map: self.process_map.clone(),
            uvm_object_data,
            uvm_component_data,
            uvm_config_db_data,
            uvm_resource_db_data,
            mailbox_queues,
            semaphore_counts,
            cover_hits,
            cover_total,
            cover_bins,
            signal_history,
            signal_history_max,
        };

        checkpoint
    }

    /// Save simulation state to a checkpoint file (wrapper [`Self::checkpoint`]).
    pub fn save_checkpoint(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        self.checkpoint().save_to_file(path)?;
        Ok(())
    }

    /// Restore dari `SimCheckpoint` (TANPA IO) — dasar `load_checkpoint` dan
    /// restore snapshot Direct RTL CPU (`mivon-emu`). Design (IrDesign) harus
    /// sama — tanpa cross-check. Antrean runtime yang TIDAK ikut checkpoint
    /// (event queue, NBA pending, wait/event control, fork) di-reset bersih.
    pub fn restore_checkpoint(
        &mut self,
        checkpoint: SimCheckpoint,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Restore signal state
        if checkpoint.signals.len() == self.state.signals.len() {
            self.state.signals = checkpoint.signals;
            self.state.next_signals = checkpoint.next_signals;
            self.state.changed = checkpoint.changed;
        } else {
            return Err("checkpoint signal count mismatch".into());
        }

        self.state.time = checkpoint.time;
        self.current_time = checkpoint.current_time;

        // RNG: seed from stored seed
        self.rng = rand::SeedableRng::seed_from_u64(checkpoint.rng_seed);

        // Process map
        self.process_map = checkpoint.process_map;

        // UVM data
        self.uvm_object_data = checkpoint.uvm_object_data;
        self.uvm_component_data = checkpoint.uvm_component_data;
        self.uvm_config_db_data = checkpoint.uvm_config_db_data;
        self.uvm_resource_db_data = checkpoint.uvm_resource_db_data;

        // Mailbox & semaphore
        self.mailbox_queues = checkpoint.mailbox_queues;
        self.semaphore_counts = checkpoint.semaphore_counts;

        // Cover data
        self.cover_hits = checkpoint.cover_hits;
        self.cover_total = checkpoint.cover_total;
        self.cover_bins = checkpoint.cover_bins;

        // Signal history (restore memory entries — disk spill file persists separately)
        self.signal_history
            .restore_memory_entries(checkpoint.signal_history);

        // Reset runtime state that should be fresh after restore
        self.events.clear();
        self.events_base = 0;
        self.nba_pending.clear();
        self.running = true;
        self.paused = false;
        self.control_flow = None;
        self.fork_groups.clear();
        self.fork_free.clear();
        self.reactive_events.clear();
        self.strobe_events.clear();
        self.fstrobe_events.clear();
        self.fmonitor_map.clear();
        self.pending_waits.clear();
        self.pending_events.clear();
        self.pending_ast_events.clear();
        self.pending_wait_orders.clear();
        Ok(())
    }

    /// Restore simulation state from a checkpoint file.
    /// Design (IrDesign) must match — no cross-checking is performed.
    pub fn load_checkpoint(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let checkpoint = SimCheckpoint::load_from_file(path)?;
        self.restore_checkpoint(checkpoint)
    }

    /// `true` bila ada pekerjaan TERTUNDA di engine: event queue terjadwal,
    /// NBA pending, wait/event control, strobe/fork — bukan kondisi idle
    /// antar langkah. Checkpoint TIDAK memuat antrean tsb, jadi snapshot/
    /// restore engine hanya aman saat fungsi ini `false` (pemakaian:
    /// `mivon-emu::cpu::rtl` menolak snapshot di tengah transaksi).
    pub fn has_pending_runtime_events(&self) -> bool {
        !self.events.iter().all(Vec::is_empty)
            || !self.foreign_events.is_empty()
            || !self.nba_pending.is_empty()
            || !self.reactive_events.is_empty()
            || !self.strobe_events.is_empty()
            || !self.fstrobe_events.is_empty()
            || !self.pending_waits.is_empty()
            || !self.pending_events.is_empty()
            || !self.pending_ast_events.is_empty()
            || !self.pending_wait_orders.is_empty()
            || !self.pending_wait_forks.is_empty()
            || !self.fork_groups.is_empty()
            || self.loop_continuation.is_some()
            || self.ast_loop_continuation.is_some()
            || !self.post_loop_tail.is_empty()
            || self.task_suspended
            || self.disable_pending.is_some()
            || self.pending_await_target.is_some()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_basic_roundtrip() {
        // Create a minimal checkpoint
        let ckpt = SimCheckpoint {
            signals: vec![LogicVec::from_u64(42, 8)],
            next_signals: vec![LogicVec::from_u64(42, 8)],
            changed: vec![false],
            time: 0,
            current_time: 0,
            rng_seed: 12345,
            process_map: HashMap::new(),
            uvm_object_data: HashMap::new(),
            uvm_component_data: HashMap::new(),
            uvm_config_db_data: HashMap::new(),
            uvm_resource_db_data: HashMap::new(),
            mailbox_queues: HashMap::new(),
            semaphore_counts: HashMap::new(),
            cover_hits: HashMap::new(),
            cover_total: HashMap::new(),
            cover_bins: HashMap::new(),
            signal_history: HashMap::new(),
            signal_history_max: 100,
        };

        let mut buf = Vec::new();
        ckpt.serialize(&mut buf).unwrap();

        let restored = SimCheckpoint::deserialize(&mut buf.as_slice()).unwrap();

        assert_eq!(restored.signals.len(), 1);
        assert_eq!(restored.signals[0].to_u64(), 42);
        assert_eq!(restored.signals[0].width, 8);
        assert_eq!(restored.time, 0);
        assert_eq!(restored.rng_seed, 12345);
        assert_eq!(restored.signal_history_max, 100);
    }

    #[test]
    fn test_checkpoint_with_uvm_data() {
        let mut uvm_objects = HashMap::new();
        uvm_objects.insert(
            1,
            UvmObjectData {
                name: "uvm_test_top".to_string(),
            },
        );
        uvm_objects.insert(
            2,
            UvmObjectData {
                name: "my_driver".to_string(),
            },
        );

        let mut config_db = HashMap::new();
        config_db.insert(
            ("uvm_test_top".to_string(), "count".to_string()),
            LogicVec::from_u64(100, 32),
        );

        let mut mailboxes = HashMap::new();
        let mut deque = VecDeque::new();
        deque.push_back(LogicVec::from_u64(1, 8));
        deque.push_back(LogicVec::from_u64(2, 8));
        mailboxes.insert(0, deque);

        let ckpt = SimCheckpoint {
            signals: vec![],
            next_signals: vec![],
            changed: vec![],
            time: 100,
            current_time: 100,
            rng_seed: 999,
            process_map: HashMap::new(),
            uvm_object_data: uvm_objects,
            uvm_component_data: HashMap::new(),
            uvm_config_db_data: config_db,
            uvm_resource_db_data: HashMap::new(),
            mailbox_queues: mailboxes,
            semaphore_counts: [(0usize, 1u32)].into(),
            cover_hits: HashMap::new(),
            cover_total: HashMap::new(),
            cover_bins: HashMap::new(),
            signal_history: HashMap::new(),
            signal_history_max: 50,
        };

        let mut buf = Vec::new();
        ckpt.serialize(&mut buf).unwrap();

        let restored = SimCheckpoint::deserialize(&mut buf.as_slice()).unwrap();

        assert_eq!(restored.time, 100);
        assert_eq!(restored.uvm_object_data.len(), 2);
        assert_eq!(restored.uvm_object_data[&1].name, "uvm_test_top");
        assert_eq!(restored.mailbox_queues.len(), 1);
        assert_eq!(restored.mailbox_queues[&0].len(), 2);
        assert_eq!(restored.semaphore_counts[&0], 1);
        assert_eq!(restored.signal_history_max, 50);
    }

    #[test]
    fn test_checkpoint_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mivon_ckpt_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.mckpt");

        let ckpt = SimCheckpoint {
            signals: vec![LogicVec::from_u64(0xFF, 8)],
            next_signals: vec![LogicVec::from_u64(0xFF, 8)],
            changed: vec![false],
            time: 42,
            current_time: 42,
            rng_seed: 777,
            process_map: HashMap::new(),
            uvm_object_data: HashMap::new(),
            uvm_component_data: HashMap::new(),
            uvm_config_db_data: HashMap::new(),
            uvm_resource_db_data: HashMap::new(),
            mailbox_queues: HashMap::new(),
            semaphore_counts: HashMap::new(),
            cover_hits: HashMap::new(),
            cover_total: HashMap::new(),
            cover_bins: HashMap::new(),
            signal_history: HashMap::new(),
            signal_history_max: 100,
        };

        ckpt.save_to_file(&path).unwrap();

        let restored = SimCheckpoint::load_from_file(&path).unwrap();
        assert_eq!(restored.signals[0].to_u64(), 0xFF);
        assert_eq!(restored.time, 42);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_checkpoint_with_cover_data() {
        let mut cover_bins = HashMap::new();
        let mut inner = HashMap::new();
        inner.insert(Symbol::intern("bin_0"), 5u64);
        cover_bins.insert(Symbol::intern("my_coverpoint"), inner);

        let ckpt = SimCheckpoint {
            signals: vec![],
            next_signals: vec![],
            changed: vec![],
            time: 0,
            current_time: 0,
            rng_seed: 0,
            process_map: HashMap::new(),
            uvm_object_data: HashMap::new(),
            uvm_component_data: HashMap::new(),
            uvm_config_db_data: HashMap::new(),
            uvm_resource_db_data: HashMap::new(),
            mailbox_queues: HashMap::new(),
            semaphore_counts: HashMap::new(),
            cover_hits: [(Symbol::intern("line_10"), 3u64)].into(),
            cover_total: [(Symbol::intern("line_10"), 10u64)].into(),
            cover_bins,
            signal_history: HashMap::new(),
            signal_history_max: 100,
        };

        let mut buf = Vec::new();
        ckpt.serialize(&mut buf).unwrap();

        let restored = SimCheckpoint::deserialize(&mut buf.as_slice()).unwrap();
        assert_eq!(restored.cover_hits[&Symbol::intern("line_10")], 3);
        assert_eq!(
            restored.cover_bins[&Symbol::intern("my_coverpoint")][&Symbol::intern("bin_0")],
            5
        );
    }
}
