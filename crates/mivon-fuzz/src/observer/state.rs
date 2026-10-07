//! state — bangun LrmState dari IrDesign mivon.

#![cfg(feature = "dev")]

use crate::lrm_model::state::{
    AlwaysKind, AlwaysProcess, DesignModel, FinalProcess, HierarchyNode, InitialProcess, LrmState,
    ParameterBinding, PortBinding, PortDirection, ProcessModel, SchedulerModel, ScopeGraph,
    ScopeId, ScopeKind, ScopeNode, SymbolEntry, SymbolId, SymbolKind, SymbolTable, TypeSystem,
};

pub fn build_lrm_state(design: &mivon_ir::IrDesign) -> LrmState {
    LrmState {
        scopes: build_scope_graph(design),
        symbols: build_symbol_table(design),
        types: TypeSystem::default(),
        elaborated_design: build_design_model(design),
        processes: build_process_model(design),
        scheduler: SchedulerModel::default(),
    }
}

fn build_scope_graph(design: &mivon_ir::IrDesign) -> ScopeGraph {
    let mut graph = ScopeGraph::default();
    let root_name = design.top.name.as_str().to_string();
    let root_id = ScopeId(root_name.clone());
    graph.root = root_id.clone();
    graph.nodes.insert(
        root_id.clone(),
        ScopeNode { id: root_id.clone(), kind: ScopeKind::Module, name: root_name },
    );
    for (sym, _) in &design.modules {
        let name_s = sym.as_str().to_string();
        let id = ScopeId(name_s.clone());
        graph.nodes.insert(
            id.clone(),
            ScopeNode { id: id.clone(), kind: ScopeKind::Module, name: name_s },
        );
        graph.parent.insert(id, root_id.clone());
    }
    graph
}

fn build_symbol_table(design: &mivon_ir::IrDesign) -> SymbolTable {
    let mut table = SymbolTable::default();
    let top_name = design.top.name.as_str().to_string();
    let root_scope = ScopeId(top_name.clone());

    for signal in &design.top.signals {
        let sig_name = signal.name.as_str().to_string();
        let id = SymbolId(format!("{}.{}", top_name, sig_name));
        let entry = SymbolEntry {
            id: id.clone(),
            name: sig_name.clone(),
            kind: SymbolKind::Variable,
            scope: root_scope.clone(),
            width: signal.width as u32,
            is_signed: signal.is_signed,
        };
        table.by_name.insert((root_scope.clone(), sig_name), id.clone());
        table.entries.insert(id, entry);
    }
    table
}

fn build_design_model(design: &mivon_ir::IrDesign) -> DesignModel {
    let mut model = DesignModel::default();
    let top_name = design.top.name.as_str().to_string();

    model.hierarchy.push(HierarchyNode {
        instance_path: top_name.clone(),
        module_name: top_name.clone(),
        children: Vec::new(),
    });

    for inst in &design.top.sub_instances {
        let inst_name = inst.instance_name.as_str().to_string();
        let mod_name = inst.module_name.as_str().to_string();

        model.hierarchy.push(HierarchyNode {
            instance_path: format!("{}.{}", top_name, inst_name),
            module_name: mod_name,
            children: Vec::new(),
        });

        for (pname, pval) in inst.param_map.iter() {
            let key = format!("{}.{}.{}", top_name, inst_name, pname.as_str());
            model.parameters.insert(
                key,
                ParameterBinding {
                    instance_path: format!("{}.{}", top_name, inst_name),
                    param_name: pname.as_str().to_string(),
                    default_value: String::new(),
                    override_value: Some(pval.to_string()),
                    is_overridden: true,
                },
            );
        }

        for (port_sym, _) in inst.port_map.iter() {
            model.port_bindings.push(PortBinding {
                instance_path: format!("{}.{}", top_name, inst_name),
                port_name: port_sym.as_str().to_string(),
                connected_signal: String::new(),
                direction: PortDirection::Input,
            });
        }
    }

    model
}

fn build_process_model(design: &mivon_ir::IrDesign) -> ProcessModel {
    let mut model = ProcessModel::default();
    for (i, proc) in design.top.processes.iter().enumerate() {
        use mivon_ir::Process;
        match proc {
            Process::Combinational { sensitivity, .. }
            | Process::CombReactive { sensitivity, .. } => {
                model.always_blocks.push(AlwaysProcess {
                    id: format!("always_comb_{}", i),
                    kind: AlwaysKind::Comb,
                    sensitivity: sensitivity.iter().map(|s| format!("{:?}", s)).collect(),
                });
            }
            Process::Sequential { clock, .. } => {
                model.always_blocks.push(AlwaysProcess {
                    id: format!("always_ff_{}", i),
                    kind: AlwaysKind::Ff,
                    sensitivity: vec![format!("{:?}", clock)],
                });
            }
            Process::AlwaysWithDelay { .. } => {
                model.always_blocks.push(AlwaysProcess {
                    id: format!("always_{}", i),
                    kind: AlwaysKind::Plain,
                    sensitivity: Vec::new(),
                });
            }
            Process::Initial { .. } => {
                model.initial_blocks.push(InitialProcess { id: format!("initial_{}", i) });
            }
            Process::Final { .. } => {
                model.final_blocks.push(FinalProcess { id: format!("final_{}", i) });
            }
        }
    }
    model
}
