use super::super::SimulationEngine;
use crate::simulator::types::*;
use crate::simulator::util::*;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

impl SimulationEngine {
    /// Tulis segmen ke sinyal ULTRA-LEBAR (array memori flatten ≥ 4M bit)
    /// langsung — tanpa read-modify-write penuh (clone 1e9 bit → OOM).
    /// Return true bila sinyal wide & partial dijalankan (jalur lama di-skip);
    /// false → gunakan RMW normal utk sinyal reguler.
    #[inline]
    pub fn write_partial_if_wide(
        &mut self,
        sig_id: SignalId,
        start: usize,
        end: usize,
        val: &mivon_ir::LogicVec,
    ) -> bool {
        let wide = self
            .design
            .top
            .signals
            .get(sig_id)
            .map(|s| s.width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD)
            .unwrap_or(false);
        if !wide {
            return false;
        }
        self.state.write_signal_slice(sig_id, start, end, val);
        true
    }

    /// Catat perubahan sinyal: geser `signal_last_change` ke `signal_prev_change`,
    /// set last_change = waktu kini, dan catat arah edge (PosEdge/NegEdge) dari
    /// transisi LSB old→new (SIM-24: dipakai setup/hold edge-aware + dedupe
    /// width/period agar tidak spam).
    fn record_signal_change(&mut self, id: usize, old: &LogicVec, new: &LogicVec) {
        // Skip same-value write: pulse timer width/period TIDAK boleh reset oleh
        // write nilai yang sama (mis. `clk = 1` saat clk sudah 1) — kalau tidak,
        // satu pulse nyata terpecah jadi segmen pendek → false positive width
        // violation (SIM-24). Gunakan bit-wise comparison (bukan to_u64 yang
        // menganggap X/Z = 0) supaya transisi partial X→0/X→1 tetap tercatat.
        if old.bits == new.bits && old.width == new.width {
            return;
        }
        if let Some(prev) = self.signal_last_change.get(&id) {
            self.signal_prev_change.insert(id, *prev);
            // Nilai commit sebelum pulse terakhir (SIM-09 pulse control):
            // nilai yang sedang digantikan oleh transisi ini.
            self.signal_prev_value.insert(id, old.clone());
        }
        self.signal_last_change.insert(id, self.state.time);
        let old_lsb = old.bits.first().copied();
        let new_lsb = new.bits.first().copied();
        let dir = match (old_lsb, new_lsb) {
            (Some(LogicVal::Zero), Some(LogicVal::One)) => {
                Some(mivon_ast::types::EdgeKind::PosEdge)
            }
            (Some(LogicVal::One), Some(LogicVal::Zero)) => {
                Some(mivon_ast::types::EdgeKind::NegEdge)
            }
            _ => None,
        };
        if let Some(d) = dir {
            self.signal_last_dir.insert(id, d);
        }
    }

    pub(crate) fn write_lvalue(
        &mut self,
        lvalue: &IrLValue,
        mut val: LogicVec,
        is_blocking: bool,
    ) -> Result<(), SimError> {
        // Check for const violation
        if let Some(id) = self.signal_id_from_lvalue(lvalue) {
            if let Some(sig) = self.design.top.signals.get(id) {
                if sig.is_const {
                    return Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::DpiError,
                        format!("cannot write to const signal '{}'", sig.name),
                    ));
                }
            }
        }
        // ── Forced override ($assign): tahan write sampai $deassign ──
        // $deposit TIDAK masuk map ini — ia sekali tulis dan bisa ditimpa driver.
        if let Some(id) = self.signal_id_from_lvalue(lvalue) {
            if self.forced_signals.contains(&id) {
                return Ok(());
            }
        }

        // ── Race detection: Write-Write check ──
        // Cegah false positive untuk multi-driver nets yang intentional
        if let Some(id) = self.signal_id_from_lvalue(lvalue) {
            let is_multi_driver = self
                .design
                .top
                .signals
                .get(id)
                .map(|s| s.multi_driver)
                .unwrap_or(false);
            if !is_multi_driver {
                // Check if a DIFFERENT process already wrote this signal in this delta
                if let Some(existing_writer) = self.signal_writers.get(&id) {
                    if let (Some(current_pid), Some(prev_pid)) =
                        (self.current_process_id, existing_writer)
                    {
                        if current_pid != *prev_pid {
                            let sig_name = self
                                .design
                                .top
                                .signals
                                .get(id)
                                .map(|s| s.name.as_str())
                                .unwrap_or("<unknown>");
                            self.emit_warning(
                            mivon_core::diagnostics::DiagCode::SignalContention,
                                format!(
                                    "race condition: signal '{}' written by multiple processes in same delta cycle",
                                    sig_name
                                ),
                            );
                        }
                    }
                }
                // Track this write
                self.signal_writers.insert(id, self.current_process_id);
                // SIM-12: track write type (blocking = true)
                // If same signal was written by non-blocking (false) in same delta → mixed race
                if let Some(&prev_is_blocking) = self.signal_write_types.get(&id) {
                    if prev_is_blocking != is_blocking {
                        let sig_name = self
                            .design
                            .top
                            .signals
                            .get(id)
                            .map(|s| s.name.as_str())
                            .unwrap_or("<unknown>");
                        self.emit_warning(
                            mivon_core::diagnostics::DiagCode::NbaWriteConflict,
                            format!(
                                "race condition: signal '{}' has mixed blocking/non-blocking writes in same delta cycle",
                                sig_name
                            ),
                        );
                    }
                }
                self.signal_write_types.insert(id, is_blocking);
            }
            // Track oscillation (write count)
            *self.signal_write_count.entry(id).or_insert(0) += 1;
        }

        match lvalue {
            IrLValue::Signal(id, _) => {
                // ── Glitch detection (SIM-23): pulse A→B→A dalam glitch_window ──
                // Hanya untuk full-signal write (bukan RangeSelect/BitSelect partial),
                // supaya update bit parsial tidak memicu false positive. Skip untuk
                // multi-driver nets: nilai yang benar-benar ter-commit adalah hasil
                // resolve_net_values(), bukan val mentah → glitch bisa false positive.
                if self.glitch_window > 0 {
                    let is_multi_driver = self
                        .design
                        .top
                        .signals
                        .get(*id)
                        .map(|s| s.multi_driver)
                        .unwrap_or(false);
                    if !is_multi_driver {
                        let old = self.state.read_signal(*id).clone();
                        if old.to_u64() != val.to_u64() {
                            if let Some(&(t_prev, ref val_before)) = self.glitch_prev.get(id) {
                                let dt = self.state.time.saturating_sub(t_prev);
                                if dt <= self.glitch_window && val.to_u64() == val_before.to_u64() {
                                    let sig_name = self
                                        .design
                                        .top
                                        .signals
                                        .get(*id)
                                        .map(|s| s.name.as_str())
                                        .unwrap_or("<unknown>");
                                    self.emit_warning(
                                        mivon_core::diagnostics::DiagCode::SignalGlitch,
                                        format!(
                                            "glitch detected on signal '{}' at time {}: value reverted within {} time units",
                                            sig_name, self.state.time, dt
                                        ),
                                    );
                                }
                            }
                            self.glitch_prev.insert(*id, (self.state.time, old));
                        }
                    }
                }

                sanitize_for_2state(&self.design.top.signals, *id, &mut val);
                let is_str = self
                    .design
                    .top
                    .signals
                    .get(*id)
                    .map(|s| s.is_string)
                    .unwrap_or(false);
                let sig_info = self.design.top.signals.get(*id).cloned();
                let is_dyn = sig_info
                    .as_ref()
                    .map(|s| s.is_dynamic || s.is_queue)
                    .unwrap_or(false);
                let resized = if is_str || is_dyn {
                    val
                } else {
                    let target_width = self.state.read_signal(*id).width;
                    if val.width != target_width {
                        val.resize(target_width)
                    } else {
                        val
                    }
                };
                // ── SDF delay (WAV-13): signal ber-annotasi delay dari
                // `annotate_sdf` (`IrSignal.delay_rise/delay_fall`, ps) — commit
                // ditunda ke t+delay via event terjadwal (rise utk 0→1, fall utk
                // 1→0, min utk transisi lain). Sebelumnya delay disimpan tapi
                // TIDAK pernah dibaca → SDF tidak mempengaruhi timing sim.
                if let Some(sig) = self.design.top.signals.get(*id) {
                    if sig.delay_rise.is_some() || sig.delay_fall.is_some() {
                        let old = self.state.read_signal(*id).clone();
                        if old.bits != resized.bits {
                            let delay_ps = match (old.bits.first(), resized.bits.first()) {
                                (Some(LogicVal::Zero), Some(LogicVal::One)) => {
                                    sig.delay_rise.unwrap_or(0)
                                }
                                (Some(LogicVal::One), Some(LogicVal::Zero)) => {
                                    sig.delay_fall.unwrap_or(0)
                                }
                                _ => sig.delay_rise.unwrap_or(0).min(sig.delay_fall.unwrap_or(0)),
                            };
                            let delay_units = self.sdf_ps_to_time_units(delay_ps);
                            if delay_units > 0 {
                                let t = self.state.time + delay_units;
                                self.push_event(
                                    t as usize,
                                    RegionEvent {
                                        region: EventRegion::Active,
                                        event: EventKind::SdfDelayedWrite {
                                            sig_id: *id,
                                            value: resized,
                                        },
                                    },
                                );
                                return Ok(());
                            }
                        }
                    }
                }
                // Apply resolution for multi-driver nets
                if let Some(ref info) = sig_info {
                    if info.multi_driver
                        && matches!(
                            info.kind,
                            SignalKind::Wire | SignalKind::Inout | SignalKind::Output
                        )
                    {
                        let current = self.state.read_signal(*id).clone();
                        let resolved = resolve_net_values(info.net_type, &current, &resized);
                        self.state.write_signal(*id, resolved);
                        return Ok(());
                    }
                }
                let old_val = self.state.read_signal(*id).clone();
                self.state.write_signal(*id, resized.clone());
                self.record_signal_change(*id, &old_val, &resized);
            }
            IrLValue::RangeSelect(sig_id, msb, lsb) => {
                sanitize_for_2state(&self.design.top.signals, *sig_id, &mut val);
                let (start, end) = if *msb > *lsb {
                    (*lsb, *msb)
                } else {
                    (*msb, *lsb)
                };
                // Array memori flat ultra-lebar: tulis segmen langsung (tanpa
                // RMW penuh = clone 1e9 bit → OOM).
                if self.write_partial_if_wide(*sig_id, start, end + 1, &val) {
                    return Ok(());
                }
                let mut existing = self.state.read_signal(*sig_id).clone();
                // ══ Bounds check: pastikan range tidak melebihi signal width ══
                if end >= existing.bits.len() {
                    let sig_name = self
                        .design
                        .top
                        .signals
                        .get(*sig_id)
                        .map(|s| s.name.as_str())
                        .unwrap_or("<unknown>");
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::MemoryOutOfBounds,
                        format!(
                            "RangeSelect out of bounds: signal '{}' [{}:{}] exceeds width {}",
                            sig_name,
                            msb,
                            lsb,
                            existing.bits.len()
                        ),
                    );
                    // Clamp to signal width to prevent panic
                    let max_end = existing.bits.len().saturating_sub(1);
                    let end = end.min(max_end);
                    for (i, b) in val.bits.iter().enumerate() {
                        if start + i <= end && start + i < existing.bits.len() {
                            existing.bits[start + i] = *b;
                        }
                    }
                } else {
                    for (i, b) in val.bits.iter().enumerate() {
                        if start + i <= end {
                            existing.bits[start + i] = *b;
                        }
                    }
                }
                let old_val = self.state.read_signal(*sig_id).clone();
                self.state.write_signal(*sig_id, existing.clone());
                self.record_signal_change(*sig_id, &old_val, &existing);
            }
            IrLValue::BitSelect(sig_id, idx) => {
                sanitize_for_2state(&self.design.top.signals, *sig_id, &mut val);
                // Array memori flat ultra-lebar: tulis 1 bit langsung.
                if self.write_partial_if_wide(*sig_id, *idx, *idx + 1, &val) {
                    return Ok(());
                }
                let mut existing = self.state.read_signal(*sig_id).clone();
                if let Some(b) = val.bits.first() {
                    if *idx < existing.bits.len() {
                        existing.bits[*idx] = *b;
                    }
                }
                let old_val = self.state.read_signal(*sig_id).clone();
                self.state.write_signal(*sig_id, existing.clone());
                self.record_signal_change(*sig_id, &old_val, &existing);
            }
            IrLValue::ArrayIndex {
                sig_id,
                index,
                elem_width,
            } => {
                let key_val = self.evaluate_expr(index)?;
                // Check if this is an associative array
                let sig_info = self.design.top.signals.get(*sig_id);
                if sig_info.map(|s| s.is_associative).unwrap_or(false) {
                    sanitize_for_2state(&self.design.top.signals, *sig_id, &mut val);
                    let assoc_map = self.assoc_data.entry(*sig_id).or_default();
                    assoc_map.insert(key_val, val);
                    return Ok(());
                }
                sanitize_for_2state(&self.design.top.signals, *sig_id, &mut val);
                let idx = key_val.to_u64() as usize;
                let start = idx * elem_width;
                let needed = start + elem_width;
                // ══ Bounds check: warning untuk fixed-size array out-of-bounds ══
                let is_dynamic = sig_info
                    .map(|s| s.is_dynamic || s.is_queue)
                    .unwrap_or(false);
                // Array memori flat ultra-lebar & index inside → tulis segmen
                // langsung (tanpa RMW penuh = clone 1e9 bit → OOM).
                let wide = self
                    .design
                    .top
                    .signals
                    .get(*sig_id)
                    .map(|s| s.width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD)
                    .unwrap_or(false);
                if wide && !is_dynamic {
                    // Array memori flat fixed: tulis segmen langsung; bounds
                    // di-clamp aman di write_signal_slice (LRM OOB = no-op).
                    self.state
                        .write_signal_slice(*sig_id, start, start + elem_width, &val);
                    self.signal_last_change.insert(*sig_id, self.state.time);
                    return Ok(());
                }
                let mut existing = self.state.read_signal(*sig_id).clone();
                // DEBUG Bug-1 (array 1-bit unpacked miscalc elem_width) —
                // DBG_ARRIDX=1: tampilkan dimensi & elem_width runtime.
                if std::env::var("DBG_ARRIDX").is_ok() {
                    if let Some(si) = self.design.top.signals.get(*sig_id) {
                        eprintln!(
                            "[ARRIDX] {} idx={} ew_lval={} sig.width={} sig.elem_width={} array_depth={} dims={:?}",
                            si.name.as_str(), idx, elem_width, si.width, si.elem_width,
                            si.array_depth, si.array_dims
                        );
                    }
                }
                if needed > existing.width && !is_dynamic {
                    let sig_name = self
                        .design
                        .top
                        .signals
                        .get(*sig_id)
                        .map(|s| s.name.as_str())
                        .unwrap_or("<unknown>");
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::MemoryOutOfBounds,
                        format!("ArrayIndex out of bounds: '{}' index {} exceeds array size (needed {}b, signal width {}b)",
                            sig_name, idx, needed, existing.width),
                    );
                }
                // ══ Guard index negatif/absurd: resize raksasa = hang/OOM ══
                // (mivon-fuzz `r[-1] = ...`: `-1` → to_u64() = 0xFFFFFFFF →
                // start = idx*elem_width ~34Gb → resize men-jam -> hang).
                // Dynamic/queue array tetap GROW (size runtime sah legal);
                // Fixed array DIGROW untuk OOB kecil-dekat-batas (pola lama
                // lenient, mis. `r[5]` pada [0:2] → 2x) TAPI hanya sampai
                // batas wajar; induk di luar itu diabaikan (LRM OOB = no-op).
                if needed > existing.width {
                    let can_grow = is_dynamic
                        || needed <= existing.width.saturating_mul(4).max(elem_width * 2);
                    if !can_grow {
                        // OOB ekstrem (negatif / absurd) — abai tulis.
                        return Ok(());
                    }
                    existing.bits.resize(needed, LogicVal::X);
                    existing.width = needed;
                }
                for (i, b) in val.bits.iter().enumerate() {
                    if start + i < needed {
                        existing.bits[start + i] = *b;
                    }
                }
                self.state.write_signal(*sig_id, existing);
                self.signal_last_change.insert(*sig_id, self.state.time);
            }
            IrLValue::ArrayRangeSelect {
                sig_id,
                index,
                elem_width,
                msb,
                lsb,
            } => {
                let idx_val = self.evaluate_expr(index)?;
                let idx = idx_val.to_u64() as usize;
                let base = idx * elem_width;
                let (start, end) = if *msb > *lsb {
                    (*lsb, *msb)
                } else {
                    (*msb, *lsb)
                };
                // Array memori flat ultra-lebar: tulis segmen langsung.
                if self.write_partial_if_wide(*sig_id, base + start, base + end + 1, &val) {
                    return Ok(());
                }
                let mut existing = self.state.read_signal(*sig_id).clone();
                // ══ Bounds check: pastikan base+end tidak melebihi signal width ══
                if base + end >= existing.bits.len() {
                    let sig_name = self
                        .design
                        .top
                        .signals
                        .get(*sig_id)
                        .map(|s| s.name.as_str())
                        .unwrap_or("<unknown>");
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::MemoryOutOfBounds,
                        format!("ArrayRangeSelect out of bounds: signal '{}' index {} -> abs[{}:{}] exceeds width {}",
                            sig_name, idx, base + start, base + end, existing.bits.len()),
                    );
                    // Clamp to bounds to prevent panic
                    let max_end = existing.bits.len().saturating_sub(1).saturating_sub(base);
                    let end = end.min(max_end);
                    let abs_start = base + start;
                    for (i, b) in val.bits.iter().enumerate() {
                        if abs_start + i <= base + end && abs_start + i < existing.bits.len() {
                            existing.bits[abs_start + i] = *b;
                        }
                    }
                } else {
                    let abs_start = base + start;
                    for (i, b) in val.bits.iter().enumerate() {
                        if abs_start + i <= base + end {
                            existing.bits[abs_start + i] = *b;
                        }
                    }
                }
                let is_init = self.state.read_signal(*sig_id).all_x()
                    || self.state.read_signal(*sig_id).all_z();
                let old_val = self.state.read_signal(*sig_id).clone();
                self.state.write_signal(*sig_id, existing.clone());
                if !is_init {
                    self.record_signal_change(*sig_id, &old_val, &existing);
                }
            }
            IrLValue::ArrayBitSelect {
                sig_id,
                index,
                elem_width,
                bit,
            } => {
                let idx_val = self.evaluate_expr(index)?;
                let idx = idx_val.to_u64() as usize;
                // bit bisa dinamis (`arr[i][j]` dengan j runtime) maupun
                // konstanta — evaluasi keduanya seragam.
                let bit_val = self.evaluate_expr(bit)?;
                let bit = bit_val.to_u64() as usize;
                let abs_idx = idx * elem_width + bit;
                // Array memori flat ultra-lebar: tulis 1 bit langsung.
                if self.write_partial_if_wide(*sig_id, abs_idx, abs_idx + 1, &val) {
                    return Ok(());
                }
                let mut existing = self.state.read_signal(*sig_id).clone();
                if let Some(b) = val.bits.first() {
                    if abs_idx < existing.bits.len() {
                        existing.bits[abs_idx] = *b;
                    }
                }
                let old_val = self.state.read_signal(*sig_id).clone();
                self.state.write_signal(*sig_id, existing.clone());
                self.record_signal_change(*sig_id, &old_val, &existing);
            }
            // Dynamic indexed part-select lvalue: `sig[base +: width]` dengan
            // base runtime (mis. `packed_data_d[word_sel*BusWidth +: BusWidth]`).
            // Base dievaluasi saat write; lebar sudah di-resolve saat elaborasi.
            IrLValue::ExprPartSelect {
                sig_id,
                base,
                width,
            } => {
                sanitize_for_2state(&self.design.top.signals, *sig_id, &mut val);
                let base_val = self.evaluate_expr(base)?;
                let start = base_val.to_u64() as usize;
                let w = *width;
                // Array memori flat ultra-lebar: tulis segmen langsung.
                if self.write_partial_if_wide(*sig_id, start, start + w, &val) {
                    return Ok(());
                }
                let mut existing = self.state.read_signal(*sig_id).clone();
                // Bounds check: peringatan + clamp agar tidak panic.
                if start + w > existing.bits.len() {
                    let sig_name = self
                        .design
                        .top
                        .signals
                        .get(*sig_id)
                        .map(|s| s.name.as_str())
                        .unwrap_or("<unknown>");
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::MemoryOutOfBounds,
                        format!(
                            "ExprPartSelect out of bounds: signal '{}' [{} +: {}] exceeds width {}",
                            sig_name,
                            start,
                            w,
                            existing.bits.len()
                        ),
                    );
                }
                for (i, b) in val.bits.iter().enumerate() {
                    let abs = start + i;
                    if i < w && abs < existing.bits.len() {
                        existing.bits[abs] = *b;
                    }
                }
                let old_val = self.state.read_signal(*sig_id).clone();
                self.state.write_signal(*sig_id, existing.clone());
                self.record_signal_change(*sig_id, &old_val, &existing);
            }
            IrLValue::Concat(parts) => {
                // LRM 1800-2017 §10.7: assignment ke concat lvalue — RHS
                // di-zero-extend ke lebar total concat, lalu dibagikan
                // MSB-first (part PERTAMA = bit paling tinggi). Handler lama
                // mengiris LSB-first (offset naik dari 0) sehingga
                // `{co, s} = ci + a + b` membalik bit (co dapat LSB) dan
                // mengisi X saat RHS lebih sempit dari total concat.
                let total: usize = parts.iter().map(|p| self.get_lvalue_width(p)).sum();
                let mut bits = val.bits.clone();
                if bits.len() < total {
                    bits.resize(total, LogicVal::Zero);
                } else if bits.len() > total {
                    bits.truncate(total);
                }
                let mut offset = total;
                for part in parts {
                    let w = self.get_lvalue_width(part);
                    offset -= w;
                    let sub_val = LogicVec {
                        bits: bits[offset..offset + w].to_vec(),
                        width: w,
                    };
                    self.write_lvalue(part, sub_val, is_blocking)?;
                }
            }
            IrLValue::ObjectField { sig_id, field } => {
                // Class object field write: obj signal → handle → get_object → fields.
                sanitize_for_2state(&self.design.top.signals, *sig_id, &mut val);
                let handle = self.state.read_signal(*sig_id);
                let obj_id = handle.to_u64() as ObjId;
                if let Some(obj) = self.state.get_object_mut(obj_id) {
                    obj.fields.insert(*field, val);
                }
            }
            // Lvalue hierarkis (nama di flattened signal list — mis. signal
            // interface instance `sif.csb`). Resolve nama → SignalId lalu
            // dispatch ulang ke write penuh (const check, race, resize, dll).
            IrLValue::HierRef(name) => {
                let Some(sig_id) = self.find_signal(name.as_str()) else {
                    return Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::UndefinedSignal,
                        format!("hierarchical signal '{}' not found for write", name),
                    ));
                };
                self.write_lvalue(&IrLValue::Signal(sig_id, 0), val, is_blocking)?;
            }
            // Seleksi bit/index pada lvalue hierarkis: `sif.sd_out[i]`.
            // Lebar elemen ditentukan runtime dari SignalInfo: array unpacked
            // (array_depth > 0) → offset = index * elem_width, tulis sebesar
            // elem_width; sinyal flat → offset = index (1 bit).
            IrLValue::HierRefIndex { name, index } => {
                let Some(sig_id) = self.find_signal(name.as_str()) else {
                    return Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::UndefinedSignal,
                        format!("hierarchical signal '{}' not found for write", name),
                    ));
                };
                let idx_val = self.evaluate_expr(index)?;
                let idx = idx_val.to_u64() as usize;
                let sig = &self.design.top.signals[sig_id];
                // `[i]` pada `logic [3:0] x` (array_depth==1) adalah BIT select;
                // baru word select bila unpacked array (array_depth > 1) —
                // konsisten dengan elaborate_lvalue BitSelect untuk signal biasa.
                let (elem_width, word_sel) = if sig.array_depth > 1 {
                    (sig.elem_width.max(1), idx)
                } else {
                    (1usize, idx)
                };
                if std::env::var("MIVON_DBG_HIERWR").is_ok() {
                    eprintln!(
                        "[DBG-HIERWR] '{}' idx={} array_depth={} elem_w={} sig_w={} val={}",
                        name.as_str(),
                        idx,
                        sig.array_depth,
                        sig.elem_width,
                        sig.width,
                        val.to_u64()
                    );
                }
                let lsb = word_sel.saturating_mul(elem_width);
                let write_w = val.width.min(elem_width);
                // Tulis bit [lsb .. lsb+write_w) dari signal.
                let mut existing = self.state.read_signal(sig_id).clone();
                sanitize_for_2state(&self.design.top.signals, sig_id, &mut val);
                for (i, b) in val.bits.iter().take(write_w).enumerate() {
                    let pos = lsb + i;
                    if pos < existing.bits.len() {
                        existing.bits[pos] = *b;
                    }
                }
                let old_val = self.state.read_signal(sig_id).clone();
                self.state.write_signal(sig_id, existing.clone());
                self.record_signal_change(sig_id, &old_val, &existing);
            }
        }
        Ok(())
    }

    pub(crate) fn get_lvalue_width(&self, lvalue: &IrLValue) -> usize {
        match lvalue {
            IrLValue::Signal(id, _) => self.state.read_signal(*id).width,
            IrLValue::RangeSelect(_, msb, lsb) => {
                if *msb > *lsb {
                    msb - lsb + 1
                } else {
                    lsb - msb + 1
                }
            }
            IrLValue::BitSelect(_, _) => 1,
            IrLValue::ArrayIndex { elem_width, .. } => *elem_width,
            IrLValue::ArrayRangeSelect { msb, lsb, .. } => {
                if *msb > *lsb {
                    msb - lsb + 1
                } else {
                    lsb - msb + 1
                }
            }
            IrLValue::ArrayBitSelect { .. } => 1,
            IrLValue::ExprPartSelect { width, .. } => *width,
            IrLValue::ObjectField { .. } => 64,
            IrLValue::HierRef(name) => self
                .find_signal(name.as_str())
                .and_then(|id| self.design.top.signals.get(id))
                .map(|s| s.width)
                .unwrap_or(1),
            IrLValue::HierRefIndex { name, .. } => self
                .find_signal(name.as_str())
                .and_then(|id| self.design.top.signals.get(id))
                .map(|s| {
                    if s.array_depth > 1 {
                        s.elem_width.max(1)
                    } else {
                        1
                    }
                })
                .unwrap_or(1),
            IrLValue::Concat(parts) => parts.iter().map(|p| self.get_lvalue_width(p)).sum(),
        }
    }

    pub(crate) fn get_local(&self, name: &str) -> Option<LogicVec> {
        for scope in self.method_locals.iter().rev() {
            if let Some(v) = scope.get(&Symbol::intern(name)) {
                return Some(v.clone());
            }
        }
        None
    }

    pub(crate) fn set_local(&mut self, name: &str, val: LogicVec) {
        if let Some(scope) = self.method_locals.last_mut() {
            scope.insert(Symbol::intern(name), val);
        }
    }

    pub(crate) fn write_ast_lvalue(
        &mut self,
        lhs: &mivon_ast::Expr,
        val: LogicVec,
    ) -> Result<(), SimError> {
        match lhs {
            mivon_ast::Expr::Ident { name, .. } => self.write_local_or_field(name.as_str(), val),
            mivon_ast::Expr::MemberAccess { obj, field } => {
                let obj_val = self.evaluate_ast_expr(obj)?;
                let obj_id = obj_val.to_u64() as ObjId;
                if let Some(obj_data) = self.state.get_object_mut(obj_id) {
                    obj_data.fields.insert(*field, val);
                    Ok(())
                } else {
                    Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::NullHandle,
                        format!("object {} not found for field '{}'", obj_id, field),
                    ))
                }
            }
            mivon_ast::Expr::BitSelect { expr: inner, index } => {
                // `arr[idx] = rhs` — array element (array field/queue) ATAU
                // bit select pada vector. Konsisten dgn evaluate_ast_stmt.
                let idx_val = self.evaluate_ast_expr(index)?;
                let idx = idx_val.to_u64() as usize;
                if let Some(elem_width) = self.get_field_elem_width(inner) {
                    let lhs_val = self.evaluate_ast_expr(inner)?;
                    let mut bits = lhs_val.bits.clone();
                    let start = idx * elem_width;
                    for (j, b) in val.bits.iter().enumerate() {
                        if start + j < bits.len() {
                            bits[start + j] = *b;
                        }
                    }
                    let new_val = LogicVec {
                        width: bits.len(),
                        bits,
                    };
                    match inner.as_ref() {
                        mivon_ast::Expr::Ident { name, .. } => {
                            self.write_local_or_field(name.as_str(), new_val)
                        }
                        mivon_ast::Expr::MemberAccess { obj, field } => {
                            let ov = self.evaluate_ast_expr(obj)?;
                            let oid = ov.to_u64() as ObjId;
                            if let Some(o) = self.state.get_object_mut(oid) {
                                o.fields.insert(*field, new_val);
                                Ok(())
                            } else {
                                Err(self.diag_error(
                                    mivon_core::diagnostics::DiagCode::NullHandle,
                                    format!("object {} not found for field write", oid),
                                ))
                            }
                        }
                        _ => Err(self.diag_error(
                            mivon_core::diagnostics::DiagCode::NotImplemented,
                            format!("unsupported array base in task method: {:?}", inner),
                        )),
                    }
                } else {
                    let lhs_val = self.evaluate_ast_expr(inner)?;
                    let mut bits = lhs_val.bits.clone();
                    if idx < bits.len() {
                        let bit = val.bits.first().copied().unwrap_or(LogicVal::X);
                        bits[idx] = bit;
                    }
                    let width = bits.len();
                    let new_val = LogicVec { width, bits };
                    match inner.as_ref() {
                        mivon_ast::Expr::Ident { name, .. } => {
                            self.write_local_or_field(name.as_str(), new_val)
                        }
                        mivon_ast::Expr::MemberAccess { obj, field } => {
                            let ov = self.evaluate_ast_expr(obj)?;
                            let oid = ov.to_u64() as ObjId;
                            if let Some(o) = self.state.get_object_mut(oid) {
                                o.fields.insert(*field, new_val);
                                Ok(())
                            } else {
                                Err(self.diag_error(
                                    mivon_core::diagnostics::DiagCode::NullHandle,
                                    format!("object {} not found for field write", oid),
                                ))
                            }
                        }
                        _ => Err(self.diag_error(
                            mivon_core::diagnostics::DiagCode::NotImplemented,
                            format!("unsupported bit base in task method: {:?}", inner),
                        )),
                    }
                }
            }
            mivon_ast::Expr::RangeSelect {
                expr: inner,
                msb,
                lsb,
            } => {
                let lhs_val = self.evaluate_ast_expr(inner)?;
                let msb_val = self.evaluate_ast_expr(msb)?;
                let lsb_val = self.evaluate_ast_expr(lsb)?;
                let m = msb_val.to_u64() as usize;
                let l = lsb_val.to_u64() as usize;
                let (start, end) = if m > l { (l, m) } else { (m, l) };
                let range_len = end - start + 1;
                let mut bits = lhs_val.bits.clone();
                for j in 0..val.width.min(range_len) {
                    if start + j < bits.len() {
                        bits[start + j] = val.bits[j];
                    }
                }
                let new_val = LogicVec {
                    width: bits.len(),
                    bits,
                };
                match inner.as_ref() {
                    mivon_ast::Expr::Ident { name, .. } => {
                        self.write_local_or_field(name.as_str(), new_val)
                    }
                    mivon_ast::Expr::MemberAccess { obj, field } => {
                        let ov = self.evaluate_ast_expr(obj)?;
                        let oid = ov.to_u64() as ObjId;
                        if let Some(o) = self.state.get_object_mut(oid) {
                            o.fields.insert(*field, new_val);
                            Ok(())
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::NullHandle,
                                format!("object {} not found for field write", oid),
                            ))
                        }
                    }
                    _ => Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::NotImplemented,
                        format!("unsupported range base in task method: {:?}", inner),
                    )),
                }
            }
            mivon_ast::Expr::PartSelect {
                expr: inner,
                base,
                width,
            } => {
                // `inner[base +: width]` — parser sudah menormalisasi `-:` ke
                // base = msb-(width-1), jadi base selalu indeks BAWAH.
                let base_val = self.evaluate_ast_expr(base)?;
                let width_val = self.evaluate_ast_expr(width)?;
                let b = base_val.to_u64() as usize;
                let w = width_val.to_u64() as usize;
                let lhs_val = self.evaluate_ast_expr(inner)?;
                let mut bits = lhs_val.bits.clone();
                for j in 0..val.width.min(w.max(1)) {
                    if b + j < bits.len() {
                        bits[b + j] = val.bits[j];
                    }
                }
                let new_val = LogicVec {
                    width: bits.len(),
                    bits,
                };
                match inner.as_ref() {
                    mivon_ast::Expr::Ident { name, .. } => {
                        self.write_local_or_field(name.as_str(), new_val)
                    }
                    mivon_ast::Expr::MemberAccess { obj, field } => {
                        let ov = self.evaluate_ast_expr(obj)?;
                        let oid = ov.to_u64() as ObjId;
                        if let Some(o) = self.state.get_object_mut(oid) {
                            o.fields.insert(*field, new_val);
                            Ok(())
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::NullHandle,
                                format!("object {} not found for field write", oid),
                            ))
                        }
                    }
                    _ => Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::NotImplemented,
                        format!("unsupported part-select base in task method: {:?}", inner),
                    )),
                }
            }
            _ => Err(self.diag_error(
                mivon_core::diagnostics::DiagCode::NotImplemented,
                format!("unsupported lvalue type in task method: {:?}", lhs),
            )),
        }
    }

    pub(crate) fn ast_lvalue_to_ir(&self, lhs: &mivon_ast::Expr) -> Option<IrLValue> {
        match lhs {
            mivon_ast::Expr::Ident { name, .. } => {
                let sig_id = self.find_signal(name.as_str())?;
                Some(IrLValue::Signal(sig_id, 0))
            }
            _ => None,
        }
    }

    pub(crate) fn find_ast_signal_id(&self, expr: &mivon_ast::Expr) -> Option<SignalId> {
        match expr {
            mivon_ast::Expr::Ident { name, .. } => self.find_signal(name.as_str()),
            // F27: `@(posedge b.clk)` di task/method — b.clk adalah field port
            // interface; resolve hier path via hier_signal_map (design sudah
            // di-flatten saat runtime).
            mivon_ast::Expr::MemberAccess { obj, field } => {
                let hier = Self::build_hier_name(obj, field.as_str());
                if hier.is_empty() {
                    None
                } else {
                    self.design
                        .hier_signal_map
                        .get(&Symbol::intern(&hier))
                        .copied()
                }
            }
            _ => None,
        }
    }

    pub(crate) fn write_local_or_field(
        &mut self,
        name: &str,
        val: LogicVec,
    ) -> Result<(), SimError> {
        if self.get_local(name).is_some() {
            self.set_local(name, val);
            return Ok(());
        }
        if let Some(obj_id) = self.current_this {
            if let Some(obj) = self.state.get_object_mut(obj_id) {
                obj.fields.insert(Symbol::intern(name), val);
                return Ok(());
            }
        }
        // Nama tak bisa di-resolve sebagai local atau field (mis. variabel yang
        // dideklarasikan di dalam blok bersarang seperti fork/begin-end yang
        // tidak terdaftar di scope method). Daripada mematikan seluruh simulasi,
        // degradasi ke local auto (seperti pembacaan yang sudah warning + default
        // di evaluate_ast_expr) agar run tetap selesai.
        if self.method_locals.is_empty() {
            self.method_locals.push(HashMap::new());
        }
        if let Some(scope) = self.method_locals.last_mut() {
            scope.insert(Symbol::intern(name), val);
        }
        self.diag_warn_at(
            mivon_core::diagnostics::DiagCode::NullHandle,
            format!("cannot resolve '{}' as local or field; creating implicit local (declared in nested block?)", name),
            0,
            0,
        );
        Ok(())
    }

    /// WAV-13: konversi delay SDF (ps) ke time unit desain (`state.time`).
    /// `design.timescale` = (unit, precision) mis. ("1ns", "1ps") → unit
    /// exponent -9 → 1 unit = 1ns → 1000ps = 1 unit. Tanpa timescale, asumsi
    /// default 1ns (sama dengan `TimeFormat::default`).
    fn sdf_ps_to_time_units(&self, ps: u64) -> u64 {
        let exp = self
            .design
            .timescale
            .as_ref()
            .and_then(|(unit, _)| crate::simulator::types::TimeFormat::unit_exponent(unit))
            .unwrap_or(-9);
        if exp >= -12 {
            // Unit >= ps (ps, ns, us, ...): bagi 10^(12+exp).
            ps / 10u64.pow((12 + exp) as u32)
        } else {
            // Unit < ps (fs): kali 10^(-(12+exp)).
            ps.saturating_mul(10u64.pow((-12 - exp) as u32))
        }
    }

    /// WAV-13: commit write tertunda dari event `EventKind::SdfDelayedWrite` —
    /// sanitize + resize + resolusi multi-driver + write + record_signal_change
    /// (sama dengan jalur write langsung di `write_lvalue`, minus race/glitch/
    /// force — commit terjadi dari event handler, bukan dari proses).
    pub(crate) fn commit_delayed_signal_write(
        &mut self,
        sig_id: usize,
        mut val: LogicVec,
    ) -> Result<(), SimError> {
        sanitize_for_2state(&self.design.top.signals, sig_id, &mut val);
        let sig_info = self.design.top.signals.get(sig_id).cloned();
        let is_str = sig_info.as_ref().map(|s| s.is_string).unwrap_or(false);
        let is_dyn = sig_info
            .as_ref()
            .map(|s| s.is_dynamic || s.is_queue)
            .unwrap_or(false);
        let resized = if is_str || is_dyn {
            val
        } else {
            let target_width = self.state.read_signal(sig_id).width;
            if val.width != target_width {
                val.resize(target_width)
            } else {
                val
            }
        };
        if let Some(ref info) = sig_info {
            if info.multi_driver
                && (info.kind == SignalKind::Wire || info.kind == SignalKind::Inout)
            {
                let current = self.state.read_signal(sig_id).clone();
                let resolved = resolve_net_values(info.net_type, &current, &resized);
                self.state.write_signal(sig_id, resolved);
                return Ok(());
            }
        }
        let old_val = self.state.read_signal(sig_id).clone();
        self.state.write_signal(sig_id, resized.clone());
        self.record_signal_change(sig_id, &old_val, &resized);
        Ok(())
    }
}
