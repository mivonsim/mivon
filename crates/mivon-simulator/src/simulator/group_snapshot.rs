//! PERF-06: cache snapshot sinyal antar-kelompok evaluasi paralel.
//!
//! Evaluasi comb/sequential paralel dibagi menjadi kelompok (layering
//! RAW/WAR/WAW). Tiap kelompok butuh snapshot state terbaru, tetapi
//! membangun ulang SELURUH snapshot per kelompok berarti O(sinyal x
//! kelompok) alokasi `Arc` per delta — bagian yang tetap serial dan
//! mendominasi biaya saat jumlah sinyal besar.
//!
//! Kelompok hanya mengubah sinyal yang ditulisnya, jadi snapshot cukup
//! dibangun sekali per pemanggilan lalu di-refresh inkremental untuk
//! sinyal tersebut. Snapshot tetap immutable (`Arc<LogicVec>`) sehingga
//! dibaca paralel tanpa `&mut`.

use std::sync::Arc;

use mivon_core::LogicVec;

/// Snapshot immutable sinyal yang dipakai bersama antar kelompok.
pub(crate) struct GroupSnapshot {
    entries: Vec<Arc<LogicVec>>,
}

impl GroupSnapshot {
    /// Bangun snapshot awal; `value` dipanggil sekali per sinyal.
    pub(crate) fn build(count: usize, mut value: impl FnMut(usize) -> LogicVec) -> Self {
        let entries = (0..count).map(|i| Arc::new(value(i))).collect();
        Self { entries }
    }

    /// Tampilan baca-saja untuk `SignalView` (dibaca paralel).
    pub(crate) fn as_slice(&self) -> &[Arc<LogicVec>] {
        &self.entries
    }

    /// Refresh entri untuk sinyal yang ditulis kelompok sebelumnya.
    /// Id duplikat di-dedup; id di luar jangkauan diabaikan.
    pub(crate) fn refresh(&mut self, ids: &[usize], mut value: impl FnMut(usize) -> LogicVec) {
        let mut dirty: Vec<usize> = ids
            .iter()
            .copied()
            .filter(|id| *id < self.entries.len())
            .collect();
        dirty.sort_unstable();
        dirty.dedup();
        for id in dirty {
            self.entries[id] = Arc::new(value(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(v: u64) -> LogicVec {
        LogicVec::from_u64(v, 8)
    }

    #[test]
    fn build_calls_value_once_per_signal() {
        let mut calls = 0usize;
        let snap = GroupSnapshot::build(3, |i| {
            calls += 1;
            lv(i as u64)
        });
        assert_eq!(calls, 3);
        assert_eq!(snap.as_slice().len(), 3);
        assert_eq!(snap.as_slice()[2].to_u64(), 2);
    }

    #[test]
    fn refresh_only_rebuilds_dirty_entries() {
        let mut calls = 0usize;
        let mut snap = GroupSnapshot::build(3, |_| {
            calls += 1;
            lv(0)
        });
        let untouched = Arc::clone(&snap.as_slice()[0]);
        let replaced = Arc::clone(&snap.as_slice()[1]);
        // Duplikat + id di luar jangkauan: hanya id 1 yang dibangun ulang.
        snap.refresh(&[1, 1, 9], |i| {
            calls += 1;
            lv(i as u64)
        });
        assert_eq!(calls, 4, "build 3 + satu refresh (dedup, 9 diabaikan)");
        assert!(
            Arc::ptr_eq(&untouched, &snap.as_slice()[0]),
            "id 0 tidak boleh dibangun ulang"
        );
        assert!(
            !Arc::ptr_eq(&replaced, &snap.as_slice()[1]),
            "id 1 wajib diganti nilai baru"
        );
        assert_eq!(snap.as_slice()[1].to_u64(), 1);
    }

    #[test]
    fn refresh_with_empty_ids_keeps_all_entries() {
        let mut snap = GroupSnapshot::build(2, |_| lv(0));
        let before: Vec<Arc<LogicVec>> = snap.as_slice().iter().map(Arc::clone).collect();
        let mut calls = 0usize;
        snap.refresh(&[], |_| {
            calls += 1;
            lv(1)
        });
        assert_eq!(calls, 0);
        for (old, new) in before.iter().zip(snap.as_slice()) {
            assert!(Arc::ptr_eq(old, new));
        }
    }
}
