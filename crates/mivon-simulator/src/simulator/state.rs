use mivon_ir::{IrDesign, IrModule, LogicVec, ObjId, ObjectData, SignalId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Per-signal delay information from SDF annotation.
#[derive(Debug, Clone)]
pub struct SignalDelay {
    pub rise: u64, // rise delay in ps
    pub fall: u64, // fall delay in ps
}

/// Cache chunk size: 1024 bit per chunk (`[u64;32]`, 32 cell/u64 × 2-bit).
const WIDE_CHUNK_BITS: usize = 1024;

/// Penyimpanan packed utk array memori raksasa (miliaran bit). Tiap cell
/// 2-bit: X=0, Zero=1, One=2, Z=3. Hanya chunk yang pernah ditulis yang
/// dimuat — jumlah memori proporsional dengan akses nyata, bukan lebar.
#[derive(Debug, Clone, Default)]
pub struct WideMem {
    chunks: HashMap<usize, [u64; 32]>,
}

impl WideMem {
    #[inline]
    pub fn get(&self, bit: usize) -> mivon_core::LogicVal {
        let (ci, cell) = (bit / WIDE_CHUNK_BITS, (bit % WIDE_CHUNK_BITS) / 32);
        let off = ((bit % WIDE_CHUNK_BITS) % 32) * 2;
        let v = ((self.chunks.get(&ci).map(|c| c[cell]).unwrap_or(0)) >> off) & 0b11;
        match v {
            0 => mivon_core::LogicVal::X,
            1 => mivon_core::LogicVal::Zero,
            2 => mivon_core::LogicVal::One,
            _ => mivon_core::LogicVal::Z,
        }
    }

    #[inline]
    pub fn set(&mut self, bit: usize, val: mivon_core::LogicVal) {
        let (ci, cell) = (bit / WIDE_CHUNK_BITS, (bit % WIDE_CHUNK_BITS) / 32);
        let off = ((bit % WIDE_CHUNK_BITS) % 32) * 2;
        let cellv = match val {
            mivon_core::LogicVal::X => 0,
            mivon_core::LogicVal::Zero => 1,
            mivon_core::LogicVal::One => 2,
            mivon_core::LogicVal::Z => 3,
        };
        let c = self.chunks.entry(ci).or_insert([0u64; 32]);
        c[cell] = (c[cell] & !(0b11u64 << off)) | (cellv << off);
    }

    /// Baca blok `start..(start+width)` teroptimasi: slot sejajar 32-bit
    /// (umum: elem 512/64) = per-chunk copy; lainnya fallback per-bit `get`.
    /// 512-bit slot = 16 chunk-shift vs 512 `get` sebelumnya.
    pub fn read_block(&self, start: usize, width: usize) -> Vec<mivon_core::LogicVal> {
        let mut out = Vec::with_capacity(width);
        if width == 0 {
            return out;
        }
        let last = start + width - 1;
        let aligned = start.is_multiple_of(32)
            && width.is_multiple_of(32)
            && start / WIDE_CHUNK_BITS == last / WIDE_CHUNK_BITS;
        if aligned {
            let ci = start / WIDE_CHUNK_BITS;
            let cell0 = (start % WIDE_CHUNK_BITS) / 32;
            let ncell = width / 32;
            let src = self.chunks.get(&ci).copied().unwrap_or([0u64; 32]);
            for k in 0..ncell {
                let w = src[cell0 + k];
                for jj in 0..32 {
                    out.push(match (w >> (jj * 2)) & 0b11 {
                        0 => mivon_core::LogicVal::X,
                        1 => mivon_core::LogicVal::Zero,
                        2 => mivon_core::LogicVal::One,
                        _ => mivon_core::LogicVal::Z,
                    });
                }
            }
        } else {
            for i in 0..width {
                out.push(self.get(start + i));
            }
        }
        out
    }

    /// Tulis blok: `val` mengisi `start..start+len` (sebanyak len tersedia).
    pub fn write_block(&mut self, start: usize, val: &mivon_core::LogicVec) {
        for (i, b) in val.bits.iter().enumerate() {
            self.set(start + i, *b);
        }
    }
}

pub struct SimulationState {
    pub signals: Vec<LogicVec>,
    pub next_signals: Vec<LogicVec>,
    pub changed: Vec<bool>,
    /// Sinyal besar (width ≥ `LogicVec::LAZY_ZERO_THRESHOLD`) yang SUDAH
    /// pernah ditulis nyata (bukan default X). Sinyal besar yang belum
    /// ter-tulis di-snapshot LAZY (tanpa materialisasi) selama evaluasi —
    /// array memory flat (cache 65536x32x512 = 1.07e9 entry) dibaca tiap
    /// delta tapi TIDAK perlu di-clone penuh → puncak RSS turun drastis.
    pub large_dirty: Vec<bool>,
    /// Penyimpanan PACKED untuk nilai array memori raksasa yang telah
    /// ditulis: chunk 1024-bit (`[u64;32]`, 2-bit/cell: X=0 Zero=1 One=2 Z=3).
    /// 1e9 bit → ~120MB fisik (vs 1GB `Vec<LogicVal>`) — hanya chunk yang
    /// benar-benar tersentuh oleh tulis. Sinyal ≥ LAZY_ZERO_THRESHOLD memakai
    /// `wide_mem`; `signals`/`next_signals` tetap lazy (X) sbg base.
    pub wide_mem: HashMap<SignalId, WideMem>,
    /// LANG-08: net alias redirect — member SignalId → canonical SignalId.
    /// `alias a = b;` → net_aliases {b: a} (canonical = id terkecil); read/
    /// write member di-direct ke canonical sehingga semua anggota satu
    /// jaringan (short). Identity default (id → id).
    pub alias_redirect: Vec<SignalId>,
    pub time: u64,
    pub objects: Vec<ObjectData>,
    next_obj_id: ObjId,
    /// Fallback LogicVec returned for out-of-bounds read requests.
    /// Prevents panic; instead returns a zero-width X value.
    dummy_signal: LogicVec,
    /// Format untuk `%t` (dari `$timeformat`).
    pub timeformat: crate::simulator::types::TimeFormat,
}

/// Nilai state awal sebuah sinyal dari `init_val` (SignalInfo). Untuk array
/// memory flat raksasa (cache 65536x32x512 = 1.07e9 entry) `init_val.clone()`
/// = materialisasi penuh (1-3 GB/OOM mesin kecil). Default-nya X (memori tak
/// terinisialisasi) → pakai lazy-zero X tanpa copy: driver realistis selalu
/// menulis SEBELUM membaca slot memory; pada akses nyata halaman ter-write
/// kala itu juga. Initializer non-X (jarang utk array besar) tetap di-clone.
fn state_init_signal(init_val: &mivon_core::LogicVec) -> mivon_core::LogicVec {
    if init_val.width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD
        && init_val
            .bits
            .iter()
            .take(8)
            .all(|b| *b == mivon_core::LogicVal::X)
    {
        return mivon_core::LogicVec::new(init_val.width);
    }
    init_val.clone()
}

impl SimulationState {
    pub fn new(design: &IrDesign) -> Self {
        let mut signals = Vec::new();
        let mut next_signals = Vec::new();

        for sig in &design.top.signals {
            signals.push(state_init_signal(&sig.init_val));
            next_signals.push(state_init_signal(&sig.init_val));
        }

        let changed = vec![true; signals.len()];
        let large_dirty = vec![false; signals.len()];
        // LANG-08: net alias redirect — canonical utk tiap member (identity
        // default); member yang di-alias di-direct ke canonical.
        let mut alias_redirect: Vec<SignalId> = (0..signals.len()).collect();
        for (member, canonical) in &design.net_aliases {
            if *member < signals.len() && *canonical < signals.len() {
                alias_redirect[*member] = *canonical;
            }
        }

        // Index 0 is reserved for null handle
        let objects = vec![ObjectData {
            class_name: mivon_core::intern::Symbol::EMPTY,
            fields: HashMap::new(),
        }];

        // Seed basis unit %t dari timescale desain (mis. 1ps → -12).
        // Tanpa ini, %t mengasumsikan basis 1ns untuk desain 1ps/1us.
        let mut timeformat = crate::simulator::types::TimeFormat::default();
        if let Some((ref unit, _)) = design.timescale {
            if let Some(exp) = crate::simulator::types::TimeFormat::unit_exponent(unit) {
                timeformat.base_units = exp;
            }
        }

        SimulationState {
            signals,
            next_signals,
            changed,
            large_dirty,
            wide_mem: HashMap::new(),
            alias_redirect,
            time: 0,
            objects,
            next_obj_id: 1,
            dummy_signal: LogicVec::new(1),
            timeformat,
        }
    }

    pub fn alloc_object(&mut self, class_name: mivon_core::intern::Symbol) -> ObjId {
        let id = self.next_obj_id;
        self.next_obj_id += 1;
        self.objects.push(ObjectData {
            class_name,
            fields: HashMap::new(),
        });
        id
    }

    pub fn reset_objects(&mut self) {
        self.next_obj_id = 1;
        self.objects.clear();
        // Index 0 is reserved for null
        self.objects.push(ObjectData {
            class_name: mivon_core::intern::Symbol::EMPTY,
            fields: HashMap::new(),
        });
    }

    pub fn get_object(&self, id: ObjId) -> Option<&ObjectData> {
        if id > 0 && !self.check_obj_bounds(id) {
            return None;
        }
        self.objects.get(id)
    }

    pub fn get_object_mut(&mut self, id: ObjId) -> Option<&mut ObjectData> {
        if id > 0 && !self.check_obj_bounds(id) {
            return None;
        }
        self.objects.get_mut(id)
    }

    /// Returns false if id is out of bounds, emitting at most one warning per process lifetime.
    #[inline(always)]
    fn check_signal_bounds(&self, id: SignalId) -> bool {
        if id < self.signals.len() {
            return true;
        }
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "[WARN] (internal) SimulationState: signal id {} out of bounds (signals.len={}, next_signals.len={}, changed.len={}) — bukan dari source HDL, tidak ada lokasi source",
                id,
                self.signals.len(),
                self.next_signals.len(),
                self.changed.len()
            );
        }
        false
    }

    /// Returns false if id is out of bounds, emitting at most one warning per process lifetime.
    #[inline(always)]
    fn check_obj_bounds(&self, id: ObjId) -> bool {
        if id < self.objects.len() {
            return true;
        }
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "[WARN] (internal) SimulationState: object id {} out of bounds (objects.len={}) — bukan dari source HDL, tidak ada lokasi source",
                id,
                self.objects.len()
            );
        }
        false
    }

    pub fn read_signal(&self, id: SignalId) -> &LogicVec {
        if !self.check_signal_bounds(id) {
            return &self.dummy_signal;
        }
        // LANG-08: net alias — baca member → baca canonical (nilai sama).
        let id = self.alias_redirect.get(id).copied().unwrap_or(id);
        if self.changed[id] {
            &self.next_signals[id]
        } else {
            &self.signals[id]
        }
    }

    pub fn write_signal(&mut self, id: SignalId, val: LogicVec) {
        if !self.check_signal_bounds(id) {
            return; // silently drop
        }
        // LANG-08: net alias — tulis ke canonical (semua anggota satu jaringan).
        let id = self.alias_redirect.get(id).copied().unwrap_or(id);
        // Sinyal besar yang menerima tulis nyata → tandai dirty (snapshot
        // berikutnya harus clone kenyataan, bukan lazy X).
        if self.large_dirty.len() > id
            && self.signals[id].width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD
        {
            self.large_dirty[id] = true;
        }
        // Compare against pending (next_signals) if already changed this delta,
        // otherwise compare against committed (signals)
        if self.changed[id] {
            if self.next_signals[id] != val {
                self.next_signals[id] = val;
            }
        } else if self.signals[id] != val {
            self.next_signals[id] = val;
            self.changed[id] = true;
        }
    }

    /// Snapshot nilai sinyal utk evaluasi/preponed/history. Sinyal ultra-lebar
    /// (array memori flat) SELALU di-snapshot LAZY (tanpa materialisasi):
    /// - par-eval utk array raksasa SUDAH di-disable (`has_wide_signals` →
    ///   sequential) sehingga snapshot tidak pernah dipakai utk nilai evaluasi;
    /// - evaluasi sequential membaca state langsung;
    /// - preponed/signal_history hanya butuh skalar/edge (array tak relevan).
    ///
    /// Klon penuh 1e9 bit per delta = OOM mesin kecil.
    #[inline]
    pub fn snapshot_signal(&self, id: SignalId) -> LogicVec {
        let lv = if self.changed[id] {
            &self.next_signals[id]
        } else {
            &self.signals[id]
        };
        if lv.width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD {
            return mivon_core::LogicVec::new(lv.width);
        }
        lv.clone()
    }

    /// Tulis segmen sinyal langsung ke pending (`next`) tanpa clone penuh —
    /// utk array memori flat ultra-lebar (RMW penuh = clone 1e9 bit/OOM).
    /// `start..end` (end exclusive) diisi dari `val.bits` (sebanyak len);
    /// out-of-bounds mengikuti LRM (tulis dibatasi panjang, OOB diabaikan).
    /// Sinyal ultra-lebar dialihkan ke penyimpanan PACKED (`wide_mem`) —
    /// `next_signals` tetap lazy (X) sbg base.
    #[inline]
    pub fn write_signal_slice(&mut self, id: SignalId, start: usize, end: usize, val: &LogicVec) {
        let id = self.alias_redirect.get(id).copied().unwrap_or(id);
        if id >= self.next_signals.len() {
            return;
        }
        if self.next_signals[id].width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD {
            // Array raksasa: tulis packed — base lazy tak diubah.
            let wm = self.wide_mem.entry(id).or_default();
            wm.write_block(start, val);
            self.large_dirty[id] = true;
            self.changed[id] = true;
            return;
        }
        if self.large_dirty.len() > id
            && self.next_signals[id].width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD
        {
            self.large_dirty[id] = true;
        }
        let n = val.bits.len();
        let mut end = end.min(start.saturating_add(n));
        if end < start {
            return;
        }
        let target = &mut self.next_signals[id];
        if end > target.bits.len() {
            end = target.bits.len();
        }
        if start < end {
            target.bits[start..end].copy_from_slice(&val.bits[..end - start]);
        }
        self.changed[id] = true;
    }

    /// Baca satu bit sinyal — utk sinyal ultra-lebar dibaca dari penyimpanan
    /// packed (`wide_mem`) bila telah ditulis; else X default (lazy base).
    #[inline]
    pub fn read_signal_bit(&self, id: SignalId, bit: usize) -> mivon_core::LogicVal {
        let id = self.alias_redirect.get(id).copied().unwrap_or(id);
        if let Some(wm) = self.wide_mem.get(&id) {
            return wm.get(bit);
        }
        let lv = if self.changed[id] {
            &self.next_signals[id]
        } else {
            &self.signals[id]
        };
        lv.bits.get(bit).copied().unwrap_or(mivon_core::LogicVal::X)
    }

    /// Baca blok sinyal tanpа materialisasi penuh — utk sinyal ultra-lebar
    /// dibaca слиce dari `wide_mem`; sinyal reguler dibaca via borrow.
    #[inline]
    pub fn read_signal_slice(&self, id: SignalId, start: usize, width: usize) -> LogicVec {
        let id = self.alias_redirect.get(id).copied().unwrap_or(id);
        if let Some(wm) = self.wide_mem.get(&id) {
            // Array raksasa: baca blok (per-chunk — jauh lebih cepat utk
            // slot 512-bit seperti cache data line).
            let bits = wm.read_block(start, width);
            return LogicVec { width, bits };
        }
        let lv = if self.changed[id] {
            &self.next_signals[id]
        } else {
            &self.signals[id]
        };
        let mut bits = Vec::with_capacity(width);
        for i in 0..width {
            bits.push(
                lv.bits
                    .get(start + i)
                    .copied()
                    .unwrap_or(mivon_core::LogicVal::X),
            );
        }
        LogicVec { width, bits }
    }

    pub fn commit_changes(&mut self) -> Vec<(SignalId, LogicVec, LogicVec)> {
        let mut changed = Vec::new();
        for i in 0..self.signals.len() {
            if self.changed[i] {
                if self.signals[i].width >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD {
                    // Array memory flat besar: pindahkan OWNERSHIP (swap, O(1))
                    // tanpa clone 1e9 bit (spike 3 GB/OOM saat miss tulis).
                    // next_signals di-reset lazy-X untuk delta berikutnya.
                    let w = self.next_signals[i].width;
                    let old = mivon_core::LogicVec::new(w);
                    std::mem::swap(&mut self.signals[i], &mut self.next_signals[i]);
                    self.next_signals[i] = mivon_core::LogicVec::new(w);
                    self.changed[i] = false;
                    changed.push((i, old, mivon_core::LogicVec::new(w)));
                    self.large_dirty[i] = true;
                    continue;
                }
                let old = self.signals[i].clone();
                let new = self.next_signals[i].clone();
                self.signals[i] = new.clone();
                self.next_signals[i] = new.clone();
                self.changed[i] = false;
                if self.signals[i] != old {
                    changed.push((i, old, self.signals[i].clone()));
                }
            }
        }
        changed
    }

    pub fn advance_time(&mut self) {
        self.time += 1;
    }

    pub fn signal_name(&self, id: SignalId, module: &IrModule) -> String {
        module
            .signals
            .get(id)
            .map(|s| s.name.to_string())
            .unwrap_or_else(|| format!("sig_{}", id))
    }

    pub fn dump_all_signals(&self, module: &IrModule) {
        println!("--- Time {} ---", self.time);
        for sig in &module.signals {
            let val = self.read_signal(self.find_signal_id(sig.name.as_str(), module).unwrap_or(0));
            println!("  {} = {} ({}b)", sig.name, val, sig.width);
        }
    }

    fn find_signal_id(&self, name: &str, module: &IrModule) -> Option<SignalId> {
        module.signals.iter().position(|s| s.name.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wide_mem_roundtrip() {
        let mut wm = WideMem::default();
        assert_eq!(wm.get(0), mivon_core::LogicVal::X); // belum ditulis = X
        wm.set(0, mivon_core::LogicVal::One);
        wm.set(31, mivon_core::LogicVal::One);
        wm.set(32, mivon_core::LogicVal::Zero);
        wm.set(63, mivon_core::LogicVal::Z);
        assert_eq!(wm.get(0), mivon_core::LogicVal::One);
        assert_eq!(wm.get(30), mivon_core::LogicVal::X); // tetangga tetap X
        assert_eq!(wm.get(31), mivon_core::LogicVal::One);
        assert_eq!(wm.get(32), mivon_core::LogicVal::Zero);
        assert_eq!(wm.get(33), mivon_core::LogicVal::X);
        assert_eq!(wm.get(63), mivon_core::LogicVal::Z);
    }

    #[test]
    fn test_wide_mem_chunk_boundary() {
        // Boundary chunk 2048 — bit di ujung & awal chunk berikutnya.
        let mut wm = WideMem::default();
        wm.set(WIDE_CHUNK_BITS - 1, mivon_core::LogicVal::One);
        wm.set(WIDE_CHUNK_BITS, mivon_core::LogicVal::One);
        wm.set(WIDE_CHUNK_BITS + 2047, mivon_core::LogicVal::Zero);
        assert_eq!(wm.get(WIDE_CHUNK_BITS - 1), mivon_core::LogicVal::One);
        assert_eq!(wm.get(WIDE_CHUNK_BITS), mivon_core::LogicVal::One);
        assert_eq!(wm.get(WIDE_CHUNK_BITS + 1), mivon_core::LogicVal::X);
        assert_eq!(wm.get(WIDE_CHUNK_BITS + 2047), mivon_core::LogicVal::Zero);
        assert_eq!(wm.get(WIDE_CHUNK_BITS + 2048), mivon_core::LogicVal::X);
    }

    #[test]
    fn test_wide_mem_write_block_and_read_signal_slice() {
        let w = WIDE_CHUNK_BITS * 2 + 1024;
        let mut st = SimulationState {
            signals: vec![LogicVec::new(w)],
            next_signals: vec![LogicVec::new(w)],
            changed: vec![false],
            large_dirty: vec![false],
            wide_mem: HashMap::new(),
            alias_redirect: vec![0],
            time: 0,
            objects: vec![],
            next_obj_id: 1,
            dummy_signal: LogicVec::new(1),
            timeformat: crate::simulator::types::TimeFormat::default(),
        };
        // Tulis blok 64 bit di offset 100.
        let mut v = LogicVec::new(64);
        for (i, b) in v.bits.iter_mut().enumerate() {
            *b = if i % 2 == 0 {
                mivon_core::LogicVal::One
            } else {
                mivon_core::LogicVal::Zero
            };
        }
        st.write_signal_slice(0, 100, 100 + 64, &v);
        // Baca slice dan bit.
        let got = st.read_signal_slice(0, 100, 64);
        assert_eq!(got, v);
        assert_eq!(st.read_signal_bit(0, 100), mivon_core::LogicVal::One);
        assert_eq!(st.read_signal_bit(0, 101), mivon_core::LogicVal::Zero);
        assert_eq!(st.read_signal_bit(0, 99), mivon_core::LogicVal::X); // di luar blok
                                                                        // Chunk yg belum ditulis = X.
        assert_eq!(
            st.read_signal_bit(0, WIDE_CHUNK_BITS),
            mivon_core::LogicVal::X
        );
    }
}
