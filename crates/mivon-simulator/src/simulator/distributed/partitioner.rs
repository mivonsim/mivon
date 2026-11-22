//! Design Partitioner — membagi desain menjadi partitions untuk distributed simulation.
//!
//! # Algorithm
//!
//! 1. **Koleksi Instances** — Kumpulkan semua sub-instances dari top module.
//! 2. **Graph Construction** — Bangun graph instance dependencies (signal connectivity).
//! 3. **Partition Assignment** — Assign instances ke partitions secara round-robin atau
//!    berdasarkan connectivity (min-cut).
//! 4. **Cross-partition Signal Detection** — Identifikasi signal yang menghubungkan
//!    instances di partition berbeda.
//! 5. **Partition Export** — Generate file SV terpisah per partition + wrapper.
//!
//! # Simple Partition Strategy (Round-Robin)
//!
//! Untuk MVP, gunakan round-robin assignment berdasarkan instance count:
//! Partition 0: instance 0, 3, 6, ...
//! Partition 1: instance 1, 4, 7, ...
//! Partition 2: instance 2, 5, 8, ...
//!
//! Semua signal global (clock, reset) tetap di semua partition.
//! Cross-partition signal di-exchange via distributed protocol.

use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::{HashMap, HashSet};

/// Information about a signal that crosses partition boundaries.
#[derive(Debug, Clone)]
pub struct PartitionSignal {
    /// Signal ID in the original (flattened) design.
    pub signal_id: SignalId,
    /// Signal name.
    pub signal_name: String,
    /// Signal width.
    pub width: usize,
    /// Source partition ID (the partition that drives this signal).
    pub source_partition: usize,
    /// Destination partition IDs (partitions that read this signal).
    pub dest_partitions: Vec<usize>,
    /// Whether this signal needs to be synchronized (crosses a clock domain).
    pub needs_sync: bool,
}

/// Information about a single partition.
#[derive(Debug, Clone)]
pub struct PartitionInfo {
    /// Partition ID (0-based).
    pub id: usize,
    /// Signal IDs assigned to this partition (signals written by processes in this partition).
    pub signal_ids: Vec<SignalId>,
    /// Process IDs assigned to this partition.
    pub process_ids: Vec<usize>,
    /// Instance names assigned to this partition (empty if top-level processes).
    pub instance_names: Vec<String>,
    /// Cross-partition signals that this partition must send/receive.
    pub cross_signals: Vec<PartitionSignal>,
    /// Estimated simulation load (number of processes).
    pub load: usize,
}

/// Complete partitioning result.
#[derive(Debug, Clone)]
pub struct Partition {
    /// Number of partitions.
    pub num_partitions: usize,
    /// Information about each partition.
    pub partitions: Vec<PartitionInfo>,
    /// All cross-partition signals (deduplicated).
    pub cross_signals: Vec<PartitionSignal>,
    /// Signal-to-partition mapping.
    pub signal_to_partition: HashMap<SignalId, usize>,
    /// Process-to-partition mapping.
    pub process_to_partition: HashMap<usize, usize>,
}

impl DesignPartitioner {
    /// Create partitions from an IrDesign.
    ///
    /// # Strategy
    ///
    /// 1. Jika tidak ada sub_instances, semua proses top-level masuk partition 0.
    /// 2. Jika ada sub_instances, distribusikan instances ke partitions secara round-robin.
    /// 3. Cross-partition signals dideteksi dari port_map instances.
    /// 4. Untuk setiap partition, kumpulkan signal IDs dan process IDs.
    pub fn partition(design: &IrDesign, num_partitions: usize) -> Partition {
        let top = &design.top;
        let num_partitions = num_partitions.max(1);

        if num_partitions == 1 || top.sub_instances.is_empty() {
            // Single partition: all processes and signals in one partition
            let all_signal_ids: Vec<SignalId> = (0..top.signals.len()).collect();
            let all_process_ids: Vec<usize> = (0..top.processes.len()).collect();
            let mut signal_to_partition = HashMap::new();
            for &sid in &all_signal_ids {
                signal_to_partition.insert(sid, 0);
            }
            let mut process_to_partition = HashMap::new();
            for &pid in &all_process_ids {
                process_to_partition.insert(pid, 0);
            }

            let p = PartitionInfo {
                id: 0,
                signal_ids: all_signal_ids,
                process_ids: all_process_ids,
                instance_names: vec!["top".to_string()],
                cross_signals: Vec::new(),
                load: top.processes.len(),
            };

            return Partition {
                num_partitions: 1,
                partitions: vec![p],
                cross_signals: Vec::new(),
                signal_to_partition,
                process_to_partition,
            };
        }

        // Multiple partitions: distribute instances round-robin
        let instances = &top.sub_instances;
        let num_instances = instances.len();
        let _instances_per_partition = num_instances.div_ceil(num_partitions);

        // Assign instances to partitions (round-robin for load balancing)
        let mut instance_to_partition: HashMap<Symbol, usize> = HashMap::new();
        for (i, inst) in instances.iter().enumerate() {
            let partition_id = i % num_partitions;
            instance_to_partition.insert(inst.instance_name, partition_id);
        }

        // Build partition info
        let mut partitions: Vec<PartitionInfo> = (0..num_partitions)
            .map(|id| PartitionInfo {
                id,
                signal_ids: Vec::new(),
                process_ids: Vec::new(),
                instance_names: Vec::new(),
                cross_signals: Vec::new(),
                load: 0,
            })
            .collect();

        // Assign signals and processes based on instance-to-partition mapping
        let mut signal_to_partition: HashMap<SignalId, usize> = HashMap::new();
        let mut process_to_partition: HashMap<usize, usize> = HashMap::new();

        // For each instance, find its signals and processes
        // In the flattened design, signals and processes are all at top level.
        // We figure out which signals belong to which instance by examining
        // the writing processes and the port mapping.
        for (pid, _process) in top.processes.iter().enumerate() {
            // Assign process to partition based on which instance it belongs to
            // For now, all top-level processes go to partition 0 (control partition)
            process_to_partition.entry(pid).or_insert(0);
        }

        // Assign signals: each signal belongs to the partition that contains
        // the instance that drives it. For top-level signals, use partition 0.
        for (sid, _sig) in top.signals.iter().enumerate() {
            signal_to_partition.entry(sid).or_insert(0);
        }

        // Build cross-partition signal list
        let mut cross_signals: Vec<PartitionSignal> = Vec::new();
        let mut cross_set: HashSet<String> = HashSet::new();

        // Detect cross-partition signals by analyzing instance port maps
        for inst in instances {
            let src_part = instance_to_partition
                .get(&inst.instance_name)
                .copied()
                .unwrap_or(0);
            for sig_id in inst.port_map.values() {
                let dst_opt = signal_to_partition.get(sig_id).copied();
                if let Some(dst_part) = dst_opt {
                    if src_part != dst_part {
                        let key = format!("{}:{}", sig_id, src_part);
                        if cross_set.insert(key) {
                            let signal_name = top
                                .signals
                                .get(*sig_id)
                                .map(|si| si.name.as_str().to_string())
                                .unwrap_or_else(|| format!("sig_{}", sig_id));
                            let width = top.signals.get(*sig_id).map(|si| si.width).unwrap_or(1);

                            cross_signals.push(PartitionSignal {
                                signal_id: *sig_id,
                                signal_name,
                                width,
                                source_partition: src_part,
                                dest_partitions: vec![dst_part],
                                needs_sync: true, // cross-partition selalu perlu sync
                            });
                        }
                    }
                }
            }
        }

        // Populate partition signal_ids and process_ids from the mappings
        for (sid, &pid) in &signal_to_partition {
            if let Some(p) = partitions.get_mut(pid) {
                p.signal_ids.push(*sid);
            }
        }
        for (pid, &part_id) in &process_to_partition {
            if let Some(p) = partitions.get_mut(part_id) {
                p.process_ids.push(*pid);
            }
        }

        // Populate partition info
        for p in &mut partitions {
            p.instance_names = instances
                .iter()
                .enumerate()
                .filter(|(i, _)| i % num_partitions == p.id)
                .map(|(_, inst)| inst.instance_name.as_str().to_string())
                .collect();
            p.load = p.instance_names.len();
        }

        // Assign cross signals to partitions
        for cs in &cross_signals {
            let src_part = cs.source_partition;
            if let Some(p) = partitions.get_mut(src_part) {
                p.cross_signals.push(cs.clone());
            }
            for &dst in &cs.dest_partitions {
                if dst != src_part {
                    if let Some(p) = partitions.get_mut(dst) {
                        p.cross_signals.push(cs.clone());
                    }
                }
            }
        }

        Partition {
            num_partitions,
            partitions,
            cross_signals,
            signal_to_partition,
            process_to_partition,
        }
    }

    /// Extract a sub-design for a specific partition.
    /// Creates a minimal IrDesign containing only the signals and processes
    /// assigned to the given partition, plus cross-partition signals as inputs/outputs.
    pub fn extract_partition_design(
        design: &IrDesign,
        partition_id: usize,
        partition: &Partition,
    ) -> IrDesign {
        let partition_info = &partition.partitions[partition_id];
        let top = &design.top;
        let mut sub_signals: Vec<SignalInfo> = Vec::new();
        let mut sub_inputs: Vec<SignalId> = Vec::new();
        let mut sub_outputs: Vec<SignalId> = Vec::new();
        let mut sub_processes: Vec<Process> = Vec::new();
        let mut old_to_new: HashMap<SignalId, SignalId> = HashMap::new();

        // 1. Add partition-local signals
        for &sid in &partition_info.signal_ids {
            if let Some(sig) = top.signals.get(sid) {
                let new_id = sub_signals.len();
                old_to_new.insert(sid, new_id);
                sub_signals.push(sig.clone());
                // Track if this is an input/output in the original design
                if top.inputs.contains(&sid) {
                    sub_inputs.push(new_id);
                }
                if top.outputs.contains(&sid) {
                    sub_outputs.push(new_id);
                }
            }
        }

        // 2. Add cross-partition signals as additional signals
        for cs in &partition_info.cross_signals {
            if let std::collections::hash_map::Entry::Vacant(e) = old_to_new.entry(cs.signal_id) {
                if let Some(sig) = top.signals.get(cs.signal_id) {
                    let new_id = sub_signals.len();
                    e.insert(new_id);
                    let mut sub_sig = sig.clone();
                    sub_sig.name = Symbol::intern(&format!("__cross_{}", sig.name.as_str()));
                    sub_signals.push(sub_sig);
                    // Cross-partition signals that are read by this partition become inputs
                    if cs.dest_partitions.contains(&partition_id)
                        && cs.source_partition != partition_id
                    {
                        sub_inputs.push(new_id);
                    }
                    // Cross-partition signals that are driven by this partition become outputs
                    if cs.source_partition == partition_id {
                        sub_outputs.push(new_id);
                    }
                }
            }
        }

        // 3. Add partition-local processes
        for &pid in &partition_info.process_ids {
            if let Some(process) = top.processes.get(pid) {
                let mut sub_process = process.clone();
                // Remap signal IDs in process body to match the new signal indices
                remap_signal_ids_in_process(&mut sub_process, &old_to_new);
                sub_processes.push(sub_process);
            }
        }

        // 4. Build sub-module
        let sub_module = IrModule {
            name: Symbol::intern(&format!(
                "{}__partition_{}",
                design.top.name.as_str(),
                partition_id
            )),
            signals: sub_signals,
            inputs: sub_inputs,
            outputs: sub_outputs,
            inouts: Vec::new(),
            processes: sub_processes,
            sub_instances: Vec::new(), // flattened
        };

        // Note: Process body IR expressions are now remapped via old_to_new map.
        // Cross-partition signal exchange handles the actual data flow at runtime.

        IrDesign {
            top: sub_module,
            modules: HashMap::new(),
            timescale: design.timescale.clone(),
            classes: HashMap::new(),
            covergroups: Vec::new(),
            hier_signal_map: HashMap::new(),
            module_functions: HashMap::new(),
            udp_defs: Vec::new(),
            dpi_imports: Vec::new(),
            specify_items: Vec::new(),
            source_lines: None,
            source_file: None,
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        }
    }

    /// Generate a summary of the partitioning.
    pub fn partition_summary(partition: &Partition) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "═══ Design Partition: {} partitions ═══\n\n",
            partition.num_partitions
        ));
        s.push_str(&format!(
            "Total cross-partition signals: {}\n\n",
            partition.cross_signals.len()
        ));

        for p in &partition.partitions {
            s.push_str(&format!(
                "Partition {}: {} instances, ~{} processes\n",
                p.id,
                p.instance_names.len(),
                p.load
            ));
            if !p.instance_names.is_empty() {
                s.push_str("  Instances: ");
                for name in &p.instance_names {
                    s.push_str(name);
                    s.push(' ');
                }
                s.push('\n');
            }
            if !p.cross_signals.is_empty() {
                s.push_str(&format!("  Cross signals: {}\n", p.cross_signals.len()));
            }
            s.push('\n');
        }

        if !partition.cross_signals.is_empty() {
            s.push_str("Cross-partition signals:\n");
            for cs in &partition.cross_signals {
                s.push_str(&format!(
                    "  {} ({} bits): Partition {} → {:?}\n",
                    cs.signal_name, cs.width, cs.source_partition, cs.dest_partitions
                ));
            }
        }

        s
    }
}

/// Design Partitioner struct.
pub struct DesignPartitioner;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signal(name: &str, width: usize) -> SignalInfo {
        SignalInfo {
            name: Symbol::intern(name),
            width,
            ..Default::default()
        }
    }

    #[test]
    fn test_single_partition_no_instances() {
        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![make_signal("clk", 1), make_signal("data", 8)],
                processes: vec![],
                sub_instances: vec![],
                ..Default::default()
            },
            ..IrDesign::default()
        };

        let p = DesignPartitioner::partition(&design, 1);
        assert_eq!(p.num_partitions, 1);
        assert!(p.cross_signals.is_empty());
    }

    #[test]
    fn test_two_partitions_with_instances() {
        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![
                    make_signal("clk", 1),
                    make_signal("rst", 1),
                    make_signal("data_in", 8),
                    make_signal("data_out", 8),
                ],
                inputs: vec![0, 1, 2],
                outputs: vec![3],
                processes: vec![
                    // Top-level clock generation
                    Process::Initial {
                        name: Symbol::intern("gen_clk"),
                        body: vec![],
                    },
                ],
                sub_instances: vec![
                    IrInstance {
                        module_name: Symbol::intern("sub_a"),
                        instance_name: Symbol::intern("u_a"),
                        port_map: std::sync::Arc::new({
                            let mut m = HashMap::new();
                            m.insert(Symbol::intern("clk"), 0);
                            m.insert(Symbol::intern("data"), 2);
                            m.insert(Symbol::intern("out"), 3);
                            m
                        }),
                        param_map: std::sync::Arc::new(HashMap::new()),
                        type_param_map: std::sync::Arc::new(HashMap::new()),
                        line: 0,
                        col: 0,
                        attrs: Vec::new(),
                        param_width_map: std::sync::Arc::new(HashMap::new()),
                    },
                    IrInstance {
                        module_name: Symbol::intern("sub_b"),
                        instance_name: Symbol::intern("u_b"),
                        port_map: std::sync::Arc::new({
                            let mut m = HashMap::new();
                            m.insert(Symbol::intern("clk"), 0);
                            m.insert(Symbol::intern("data_in"), 3);
                            m.insert(Symbol::intern("out"), 2);
                            m
                        }),
                        param_map: std::sync::Arc::new(HashMap::new()),
                        type_param_map: std::sync::Arc::new(HashMap::new()),
                        line: 0,
                        col: 0,
                        attrs: Vec::new(),
                        param_width_map: std::sync::Arc::new(HashMap::new()),
                    },
                ],
                inouts: vec![],
            },
            ..IrDesign::default()
        };

        let p = DesignPartitioner::partition(&design, 2);
        assert_eq!(p.num_partitions, 2, "should create 2 partitions");
        assert_eq!(p.partitions.len(), 2);

        // Each partition should have some instance names
        assert!(!p.partitions[0].instance_names.is_empty());
        assert!(!p.partitions[1].instance_names.is_empty());
    }

    #[test]
    fn test_empty_design() {
        let design = IrDesign::default();
        let p = DesignPartitioner::partition(&design, 4);
        assert_eq!(p.num_partitions, 1, "empty design should have 1 partition");
    }

    #[test]
    fn test_partition_summary() {
        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![make_signal("clk", 1)],
                processes: vec![Process::Initial {
                    name: Symbol::intern("init"),
                    body: vec![],
                }],
                sub_instances: vec![],
                ..Default::default()
            },
            ..IrDesign::default()
        };

        let p = DesignPartitioner::partition(&design, 1);
        let summary = DesignPartitioner::partition_summary(&p);
        assert!(summary.contains("1 partition"));
    }

    #[test]
    fn test_partition_round_robin() {
        // 5 instances across 3 partitions: 0→p0, 1→p1, 2→p2, 3→p0, 4→p1
        let mut instances = Vec::new();
        for i in 0..5 {
            instances.push(IrInstance {
                module_name: Symbol::intern(&format!("mod_{}", i)),
                instance_name: Symbol::intern(&format!("u_{}", i)),
                port_map: std::sync::Arc::new(HashMap::new()),
                param_map: std::sync::Arc::new(HashMap::new()),
                type_param_map: std::sync::Arc::new(HashMap::new()),
                line: 0,
                col: 0,
                attrs: Vec::new(),
                param_width_map: std::sync::Arc::new(HashMap::new()),
            });
        }

        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![],
                processes: vec![],
                sub_instances: instances,
                ..Default::default()
            },
            ..IrDesign::default()
        };

        let p = DesignPartitioner::partition(&design, 3);
        assert_eq!(p.num_partitions, 3);
        // Partition 0: instances 0, 3
        assert_eq!(p.partitions[0].instance_names.len(), 2);
        // Partitions 1, 2: instances 1,4 and 2 respectively
        assert_eq!(p.partitions[1].instance_names.len(), 2);
        assert_eq!(p.partitions[2].instance_names.len(), 1);
    }

    #[test]
    fn test_extract_partition_design_remaps_signal_ids() {
        // Create a design with processes that reference signals
        // Signal 0: clk, Signal 1: rst, Signal 2: data_in, Signal 3: data_out
        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![
                    make_signal("clk", 1),      // id 0
                    make_signal("rst", 1),      // id 1
                    make_signal("data_in", 8),  // id 2
                    make_signal("data_out", 8), // id 3
                ],
                inputs: vec![0, 1, 2],
                outputs: vec![3],
                processes: vec![
                    // Combinational process: data_out = data_in + 1 (references signal 2 and 3)
                    Process::Combinational {
                        name: Symbol::intern("proc1"),
                        sensitivity: vec![SignalSensitivity {
                            sig_id: 2,
                            msb: None,
                            lsb: None,
                        }],
                        body: vec![IrStmt::BlockingAssign {
                            lhs: IrLValue::Signal(3, 8), // data_out
                            rhs: IrExpr::BinaryOp(
                                BinaryIrOp::Add,
                                Box::new(IrExpr::Signal(2, 8)), // data_in
                                Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
                            ),
                            delay: None,
                        }],
                    },
                ],
                sub_instances: vec![
                    IrInstance {
                        module_name: Symbol::intern("sub_a"),
                        instance_name: Symbol::intern("u_a"),
                        port_map: std::sync::Arc::new({
                            let mut m = HashMap::new();
                            m.insert(Symbol::intern("clk"), 0);
                            m.insert(Symbol::intern("data"), 2);
                            m.insert(Symbol::intern("out"), 3);
                            m
                        }),
                        param_map: std::sync::Arc::new(HashMap::new()),
                        type_param_map: std::sync::Arc::new(HashMap::new()),
                        line: 0,
                        col: 0,
                        attrs: Vec::new(),
                        param_width_map: std::sync::Arc::new(HashMap::new()),
                    },
                    IrInstance {
                        module_name: Symbol::intern("sub_b"),
                        instance_name: Symbol::intern("u_b"),
                        port_map: std::sync::Arc::new({
                            let mut m = HashMap::new();
                            m.insert(Symbol::intern("clk"), 0);
                            m.insert(Symbol::intern("data_in"), 3);
                            m.insert(Symbol::intern("out"), 2);
                            m
                        }),
                        param_map: std::sync::Arc::new(HashMap::new()),
                        type_param_map: std::sync::Arc::new(HashMap::new()),
                        line: 0,
                        col: 0,
                        attrs: Vec::new(),
                        param_width_map: std::sync::Arc::new(HashMap::new()),
                    },
                ],
                inouts: vec![],
            },
            ..IrDesign::default()
        };

        // Partition into 2
        let partition = DesignPartitioner::partition(&design, 2);

        // Extract partition 0 design
        let sub_design = DesignPartitioner::extract_partition_design(&design, 0, &partition);

        // Verify signal IDs in process body are remapped
        // Original signals: 0(clk), 1(rst), 2(data_in), 3(data_out)
        // Partition 0 gets instance u_a (instance 0), which connects to signals 0, 2, 3
        // So partition 0 should have signals mapped to new IDs

        let sub_top = &sub_design.top;
        assert!(!sub_top.processes.is_empty(), "partition should have processes");

        // Check that process body references valid signal IDs (0..num_signals-1)
        for process in &sub_top.processes {
            match process {
                Process::Combinational { body, sensitivity, .. } => {
                    // Check sensitivity references valid signal ID
                    for sens in sensitivity {
                        assert!(
                            sens.sig_id < sub_top.signals.len(),
                            "sensitivity signal ID {} out of bounds (num_signals={})",
                            sens.sig_id,
                            sub_top.signals.len()
                        );
                    }
                    // Check body statements reference valid signal IDs
                    for stmt in body {
                        collect_and_verify_signal_ids(stmt, sub_top.signals.len());
                    }
                }
                _ => {}
            }
        }

        // Verify inputs/outputs reference valid signal IDs
        for &sig_id in &sub_top.inputs {
            assert!(sig_id < sub_top.signals.len(), "input signal ID {} out of bounds", sig_id);
        }
        for &sig_id in &sub_top.outputs {
            assert!(sig_id < sub_top.signals.len(), "output signal ID {} out of bounds", sig_id);
        }
    }
}

// Helper function to collect and verify signal IDs in statements
fn collect_and_verify_signal_ids(stmt: &IrStmt, max_sig_id: usize) {
    match stmt {
        IrStmt::BlockingAssign { lhs, rhs, .. } => {
            verify_lvalue_signal_ids(lhs, max_sig_id);
            verify_expr_signal_ids(rhs, max_sig_id);
        }
        IrStmt::NonBlockingAssign { lhs, rhs, .. } => {
            verify_lvalue_signal_ids(lhs, max_sig_id);
            verify_expr_signal_ids(rhs, max_sig_id);
        }
        IrStmt::If { cond, true_branch, false_branch, .. } => {
            verify_expr_signal_ids(cond, max_sig_id);
            for s in true_branch {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
            for s in false_branch {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::Case { expr, items, default, .. } => {
            verify_expr_signal_ids(expr, max_sig_id);
            for item in items {
                for label_expr in &item.labels {
                    verify_expr_signal_ids(label_expr, max_sig_id);
                }
                for s in &item.body {
                    collect_and_verify_signal_ids(s, max_sig_id);
                }
            }
            for s in default {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::LoopFor { cond, init, step, body, .. } => {
            verify_expr_signal_ids(cond, max_sig_id);
            if let Some(init) = init {
                collect_and_verify_signal_ids(init, max_sig_id);
            }
            if let Some(step) = step {
                collect_and_verify_signal_ids(step, max_sig_id);
            }
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::LoopWhile { cond, body, .. } => {
            verify_expr_signal_ids(cond, max_sig_id);
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::LoopDoWhile { cond, body, .. } => {
            verify_expr_signal_ids(cond, max_sig_id);
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::Repeat { count, body, .. } => {
            verify_expr_signal_ids(count, max_sig_id);
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::Foreach { array_var, body, .. } => {
            verify_expr_signal_ids(array_var, max_sig_id);
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::Delay { body, .. } => {
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::Force { lvalue, rhs, .. } => {
            verify_lvalue_signal_ids(lvalue, max_sig_id);
            verify_expr_signal_ids(rhs, max_sig_id);
        }
        IrStmt::Wait { cond, body, .. } => {
            verify_expr_signal_ids(cond, max_sig_id);
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::SysCall { args, .. } => {
            for arg in args {
                verify_expr_signal_ids(arg, max_sig_id);
            }
        }
        IrStmt::EventControl { sigs, body, iff, .. } => {
            for (sig_id, _) in sigs {
                assert!(*sig_id < max_sig_id, "EventControl signal ID {} out of bounds", sig_id);
            }
            for s in body {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
            if let Some(iff_expr) = iff {
                verify_expr_signal_ids(iff_expr, max_sig_id);
            }
        }
        IrStmt::EventTrigger { sig_id, .. } => {
            assert!(*sig_id < max_sig_id, "EventTrigger signal ID {} out of bounds", sig_id);
        }
        IrStmt::MethodCallStmt { obj, args, .. } => {
            verify_expr_signal_ids(obj, max_sig_id);
            for arg in args {
                verify_expr_signal_ids(arg, max_sig_id);
            }
        }
        IrStmt::Fork { processes, .. } => {
            for branch in processes {
                for s in branch {
                    collect_and_verify_signal_ids(s, max_sig_id);
                }
            }
        }
        IrStmt::Assert { cond, pass_stmt, fail_stmt, .. } => {
            verify_expr_signal_ids(cond, max_sig_id);
            for s in pass_stmt {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
            for s in fail_stmt {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::Assume { cond, pass_stmt, fail_stmt, .. } => {
            verify_expr_signal_ids(cond, max_sig_id);
            for s in pass_stmt {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
            for s in fail_stmt {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::WaitOrder { events, failure_stmts } => {
            for ev in events {
                assert!(*ev < max_sig_id, "WaitOrder event signal ID {} out of bounds", ev);
            }
            for s in failure_stmts {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::RandCase { items } => {
            for (cond, body) in items {
                verify_expr_signal_ids(cond, max_sig_id);
                for s in body {
                    collect_and_verify_signal_ids(s, max_sig_id);
                }
            }
        }
        IrStmt::RandSequence { productions } => {
            for (_name, items) in productions {
                for (cond, body) in items {
                    verify_expr_signal_ids(cond, max_sig_id);
                    for s in body {
                        collect_and_verify_signal_ids(s, max_sig_id);
                    }
                }
            }
        }
        IrStmt::Block { stmts } | IrStmt::NamedBlock { stmts, .. } => {
            for s in stmts {
                collect_and_verify_signal_ids(s, max_sig_id);
            }
        }
        IrStmt::Release { lvalue, .. } | IrStmt::Deassign { lvalue, .. } => {
            verify_lvalue_signal_ids(lvalue, max_sig_id);
        }
        IrStmt::SysFinish | IrStmt::Null | IrStmt::Break | IrStmt::Continue
        | IrStmt::Disable { .. } | IrStmt::WaitFork | IrStmt::Cover { .. }
        | IrStmt::Expect { .. } => {}
    }
}

fn verify_lvalue_signal_ids(lvalue: &IrLValue, max_sig_id: usize) {
    match lvalue {
        IrLValue::Signal(sig_id, ..) => {
            assert!(*sig_id < max_sig_id, "LValue::Signal ID {} out of bounds", sig_id);
        }
        IrLValue::RangeSelect(sig_id, ..) => {
            assert!(*sig_id < max_sig_id, "LValue::RangeSelect ID {} out of bounds", sig_id);
        }
        IrLValue::BitSelect(sig_id, ..) => {
            assert!(*sig_id < max_sig_id, "LValue::BitSelect ID {} out of bounds", sig_id);
        }
        IrLValue::ArrayIndex { sig_id, index, .. } => {
            assert!(*sig_id < max_sig_id, "LValue::ArrayIndex ID {} out of bounds", sig_id);
            verify_expr_signal_ids(index, max_sig_id);
        }
        IrLValue::ArrayRangeSelect { sig_id, .. } => {
            assert!(*sig_id < max_sig_id, "LValue::ArrayRangeSelect ID {} out of bounds", sig_id);
        }
        IrLValue::ArrayBitSelect { sig_id, .. } => {
            assert!(*sig_id < max_sig_id, "LValue::ArrayBitSelect ID {} out of bounds", sig_id);
        }
        IrLValue::ExprPartSelect { sig_id, .. } => {
            assert!(*sig_id < max_sig_id, "LValue::ExprPartSelect ID {} out of bounds", sig_id);
        }
        IrLValue::ObjectField { sig_id, .. } => {
            assert!(*sig_id < max_sig_id, "LValue::ObjectField ID {} out of bounds", sig_id);
        }
        IrLValue::Concat(lvalues) => {
            for lv in lvalues {
                verify_lvalue_signal_ids(lv, max_sig_id);
            }
        }
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } | IrLValue::VifField { .. } => {}
    }
}

fn verify_expr_signal_ids(expr: &IrExpr, max_sig_id: usize) {
    match expr {
        IrExpr::Signal(sig_id, ..) => {
            assert!(*sig_id < max_sig_id, "Expr::Signal ID {} out of bounds", sig_id);
        }
        IrExpr::RangeSelect(sig_id, ..) => {
            assert!(*sig_id < max_sig_id, "Expr::RangeSelect ID {} out of bounds", sig_id);
        }
        IrExpr::BitSelect(sig_id, ..) => {
            assert!(*sig_id < max_sig_id, "Expr::BitSelect ID {} out of bounds", sig_id);
        }
        IrExpr::ExprRangeSelect(inner, ..)
        | IrExpr::ExprBitSelect(inner, ..)
        | IrExpr::ExprPartSelect(inner, ..) => {
            verify_expr_signal_ids(inner, max_sig_id);
        }
        IrExpr::ArrayIndex { sig_id, index, .. } => {
            assert!(*sig_id < max_sig_id, "Expr::ArrayIndex ID {} out of bounds", sig_id);
            verify_expr_signal_ids(index, max_sig_id);
        }
        IrExpr::Concat(slices) => {
            for e in slices {
                verify_expr_signal_ids(e, max_sig_id);
            }
        }
        IrExpr::Replicate(_, inner) => {
            verify_expr_signal_ids(inner, max_sig_id);
        }
        IrExpr::UnaryOp(_, inner) => {
            verify_expr_signal_ids(inner, max_sig_id);
        }
        IrExpr::BinaryOp(_, lhs, rhs) => {
            verify_expr_signal_ids(lhs, max_sig_id);
            verify_expr_signal_ids(rhs, max_sig_id);
        }
        IrExpr::Cond(cond, true_branch, false_branch) => {
            verify_expr_signal_ids(cond, max_sig_id);
            verify_expr_signal_ids(true_branch, max_sig_id);
            verify_expr_signal_ids(false_branch, max_sig_id);
        }
        IrExpr::Signed(inner) => {
            verify_expr_signal_ids(inner, max_sig_id);
        }
        IrExpr::String(_) | IrExpr::FillLit(_) | IrExpr::This => {}
        IrExpr::SysFunc { args, .. } => {
            for arg in args {
                verify_expr_signal_ids(arg, max_sig_id);
            }
        }
        IrExpr::NewCall { .. } | IrExpr::UdpLookup { .. } => {}
        IrExpr::MethodCall { obj, args, .. } => {
            verify_expr_signal_ids(obj, max_sig_id);
            for arg in args {
                verify_expr_signal_ids(arg, max_sig_id);
            }
        }
        IrExpr::MemberAccess { obj, .. } => {
            verify_expr_signal_ids(obj, max_sig_id);
        }
        IrExpr::DpiCall { args, .. } => {
            for arg in args {
                verify_expr_signal_ids(arg, max_sig_id);
            }
        }
        IrExpr::HierRef(_) => {}
        IrExpr::Inside { expr, list } => {
            verify_expr_signal_ids(expr, max_sig_id);
            for e in list {
                verify_expr_signal_ids(e, max_sig_id);
            }
        }
        IrExpr::InsideRange { expr, lo, hi } => {
            verify_expr_signal_ids(expr, max_sig_id);
            verify_expr_signal_ids(lo, max_sig_id);
            verify_expr_signal_ids(hi, max_sig_id);
        }
        IrExpr::Cast { expr, .. } => {
            verify_expr_signal_ids(expr, max_sig_id);
        }
        IrExpr::StreamingConcat { slices, .. } => {
            for e in slices {
                verify_expr_signal_ids(e, max_sig_id);
            }
        }
        IrExpr::Dist { expr, .. } => {
            verify_expr_signal_ids(expr, max_sig_id);
        }
        IrExpr::FuncCall { args, .. } => {
            for arg in args {
                verify_expr_signal_ids(arg, max_sig_id);
            }
        }
        IrExpr::Const(_) | IrExpr::VifBinding { .. } | IrExpr::VirtualIfaceAccess { .. } => {}
        IrExpr::RealConst(_) => {}
        IrExpr::IncDec { read, lv, .. } => {
            verify_expr_signal_ids(read, max_sig_id);
            verify_lvalue_signal_ids(lv, max_sig_id);
        }
    }
}
