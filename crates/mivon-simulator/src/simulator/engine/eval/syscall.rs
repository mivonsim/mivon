use super::super::SimulationEngine;
use mivon_ast::*;
use mivon_core::error::SimError;
use mivon_ir::*;
use rand::Rng;

impl SimulationEngine {
    pub(crate) fn check_timing_constraints(&mut self) -> Result<(), SimError> {
        let current_time = self.state.time;
        let signal_names: Vec<(String, SignalId)> = self
            .design
            .top
            .signals
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.to_string(), i))
            .collect();
        let items = self.design.specify_items.clone();
        for item in &items {
            match item {
                SpecifyItem::SetupCheck {
                    data,
                    ref_event,
                    ref_edge,
                    limit,
                } => {
                    // SIM-24: setup violation dicek saat ref edge terjadi (ref_chg ==
                    // current_time), arah edge harus cocok dengan ref_edge (posedge/
                    // negedge), lalu bandingkan kapan data terakhir berubah. Fix false
                    // positive: sebelumnya fire di KEDUA edge ref (negedge juga memicu
                    // posedge-check).
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let (Expr::Ident { name: data_sig, .. }, Expr::Ident { name: ref_sig, .. }) =
                        (data, ref_event)
                    {
                        if let (Some((_, dsid)), Some((_, rsid))) = (
                            signal_names
                                .iter()
                                .find(|(n, _)| n.as_str() == data_sig.as_str()),
                            signal_names
                                .iter()
                                .find(|(n, _)| n.as_str() == ref_sig.as_str()),
                        ) {
                            if let (Some(&data_chg), Some(&ref_chg)) = (
                                self.signal_last_change.get(dsid),
                                self.signal_last_change.get(rsid),
                            ) {
                                // Arah edge ref harus cocok dengan spesifikasi (kalau ada)
                                let dir_ok = match ref_edge {
                                    Some(e) => self.signal_last_dir.get(rsid) == Some(e),
                                    None => true,
                                };
                                if dir_ok
                                    && ref_chg == current_time
                                    && data_chg < ref_chg
                                    && ref_chg - data_chg <= limit_val
                                {
                                    self.emit_warning(
                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                        format!("$setup violation: data '{}' changed {}ns before ref (limit={}ns)",
                                            data_sig, ref_chg - data_chg, limit_val),
                                    );
                                }
                            }
                        }
                    }
                }
                SpecifyItem::HoldCheck {
                    ref_event,
                    ref_edge,
                    data,
                    limit,
                } => {
                    // SIM-24: hold violation dicek saat data BERUBAH pada step ini,
                    // dan ref edge (arah sesuai ref_edge) terjadi dalam `limit` sebelumnya.
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let (Expr::Ident { name: ref_sig, .. }, Expr::Ident { name: data_sig, .. }) =
                        (ref_event, data)
                    {
                        if let (Some((_, rsid)), Some((_, dsid))) = (
                            signal_names
                                .iter()
                                .find(|(n, _)| n.as_str() == ref_sig.as_str()),
                            signal_names
                                .iter()
                                .find(|(n, _)| n.as_str() == data_sig.as_str()),
                        ) {
                            if let (Some(&ref_chg), Some(&data_chg)) = (
                                self.signal_last_change.get(rsid),
                                self.signal_last_change.get(dsid),
                            ) {
                                let dir_ok = match ref_edge {
                                    Some(e) => self.signal_last_dir.get(rsid) == Some(e),
                                    None => true,
                                };
                                if dir_ok
                                    && data_chg == current_time
                                    && ref_chg < data_chg
                                    && data_chg - ref_chg <= limit_val
                                {
                                    self.emit_warning(
                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                        format!("$hold violation: data '{}' changed {}ns after ref (limit={}ns)",
                                            data_sig, data_chg - ref_chg, limit_val),
                                    );
                                }
                            }
                        }
                    }
                }
                SpecifyItem::SetupHoldCheck {
                    ref_event,
                    ref_edge,
                    data,
                    setup_limit,
                    hold_limit,
                } => {
                    let setup_val = const_eval_simple(setup_limit).unwrap_or(0) as u64;
                    let hold_val = const_eval_simple(hold_limit).unwrap_or(0) as u64;
                    if let (Expr::Ident { name: ref_sig, .. }, Expr::Ident { name: data_sig, .. }) =
                        (ref_event, data)
                    {
                        if let (Some((_, rsid)), Some((_, dsid))) = (
                            signal_names
                                .iter()
                                .find(|(n, _)| n.as_str() == ref_sig.as_str()),
                            signal_names
                                .iter()
                                .find(|(n, _)| n.as_str() == data_sig.as_str()),
                        ) {
                            if let (Some(&ref_chg), Some(&data_chg)) = (
                                self.signal_last_change.get(rsid),
                                self.signal_last_change.get(dsid),
                            ) {
                                let dir_ok = match ref_edge {
                                    Some(e) => self.signal_last_dir.get(rsid) == Some(e),
                                    None => true,
                                };
                                // Setup: ref edge step ini, data berubah dalam setup window sebelum edge
                                if dir_ok
                                    && ref_chg == current_time
                                    && data_chg < ref_chg
                                    && ref_chg - data_chg <= setup_val
                                {
                                    self.emit_warning(
                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                        format!("$setuphold (setup) violation: data '{}' changed {}ns before ref (setup={}ns)",
                                            data_sig, ref_chg - data_chg, setup_val),
                                    );
                                }
                                // Hold: data berubah step ini, ref edge dalam hold window sebelumnya
                                if dir_ok
                                    && data_chg == current_time
                                    && ref_chg < data_chg
                                    && data_chg - ref_chg <= hold_val
                                {
                                    self.emit_warning(
                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                        format!("$setuphold (hold) violation: data '{}' changed {}ns after ref (hold={}ns)",
                                            data_sig, data_chg - ref_chg, hold_val),
                                    );
                                }
                            }
                        }
                    }
                }
                SpecifyItem::RecoveryCheck {
                    data,
                    ref_event: _ref_event,
                    ref_edge: _,
                    limit,
                } => {
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: data_sig, .. } = data {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == data_sig.as_str())
                        {
                            if let Some(&last_change) = self.signal_last_change.get(sid) {
                                let delta = current_time - last_change;
                                if delta > 0 && delta <= limit_val {
                                    let key = ("$recovery".to_string(), *sid);
                                    if self.timing_reported.get(&key) != Some(&last_change) {
                                        self.timing_reported.insert(key, last_change);
                                        self.emit_warning(
                                            mivon_core::diagnostics::DiagCode::TimingViolation,
                                            format!("$recovery violation: signal '{}' changed {}ns before ref (limit={}ns)", data_sig, delta, limit_val),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                SpecifyItem::RemovalCheck {
                    ref_event: _ref_event,
                    ref_edge: _,
                    data,
                    limit,
                } => {
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: data_sig, .. } = data {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == data_sig.as_str())
                        {
                            if let Some(&last_change) = self.signal_last_change.get(sid) {
                                let delta = current_time - last_change;
                                if delta > 0 && delta <= limit_val {
                                    let key = ("$removal".to_string(), *sid);
                                    if self.timing_reported.get(&key) != Some(&last_change) {
                                        self.timing_reported.insert(key, last_change);
                                        self.emit_warning(
                                            mivon_core::diagnostics::DiagCode::TimingViolation,
                                            format!("$removal violation: signal '{}' changed {}ns before ref (limit={}ns)", data_sig, delta, limit_val),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                SpecifyItem::RecoveryRemovalCheck {
                    ref_event: _ref_event,
                    ref_edge: _,
                    data,
                    recovery_limit,
                    removal_limit,
                } => {
                    let recov_val = const_eval_simple(recovery_limit).unwrap_or(0) as u64;
                    let remov_val = const_eval_simple(removal_limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: data_sig, .. } = data {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == data_sig.as_str())
                        {
                            if let Some(&last_change) = self.signal_last_change.get(sid) {
                                let delta = current_time - last_change;
                                if delta > 0 && delta <= recov_val {
                                    let key = ("$recrem-recov".to_string(), *sid);
                                    if self.timing_reported.get(&key) != Some(&last_change) {
                                        self.timing_reported.insert(key, last_change);
                                        self.emit_warning(
                                            mivon_core::diagnostics::DiagCode::TimingViolation,
                                            format!("$recrem (recovery) violation: signal '{}' changed {}ns before ref (recov={}ns)", data_sig, delta, recov_val),
                                        );
                                    }
                                }
                                if delta > 0 && delta <= remov_val {
                                    let key = ("$recrem-remov".to_string(), *sid);
                                    if self.timing_reported.get(&key) != Some(&last_change) {
                                        self.timing_reported.insert(key, last_change);
                                        self.emit_warning(
                                            mivon_core::diagnostics::DiagCode::TimingViolation,
                                            format!("$recrem (removal) violation: signal '{}' changed {}ns before ref (remov={}ns)", data_sig, delta, remov_val),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                SpecifyItem::PeriodCheck {
                    ref_event,
                    ref_edge,
                    limit,
                } => {
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: ref_sig, .. } = ref_event {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == ref_sig.as_str())
                        {
                            // Dedupe: hanya fire saat edge baru terjadi pada step ini
                            // (last_change == current_time) dan period antar-edge < limit.
                            if let (Some(&last_change), Some(&prev_change)) = (
                                self.signal_last_change.get(sid),
                                self.signal_prev_change.get(sid),
                            ) {
                                let dir_ok = match ref_edge {
                                    Some(e) => self.signal_last_dir.get(sid) == Some(e),
                                    None => true,
                                };
                                if dir_ok
                                    && last_change == current_time
                                    && prev_change < last_change
                                    && last_change - prev_change < limit_val
                                {
                                    let delta = last_change - prev_change;
                                    self.emit_warning(
                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                        format!("$period violation: signal '{}' period {}ns < minimum {}ns", ref_sig, delta, limit_val),
                                    );
                                }
                            }
                        }
                    }
                }
                SpecifyItem::WidthCheck {
                    ref_event,
                    ref_edge,
                    limit,
                    threshold: _threshold,
                } => {
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: ref_sig, .. } = ref_event {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == ref_sig.as_str())
                        {
                            // Dedupe: fire sekali saat pulse berakhir (edge baru terjadi),
                            // lebar pulse = last_change - prev_change. Bila ref_edge
                            // dispesifikasikan, pulse berakhir pada edge KEBALIKAN arah
                            // (mis. $width(posedge clk) mengukur pulse high yang berakhir
                            // di negedge) — filter ini mencegah false positive pulse low.
                            let dir_ok = match ref_edge {
                                Some(mivon_ast::types::EdgeKind::PosEdge) => {
                                    self.signal_last_dir.get(sid)
                                        == Some(&mivon_ast::types::EdgeKind::NegEdge)
                                }
                                Some(mivon_ast::types::EdgeKind::NegEdge) => {
                                    self.signal_last_dir.get(sid)
                                        == Some(&mivon_ast::types::EdgeKind::PosEdge)
                                }
                                None => true,
                            };
                            if let (Some(&last_change), Some(&prev_change)) = (
                                self.signal_last_change.get(sid),
                                self.signal_prev_change.get(sid),
                            ) {
                                if dir_ok
                                    && last_change == current_time
                                    && prev_change < last_change
                                    && last_change - prev_change < limit_val
                                {
                                    let delta = last_change - prev_change;
                                    self.emit_warning(
                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                        format!("$width violation: signal '{}' pulse width {}ns < minimum {}ns", ref_sig, delta, limit_val),
                                    );
                                }
                            }
                        }
                    }
                }
                SpecifyItem::SkewCheck {
                    ref_event,
                    ref_edge: _,
                    data,
                    limit,
                } => {
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: data_sig, .. } = data {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == data_sig.as_str())
                        {
                            if let Some(&data_change) = self.signal_last_change.get(sid) {
                                if let Expr::Ident { name: ref_sig, .. } = &ref_event {
                                    if let Some((_, rsid)) = signal_names
                                        .iter()
                                        .find(|(n, _)| n.as_str() == ref_sig.as_str())
                                    {
                                        if let Some(&ref_change) = self.signal_last_change.get(rsid)
                                        {
                                            let skew = data_change.abs_diff(ref_change);
                                            // Dedupe: hanya fire saat salah satu sinyal berubah
                                            if skew > limit_val
                                                && (data_change == current_time
                                                    || ref_change == current_time)
                                            {
                                                let key = ("$skew".to_string(), *sid);
                                                if self.timing_reported.get(&key)
                                                    != Some(&current_time)
                                                {
                                                    self.timing_reported.insert(key, current_time);
                                                    self.emit_warning(
                                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                                        format!("$skew violation: skew {}ns > max {}ns between '{}' and '{}'", skew, limit_val, data_sig, ref_sig),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                SpecifyItem::TimeskewCheck {
                    ref_event,
                    ref_edge: _,
                    data,
                    limit,
                    threshold: _threshold,
                } => {
                    let limit_val = const_eval_simple(limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: data_sig, .. } = data {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == data_sig.as_str())
                        {
                            if let Some(&data_change) = self.signal_last_change.get(sid) {
                                if let Expr::Ident { name: ref_sig, .. } = &ref_event {
                                    if let Some((_, rsid)) = signal_names
                                        .iter()
                                        .find(|(n, _)| n.as_str() == ref_sig.as_str())
                                    {
                                        if let Some(&ref_change) = self.signal_last_change.get(rsid)
                                        {
                                            let skew = data_change.abs_diff(ref_change);
                                            if skew > limit_val
                                                && (data_change == current_time
                                                    || ref_change == current_time)
                                            {
                                                let key = ("$timeskew".to_string(), *sid);
                                                if self.timing_reported.get(&key)
                                                    != Some(&current_time)
                                                {
                                                    self.timing_reported.insert(key, current_time);
                                                    self.emit_warning(
                                                        mivon_core::diagnostics::DiagCode::TimingViolation,
                                                        format!("$timeskew violation: skew {}ns > max {}ns between '{}' and '{}'", skew, limit_val, data_sig, ref_sig),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                SpecifyItem::NochangeCheck {
                    ref_event: _ref_event,
                    ref_edge: _,
                    data,
                    start_limit,
                    end_limit,
                } => {
                    let start_val = const_eval_simple(start_limit).unwrap_or(0) as u64;
                    let end_val = const_eval_simple(end_limit).unwrap_or(0) as u64;
                    if let Expr::Ident { name: data_sig, .. } = data {
                        if let Some((_, sid)) = signal_names
                            .iter()
                            .find(|(n, _)| n.as_str() == data_sig.as_str())
                        {
                            if let Some(&last_change) = self.signal_last_change.get(sid) {
                                let delta = current_time - last_change;
                                if delta > 0 && delta >= start_val && delta <= end_val {
                                    let key = ("$nochange".to_string(), *sid);
                                    if self.timing_reported.get(&key) != Some(&last_change) {
                                        self.timing_reported.insert(key, last_change);
                                        self.emit_warning(
                                            mivon_core::diagnostics::DiagCode::TimingViolation,
                                            format!("$nochange violation: signal '{}' changed within window [{}ns, {}ns] (delta={}ns)", data_sig, start_val, end_val, delta),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// SIM-06/07/08/10: evaluasi timing checks dari SDF (TIMINGCHECK) yang
    /// disimpan `annotate_sdf` (`sdf_timing_checks`) — sebelumnya di-parse dan
    /// disimpan TAPI tidak pernah dievaluasi. Dipanggil sekali per time step
    /// (postponed region, setelah `check_timing_constraints` specify-block).
    ///
    /// - SIM-07: min:typ:max via `get_timing_mode()` (sama dengan net delay).
    /// - SIM-08: delay NEGATIF didukung — setup negatif = window meluas SETELAH
    ///   ref edge (violation dicek saat data berubah); hold negatif = window
    ///   meluas SEBELUM ref edge (violation dicek saat ref edge).
    /// - SIM-10: SETUPHOLD di-parse penuh (signal/ref_signal + setup & hold).
    ///   Dedupe via `timing_reported` (key check-type + signal id) agar width/
    ///   period/recovery tidak spam — konsisten dengan specify-block (SIM-24).
    pub(crate) fn check_sdf_timing_constraints(&mut self) -> Result<(), SimError> {
        if self.sdf_timing_checks.is_empty() {
            return Ok(());
        }
        let current_time = self.state.time;
        let mode = crate::simulator::sdf::get_timing_mode();
        let signal_names: Vec<(String, usize)> = self
            .design
            .top
            .signals
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.to_string(), i))
            .collect();
        // SDF memakai hierarchical names (`u1.d`) — exact match dulu, lalu
        // suffix match setelah titik terakhir (mirip net delay annotation).
        let resolve_id = |sig_names: &[(String, usize)], name: &str| -> Option<usize> {
            if let Some((_, id)) = sig_names.iter().find(|(n, _)| n.as_str() == name) {
                return Some(*id);
            }
            let suffix = format!(".{}", name.trim_start_matches('.'));
            sig_names
                .iter()
                .find(|(n, _)| n.as_str().ends_with(&suffix))
                .map(|(_, id)| *id)
        };
        let checks = self.sdf_timing_checks.clone();
        for check in &checks {
            let tname = check.type_name().to_string();
            match check {
                crate::simulator::sdf::TimingCheck::Setup {
                    signal,
                    ref_signal,
                    delay,
                } => {
                    let limit = delay.get(mode);
                    let (Some(dsid), Some(rsid)) = (
                        resolve_id(&signal_names, signal),
                        resolve_id(&signal_names, ref_signal),
                    ) else {
                        continue;
                    };
                    let (Some(&_data_chg), Some(&_ref_chg)) = (
                        self.signal_last_change.get(&dsid),
                        self.signal_last_change.get(&rsid),
                    ) else {
                        continue;
                    };
                    let (Some(&data_chg), Some(&ref_chg)) = (
                        self.signal_last_change.get(&dsid),
                        self.signal_last_change.get(&rsid),
                    ) else {
                        continue;
                    };
                    if limit < 0.0 {
                        // Setup negatif (SIM-08): window meluas setelah ref edge —
                        // violation saat data berubah dalam |limit| setelah ref.
                        let win = (-limit) as u64;
                        if data_chg == current_time
                            && ref_chg < data_chg
                            && data_chg - ref_chg <= win
                        {
                            let key = (format!("{}-neg", tname), dsid);
                            if self.timing_reported.get(&key) != Some(&data_chg) {
                                self.timing_reported.insert(key, data_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("{} violation (negative setup): data '{}' changed {}ns after ref '{}' (window={}ns)",
                                        tname, signal, data_chg - ref_chg, ref_signal, win),
                                );
                            }
                        }
                    } else {
                        let limit_u = limit as u64;
                        if ref_chg == current_time
                            && data_chg < ref_chg
                            && ref_chg - data_chg <= limit_u
                        {
                            let key = (tname.clone(), rsid);
                            if self.timing_reported.get(&key) != Some(&ref_chg) {
                                self.timing_reported.insert(key, ref_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("{} violation: data '{}' changed {}ns before ref '{}' (limit={}ns)",
                                        tname, signal, ref_chg - data_chg, ref_signal, limit_u),
                                );
                            }
                        }
                    }
                }
                crate::simulator::sdf::TimingCheck::Hold {
                    signal,
                    ref_signal,
                    delay,
                } => {
                    let limit = delay.get(mode);
                    let (Some(dsid), Some(rsid)) = (
                        resolve_id(&signal_names, signal),
                        resolve_id(&signal_names, ref_signal),
                    ) else {
                        continue;
                    };
                    let (Some(&data_chg), Some(&ref_chg)) = (
                        self.signal_last_change.get(&dsid),
                        self.signal_last_change.get(&rsid),
                    ) else {
                        continue;
                    };
                    if limit < 0.0 {
                        // Hold negatif (SIM-08): window meluas SEBELUM ref edge —
                        // violation saat ref edge terjadi, data berubah dalam
                        // |limit| sebelum ref.
                        let win = (-limit) as u64;
                        if ref_chg == current_time
                            && data_chg < ref_chg
                            && ref_chg - data_chg <= win
                        {
                            let key = (format!("{}-neg", tname), rsid);
                            if self.timing_reported.get(&key) != Some(&ref_chg) {
                                self.timing_reported.insert(key, ref_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("{} violation (negative hold): data '{}' changed {}ns before ref '{}' (window={}ns)",
                                        tname, signal, ref_chg - data_chg, ref_signal, win),
                                );
                            }
                        }
                    } else {
                        let limit_u = limit as u64;
                        if data_chg == current_time
                            && ref_chg < data_chg
                            && data_chg - ref_chg <= limit_u
                        {
                            let key = (tname.clone(), dsid);
                            if self.timing_reported.get(&key) != Some(&data_chg) {
                                self.timing_reported.insert(key, data_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("{} violation: data '{}' changed {}ns after ref '{}' (limit={}ns)",
                                        tname, signal, data_chg - ref_chg, ref_signal, limit_u),
                                );
                            }
                        }
                    }
                }
                crate::simulator::sdf::TimingCheck::Setuphold {
                    signal,
                    ref_signal,
                    setup,
                    hold,
                } => {
                    let setup_l = setup.get(mode);
                    let hold_l = hold.get(mode);
                    let (Some(dsid), Some(rsid)) = (
                        resolve_id(&signal_names, signal),
                        resolve_id(&signal_names, ref_signal),
                    ) else {
                        continue;
                    };
                    let (Some(&data_chg), Some(&ref_chg)) = (
                        self.signal_last_change.get(&dsid),
                        self.signal_last_change.get(&rsid),
                    ) else {
                        continue;
                    };
                    // Setup component (negatif → window setelah ref edge)
                    if setup_l < 0.0 {
                        let win = (-setup_l) as u64;
                        if data_chg == current_time
                            && ref_chg < data_chg
                            && data_chg - ref_chg <= win
                        {
                            let key = ("$setuphold-setup-neg".to_string(), dsid);
                            if self.timing_reported.get(&key) != Some(&data_chg) {
                                self.timing_reported.insert(key, data_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("$setuphold (setup, negative) violation: data '{}' changed {}ns after ref '{}' (window={}ns)",
                                        signal, data_chg - ref_chg, ref_signal, win),
                                );
                            }
                        }
                    } else {
                        let limit_u = setup_l as u64;
                        if ref_chg == current_time
                            && data_chg < ref_chg
                            && ref_chg - data_chg <= limit_u
                        {
                            let key = ("$setuphold-setup".to_string(), rsid);
                            if self.timing_reported.get(&key) != Some(&ref_chg) {
                                self.timing_reported.insert(key, ref_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("$setuphold (setup) violation: data '{}' changed {}ns before ref '{}' (setup={}ns)",
                                        signal, ref_chg - data_chg, ref_signal, limit_u),
                                );
                            }
                        }
                    }
                    // Hold component (negatif → window sebelum ref edge)
                    if hold_l < 0.0 {
                        let win = (-hold_l) as u64;
                        if ref_chg == current_time
                            && data_chg < ref_chg
                            && ref_chg - data_chg <= win
                        {
                            let key = ("$setuphold-hold-neg".to_string(), rsid);
                            if self.timing_reported.get(&key) != Some(&ref_chg) {
                                self.timing_reported.insert(key, ref_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("$setuphold (hold, negative) violation: data '{}' changed {}ns before ref '{}' (window={}ns)",
                                        signal, ref_chg - data_chg, ref_signal, win),
                                );
                            }
                        }
                    } else {
                        let limit_u = hold_l as u64;
                        if data_chg == current_time
                            && ref_chg < data_chg
                            && data_chg - ref_chg <= limit_u
                        {
                            let key = ("$setuphold-hold".to_string(), dsid);
                            if self.timing_reported.get(&key) != Some(&data_chg) {
                                self.timing_reported.insert(key, data_chg);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("$setuphold (hold) violation: data '{}' changed {}ns after ref '{}' (hold={}ns)",
                                        signal, data_chg - ref_chg, ref_signal, limit_u),
                                );
                            }
                        }
                    }
                }
                crate::simulator::sdf::TimingCheck::Width {
                    signal,
                    delay,
                    threshold: _,
                } => {
                    let limit = delay.get(mode);
                    let Some(sid) = resolve_id(&signal_names, signal) else {
                        continue;
                    };
                    let (Some(&last_change), Some(&prev_change)) = (
                        self.signal_last_change.get(&sid),
                        self.signal_prev_change.get(&sid),
                    ) else {
                        continue;
                    };
                    if last_change == current_time && prev_change < last_change {
                        let pulse = last_change - prev_change;
                        if pulse < limit as u64 {
                            // SIM-09 pulse control: bila signal punya (PULSE ...)
                            // di SDF, pulse pendek di-REJECT — nilai di-rollback
                            // ke commit sebelum pulse (pulse di-filter, bukan
                            // violation).
                            if let Some(&pc_limit) = self.sdf_pulse_controls.get(&sid) {
                                if let Some(prev_val) = self.signal_prev_value.get(&sid).cloned() {
                                    let key = (format!("{}-pc", tname), sid);
                                    if self.timing_reported.get(&key) != Some(&last_change) {
                                        self.timing_reported.insert(key, last_change);
                                        // Rollback: nilai commit sebelum pulse.
                                        self.state.write_signal(sid, prev_val);
                                        self.signal_last_change.insert(sid, prev_change);
                                        self.signal_prev_change.remove(&sid);
                                        self.emit_warning(
                                            mivon_core::diagnostics::DiagCode::SignalGlitch,
                                            format!("pulse control: pulse {}ns on '{}' rejected (min {}ns, pulse control {:.1}ns) — value rolled back",
                                                pulse, signal, limit as u64, pc_limit),
                                        );
                                    }
                                    continue;
                                }
                            }
                            let key = (tname.clone(), sid);
                            if self.timing_reported.get(&key) != Some(&last_change) {
                                self.timing_reported.insert(key, last_change);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!(
                                        "{} violation: signal '{}' pulse width {}ns < minimum {}ns",
                                        tname, signal, pulse, limit as u64
                                    ),
                                );
                            }
                        }
                    }
                }
                crate::simulator::sdf::TimingCheck::Period { signal, delay } => {
                    let limit = delay.get(mode);
                    let Some(sid) = resolve_id(&signal_names, signal) else {
                        continue;
                    };
                    let (Some(&last_change), Some(&prev_change)) = (
                        self.signal_last_change.get(&sid),
                        self.signal_prev_change.get(&sid),
                    ) else {
                        continue;
                    };
                    if last_change == current_time && prev_change < last_change {
                        let period = last_change - prev_change;
                        if period < limit as u64 {
                            let key = (tname.clone(), sid);
                            if self.timing_reported.get(&key) != Some(&last_change) {
                                self.timing_reported.insert(key, last_change);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!(
                                        "{} violation: signal '{}' period {}ns < minimum {}ns",
                                        tname, signal, period, limit as u64
                                    ),
                                );
                            }
                        }
                    }
                }
                crate::simulator::sdf::TimingCheck::Recovery {
                    signal,
                    ref_signal: _,
                    delay,
                }
                | crate::simulator::sdf::TimingCheck::Removal {
                    signal,
                    ref_signal: _,
                    delay,
                } => {
                    let limit = delay.get(mode);
                    let Some(sid) = resolve_id(&signal_names, signal) else {
                        continue;
                    };
                    if let Some(&last_change) = self.signal_last_change.get(&sid) {
                        let delta = current_time - last_change;
                        if delta > 0 && delta <= limit as u64 {
                            let key = (tname.clone(), sid);
                            if self.timing_reported.get(&key) != Some(&last_change) {
                                self.timing_reported.insert(key, last_change);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!("{} violation: signal '{}' changed {}ns before ref (limit={}ns)",
                                        tname, signal, delta, limit as u64),
                                );
                            }
                        }
                    }
                }
                crate::simulator::sdf::TimingCheck::Skew {
                    signal,
                    ref_signal,
                    delay,
                } => {
                    let limit = delay.get(mode);
                    let (Some(sid), Some(rsid)) = (
                        resolve_id(&signal_names, signal),
                        resolve_id(&signal_names, ref_signal),
                    ) else {
                        continue;
                    };
                    if let (Some(&data_change), Some(&ref_change)) = (
                        self.signal_last_change.get(&sid),
                        self.signal_last_change.get(&rsid),
                    ) {
                        let skew = data_change.abs_diff(ref_change);
                        if skew > limit as u64
                            && (data_change == current_time || ref_change == current_time)
                        {
                            let key = (tname.clone(), sid);
                            if self.timing_reported.get(&key) != Some(&current_time) {
                                self.timing_reported.insert(key, current_time);
                                self.emit_warning(
                                    mivon_core::diagnostics::DiagCode::TimingViolation,
                                    format!(
                                        "{} violation: skew {}ns > max {}ns between '{}' and '{}'",
                                        tname, skew, limit as u64, signal, ref_signal
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn evaluate_dpi_call(
        &mut self,
        name: &str,
        args: &[IrExpr],
        return_width: usize,
    ) -> Result<LogicVec, SimError> {
        // Check if we have a matching DPI import
        // Clone immediately to avoid borrow conflicts with self.call_
        let dpi_info = self
            .design
            .dpi_imports
            .iter()
            .find(|d| d.name == name)
            .cloned();
        let is_task = dpi_info.as_ref().map(|d| d.is_task).unwrap_or(false);

        if is_task {
            return Ok(LogicVec::new(0));
        }

        let arg_vals: Vec<LogicVec> = args
            .iter()
            .map(|a| self.evaluate_expr(a))
            .collect::<Result<_, _>>()?;

        // Known built-in DPI functions (work without external libraries)
        match name {
            "svBitToInt" | "svToInt" => {
                if let Some(val) = arg_vals.first() {
                    return Ok(LogicVec::from_u64(val.to_u64(), return_width));
                }
                Ok(LogicVec::from_u64(0, return_width))
            }
            "svBitToLong" | "svToLong" => {
                if let Some(val) = arg_vals.first() {
                    return Ok(LogicVec::from_u64(val.to_u64(), return_width));
                }
                Ok(LogicVec::from_u64(0, return_width))
            }
            "svToShortReal" | "svToReal" => {
                if let Some(val) = arg_vals.first() {
                    return Ok(val.clone());
                }
                Ok(LogicVec::from_u64(0, return_width))
            }
            "svIntToBit" | "svToBit" | "svToLogic" => {
                if let Some(val) = arg_vals.first() {
                    return Ok(val.clone());
                }
                Ok(LogicVec::from_u64(0, return_width))
            }
            "svBitToBitVal" | "svBitToLogicVal" => {
                if let Some(val) = arg_vals.first() {
                    return Ok(val.clone());
                }
                Ok(LogicVec::from_u64(0, return_width))
            }
            "svRandomize" | "sv$random" | "svUrandom" | "svUrandomRange" => {
                let r: u64 = self.rng.gen();
                Ok(LogicVec::from_u64(r, return_width))
            }
            "$test$plusargs" | "svTestPlusArgs" => {
                // Handled in SysFunc dispatch — fallback here
                Ok(LogicVec::from_u64(0, return_width))
            }
            "$value$plusargs" | "svValuePlusArgs" => Ok(LogicVec::from_u64(0, return_width)),
            _ => {
                // Try to resolve via DPI engine (dynamic library loading)
                if dpi_info.is_some() {
                    return self.call_dpi_function(name, &arg_vals, return_width, is_task);
                }
                // Unknown DPI — return 0 (F20: via DiagSink agar punya lokasi)
                self.emit_warning(
                    mivon_core::diagnostics::DiagCode::DpiError,
                    format!("DPI function '{}' not found in imports, returning 0", name),
                );
                Ok(LogicVec::from_u64(0, return_width))
            }
        }
    }

    /// Call a DPI function via the DPI engine (dynamic library resolution).
    #[cfg(feature = "dpi")]
    fn call_dpi_function(
        &mut self,
        name: &str,
        arg_vals: &[LogicVec],
        return_width: usize,
        _is_task: bool,
    ) -> Result<LogicVec, SimError> {
        use crate::simulator::dpi::*;
        use std::sync::Mutex;

        // Global DPI engine (lazy initialized)
        fn dpi_engine() -> &'static Mutex<Option<DpiEngine>> {
            use std::sync::OnceLock;
            static ENGINE: OnceLock<Mutex<Option<DpiEngine>>> = OnceLock::new();
            ENGINE.get_or_init(|| Mutex::new(Some(DpiEngine::new())))
        }

        // Find matching IrDpiImport and register library
        // Clone the info first to avoid borrow conflicts
        let dpi_info = self
            .design
            .dpi_imports
            .iter()
            .find(|d| d.name.as_str() == name)
            .cloned();

        if let Some(ref info) = dpi_info {
            let mut engine_guard = dpi_engine().lock().unwrap();
            if let Some(ref mut engine) = *engine_guard {
                // Try to resolve and call the function
                match engine.resolve_function(info) {
                    Ok(()) => {
                        // Create scope from instance path (cloned to avoid borrow)
                        let scope_path = self.current_instance_path.clone().unwrap_or_default();
                        let scope = crate::simulator::dpi::current_scope_from_path(&scope_path);
                        return engine.call_function(name, arg_vals, &scope);
                    }
                    Err(e) => {
                        self.emit_warning(
                            mivon_core::diagnostics::DiagCode::DpiError,
                            format!("DPI '{}' not found in loaded libraries: {}", name, e),
                        );
                    }
                }
            }
        }

        // Fallback
        Ok(LogicVec::from_u64(0, return_width.max(1)))
    }

    /// Non-DPI fallback when dpi feature is not compiled in
    #[cfg(not(feature = "dpi"))]
    fn call_dpi_function(
        &mut self,
        name: &str,
        _arg_vals: &[LogicVec],
        return_width: usize,
        _is_task: bool,
    ) -> Result<LogicVec, SimError> {
        self.emit_warning(
            mivon_core::diagnostics::DiagCode::DpiError,
            format!(
                "DPI function '{}' not available (compile with --features dpi)",
                name
            ),
        );
        Ok(LogicVec::from_u64(0, return_width.max(1)))
    }

    pub(crate) fn handle_ast_syscall(
        &mut self,
        name: &str,
        args: &[mivon_ast::Expr],
    ) -> Result<(), SimError> {
        if let Some((is_display, radix)) = crate::simulator::util::display_task_radix(name) {
            let ir_args: Vec<IrExpr> = args
                .iter()
                .map(|a| match a {
                    // String format harus tetap IrExpr::String agar format_display
                    // mengenalinya sebagai format (bukan diubah jadi Const biner).
                    mivon_ast::Expr::String(s) => IrExpr::String(s.clone()),
                    _ => IrExpr::Const(self.evaluate_ast_expr(a).unwrap_or(LogicVec::new(32))),
                })
                .collect();
            let msg = self.format_display_radix(&ir_args, radix);
            if is_display {
                println!("{}", msg);
            } else {
                print!("{}", msg);
            }
        } else if name == "finish" {
            self.running = false;
        } else if name == "info" || name == "warning" || name == "error" || name == "fatal" {
            // $info / $warning / $error / $fatal (LRM 1800 §20.2) — jalur AST.
            // Format argumen seperti $display; $fatal menghentikan simulasi.
            let ir_args: Vec<IrExpr> = args
                .iter()
                .map(|a| match a {
                    mivon_ast::Expr::String(s) => IrExpr::String(s.clone()),
                    _ => IrExpr::Const(self.evaluate_ast_expr(a).unwrap_or(LogicVec::new(32))),
                })
                .collect();
            let msg = self.format_severity_message(&ir_args);
            self.emit_severity(name, &msg);
        }
        Ok(())
    }
}
