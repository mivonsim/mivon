//! uvm_config_db — path matching & lookup (F19).
//! UVM `set`/`get` memakai inst_path dengan wildcard (`*.agent`,
//! `uvm_test_top.*`, `?` satu karakter). Lookup: exact match menang, lalu
//! wildcard pattern PALING SPESIFIK (paling sedikit wildcard; tie-break
//! prefix literal terpanjang, lalu pattern terpanjang).
//! 1 file = 1 tanggung jawab: hanya path matching + lookup config db.

use super::super::SimulationEngine;
use crate::simulator::util::*;
use mivon_compiler::hir::{LogicVec, ObjId};
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::IrExpr;

/// Wildcard match — `*` match zero-or-more karakter (termasuk `.`), `?`
/// match satu karakter. DP iteratif dua baris (space O(len inst_path)).
pub(crate) fn config_db_path_match(pattern: &str, inst_path: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = inst_path.chars().collect();
    // dp[j] = apakah p[..i] match s[..j] (i = pattern yang sudah diproses)
    let mut dp = vec![false; s.len() + 1];
    dp[0] = true;
    for &pc in &p {
        let mut next = vec![false; s.len() + 1];
        match pc {
            '*' => {
                // '*' match zero+ chars: next[j] = dp[j] (nol) || next[j-1] (satu+)
                let mut run = false;
                for j in 0..=s.len() {
                    run = run || dp[j];
                    next[j] = run;
                }
            }
            '?' => {
                next[1..=s.len()].copy_from_slice(&dp[..s.len()]);
            }
            c => {
                for j in 1..=s.len() {
                    next[j] = dp[j - 1] && s[j - 1] == c;
                }
            }
        }
        dp = next;
    }
    dp[s.len()]
}

/// Spesifisitas pattern — LEBIH KECIL = LEBIH spesifik:
/// (jumlah wildcard, negasi panjang prefix literal, negasi panjang pattern).
/// Exact match (`wildcards == 0`, prefix = len) selalu menang.
fn pattern_specificity(pattern: &str) -> (usize, usize, usize) {
    let wildcards = pattern.chars().filter(|c| *c == '*' || *c == '?').count();
    let literal_prefix = pattern.find(['*', '?']).unwrap_or(pattern.len());
    let len = pattern.len();
    (wildcards, usize::MAX - literal_prefix, usize::MAX - len)
}

impl SimulationEngine {
    /// Cari nilai config db: exact match dulu, lalu wildcard paling spesifik.
    /// Key sekarang (inst_name, field_name, type_param) — type_param
    /// memungkinkan `uvm_config_db#(T1)::set` dan `uvm_config_db#(T2)::set`
    /// menyimpan nilai yang berbeda untuk inst/field yang sama.
    pub(crate) fn config_db_find(
        &self,
        inst_name: &str,
        field_name: &str,
        type_param: &str,
    ) -> Option<LogicVec> {
        // 1) Exact match — kemenangan penuh.
        if let Some(v) = self
            .uvm_config_db_data
            .get(&(inst_name.to_string(), field_name.to_string(), type_param.to_string()))
        {
            return Some(v.clone());
        }
        // 2) Wildcard — pilih pattern paling spesifik yang match (abaikan type_param
        //    saat wildcard matching; type_param hanya untuk exact match).
        let mut best: Option<((usize, usize, usize), LogicVec)> = None;
        for ((pat, f, _tp), v) in &self.uvm_config_db_data {
            if f != field_name {
                continue;
            }
            if config_db_path_match(pat, inst_name) {
                let sp = pattern_specificity(pat);
                let better = match &best {
                    None => true,
                    Some((bsp, _)) => sp < *bsp,
                };
                if better {
                    best = Some((sp, v.clone()));
                }
            }
        }
        best.map(|(_, v)| v)
    }

    /// VERIF-06: `uvm_config_db::exists` — 1 bila key (inst, field, type_param)
    /// punya nilai (exact atau wildcard paling spesifik), 0 bila tidak ada.
    pub(crate) fn config_db_exists(
        &self,
        inst_name: &str,
        field_name: &str,
        type_param: &str,
    ) -> bool {
        self.config_db_find(inst_name, field_name, type_param).is_some()
    }

    /// VERIF-07: cari nilai uvm_resource_db — exact match dulu, lalu
    /// wildcard scope paling spesifik (sama dengan config_db_find, reuse
    /// config_db_path_match + pattern_specificity). uvm_resource_db::set
    /// menyimpan key (scope, name); get/exists/read_by_name memakai lookup
    /// ini sehingga `set("*.env", ...)` terbaca oleh `get("tb.env", ...)`.
    pub(crate) fn resource_db_find(&self, scope: &str, name: &str) -> Option<LogicVec> {
        if let Some(v) = self
            .uvm_resource_db_data
            .get(&(scope.to_string(), name.to_string()))
        {
            return Some(v.clone());
        }
        let mut best: Option<((usize, usize, usize), LogicVec)> = None;
        for ((pat, n), v) in &self.uvm_resource_db_data {
            if n != name {
                continue;
            }
            if config_db_path_match(pat, scope) {
                let sp = pattern_specificity(pat);
                let better = match &best {
                    None => true,
                    Some((bsp, _)) => sp < *bsp,
                };
                if better {
                    best = Some((sp, v.clone()));
                }
            }
        }
        best.map(|(_, v)| v)
    }

    /// VERIF-07: `uvm_resource_db::exists` — 1 bila resource (scope, name)
    /// punya nilai (exact atau wildcard paling spesifik), 0 bila tidak ada.
    pub(crate) fn resource_db_exists(&self, scope: &str, name: &str) -> bool {
        self.resource_db_find(scope, name).is_some()
    }

    /// VERIF-07: dispatch statement-context UVM DB call —
    /// `uvm_config_db::set/get` dan `uvm_resource_db::set/get/exists/
    /// write_by_name/read_by_name` sebagai bare statement di initial/always
    /// block (jalur IR, masuk via evaluate_syscall). Sebelumnya statement ini
    /// di-eliminasi elaborator sbg side-effect-free → set tidak pernah
    /// menyimpan → get selalu 0. Semantik argumen disamakan dgn handler
    /// ekspresi (expr.rs): config_db::set(obj, inst, field, value) — value di
    /// arg ke-3; resource_db::set(scope, name, value) — value di arg ke-2.
    pub(crate) fn execute_uvm_db_stmt(
        &mut self,
        name: &str,
        ir_args: &[IrExpr],
    ) -> Result<(), SimError> {
        let arg_vals: Vec<LogicVec> = ir_args
            .iter()
            .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
            .collect();
        match name {
            "uvm_config_db::set" => {
                let inst = arg_vals.get(1).map(logicvec_to_string).unwrap_or_default();
                let field = arg_vals.get(2).map(logicvec_to_string).unwrap_or_default();
                let value = arg_vals.get(3).cloned().unwrap_or_else(|| LogicVec::new(1));
                let type_param = arg_vals.get(4).map(logicvec_to_string).unwrap_or_default();
                self.uvm_config_db_data
                    .insert((inst.clone(), field.clone(), type_param.clone()), value);
                // VERIF-06: bangunkan waiter wait_modified utk key ini.
                self.config_db_release_waiters(&inst, &field)?;
            }
            "uvm_config_db::get" => {
                let inst = arg_vals.get(1).map(logicvec_to_string).unwrap_or_default();
                let field = arg_vals.get(2).map(logicvec_to_string).unwrap_or_default();
                let type_param = arg_vals.get(4).map(logicvec_to_string).unwrap_or_default();
                if let Some(v) = self.config_db_find(&inst, &field, &type_param) {
                    if let Some(IrExpr::Signal(sid, _)) = ir_args.get(3) {
                        self.state.write_signal(*sid, v);
                    }
                }
            }
            "uvm_resource_db::set" | "uvm_resource_db::write_by_name" => {
                let scope = arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                let rname = arg_vals.get(1).map(logicvec_to_string).unwrap_or_default();
                let value = arg_vals.get(2).cloned().unwrap_or_else(|| LogicVec::new(1));
                self.uvm_resource_db_data.insert((scope, rname), value);
            }
            "uvm_resource_db::get" | "uvm_resource_db::read_by_name" => {
                let scope = arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                let rname = arg_vals.get(1).map(logicvec_to_string).unwrap_or_default();
                if let Some(v) = self.resource_db_find(&scope, &rname) {
                    if let Some(IrExpr::Signal(sid, _)) = ir_args.get(2) {
                        self.state.write_signal(*sid, v);
                    }
                }
            }
            "uvm_resource_db::exists" => {
                // return value diabaikan dalam konteks statement
            }
            "uvm_root::run_test" => {
                // VERIF-04: varian class-method run_test("name") sebagai
                // statement — sama dgn bare run_test (F18).
                let test_name = arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                self.run_uvm_test(&test_name)?;
            }
            "uvm_tr_database::get_db" => {
                // VERIF-18: singleton db sebagai statement — pastikan id ada.
                if self.uvm_tr_db_id.is_none() {
                    let id = self.state.alloc_object(Symbol::intern("uvm_tr_database"));
                    self.uvm_tr_db_id = Some(id);
                }
            }
            "uvm_tr_database::get_stream" => {
                // VERIF-18: get_stream(name) sebagai statement — buat stream.
                let stream_name = arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                self.tr_stream_get(&stream_name);
            }
            "uvm_tr_database::set_stream" => {
                // VERIF-18: set_stream(name) — stream default db.
                let stream_name = arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                self.tr_stream_get(&stream_name);
                self.tr_db_default_stream = Some(stream_name);
            }
            _ => {}
        }
        Ok(())
    }

    /// VERIF-06: release SEMUA waiter blocking `wait_modified` untuk key
    /// (inst, field) — dipanggil `set` setelah insert. Menjadwalkan
    /// `ContinueAstBlock` t+1 (pola uvm_release_waiters).
    pub(crate) fn config_db_release_waiters(
        &mut self,
        inst_name: &str,
        field_name: &str,
    ) -> Result<(), SimError> {
        let key = (inst_name.to_string(), field_name.to_string());
        let waiters = self.uvm_config_db_waiters.remove(&key).unwrap_or_default();
        if waiters.is_empty() {
            return Ok(());
        }
        let t = self.state.time as usize + 1;
        self.ensure_events(t);
        for w in waiters {
            self.push_event(
                t,
                crate::simulator::types::RegionEvent {
                    region: crate::simulator::types::EventRegion::Active,
                    event: crate::simulator::types::EventKind::ContinueAstBlock(
                        w.continuation,
                        w.fork_id,
                        w.this,
                        w.method,
                    ),
                },
            );
        }
        Ok(())
    }

    /// Path hierarki penuh objek UVM (`uvm_test_top.env.agent`), dipakai
    /// `get` dengan inst_path kosong — `uvm_config_db::get(this, "", ...)`.
    /// Fallback ke `current_instance_path` (top module) bila objek tak punya
    /// nama hierarki.
    pub(crate) fn uvm_object_full_path(&self, obj_id: ObjId) -> String {
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
        if full.is_empty() {
            self.current_instance_path.clone().unwrap_or_default()
        } else {
            full
        }
    }
}
