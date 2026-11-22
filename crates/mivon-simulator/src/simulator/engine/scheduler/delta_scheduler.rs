//! Delta-cycle scheduler foundation — stage 1 of SIM-02
//! (multi-threaded delta-cycle scheduling, Critical, open).
//!
//! Defines the `DeltaScheduler` trait and a single-threaded
//! `SequentialDeltaScheduler` implementation plus a thread-safe
//! `ParallelDeltaScheduler` for concurrent push/deliver across threads.
//! Events are grouped by simulation time (delta cycle) and delivered in
//! ascending time order. Later stages will wire a lock-free / region-parallel
//! multi-threaded scheduler into the engine run loop.
//!
//! Stage 3: Generic over event type to support RegionEvent wiring.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::simulator::types::{EventKind, EventRegion, RegionEvent};

/// An event scheduled at a given simulation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeltaEvent {
    pub time: u64,
    pub id: usize,
}

/// A scheduler that delivers events grouped by simulation time (delta cycle).
///
/// Generic over the event type `E` to support different event representations
/// (e.g., `DeltaEvent` for minimal scheduling, `RegionEvent` for full engine integration).
pub trait DeltaScheduler<E> {
    /// Push an event to be delivered at `time`.
    fn push_event(&mut self, time: u64, event: E);
    /// Deliver all events at `time` (in insertion order), returning them.
    /// Returns empty if no events at `time`.
    fn deliver_at(&mut self, time: u64) -> Vec<E>;
    /// Advance to `time`, delivering all events at times `<= time` in
    /// ascending order. Returns all delivered events.
    fn advance_to(&mut self, time: u64) -> Vec<E>;
    /// Number of pending (undelivered) events.
    fn pending(&self) -> usize;
}

/// Single-threaded delta scheduler: events stored in a `BTreeMap` keyed by
/// time, delivered in ascending time order.
#[derive(Debug, Clone)]
pub struct SequentialDeltaScheduler<E> {
    events: BTreeMap<u64, Vec<E>>,
}

impl<E> Default for SequentialDeltaScheduler<E> {
    fn default() -> Self {
        Self {
            events: BTreeMap::new(),
        }
    }
}

impl<E> SequentialDeltaScheduler<E> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<E> DeltaScheduler<E> for SequentialDeltaScheduler<E> {
    fn push_event(&mut self, time: u64, event: E) {
        self.events.entry(time).or_default().push(event);
    }

    fn deliver_at(&mut self, time: u64) -> Vec<E> {
        self.events.remove(&time).unwrap_or_default()
    }

    fn advance_to(&mut self, time: u64) -> Vec<E> {
        let mut out = Vec::new();
        let keys: Vec<u64> = self
            .events
            .keys()
            .copied()
            .filter(|&t| t <= time)
            .collect();
        for t in keys {
            if let Some(mut evs) = self.events.remove(&t) {
                out.append(&mut evs);
            }
        }
        out
    }

    fn pending(&self) -> usize {
        self.events.values().map(Vec::len).sum()
    }
}

/// Thread-safe multi-threaded delta-cycle scheduler.
///
/// Wraps the same `BTreeMap`-by-time structure as
/// `SequentialDeltaScheduler` behind a `Mutex` so multiple threads can
/// `push_event` and `deliver_at`/`advance_to` concurrently without losing
/// events or corrupting the map. Delivery order within a single time bucket
/// is FIFO (insertion order); across time buckets it is ascending by time.
///
/// This is additive and NOT wired into the simulation engine run loop yet —
/// it exists as a correct, tested multi-threaded delta scheduler behind the
/// `DeltaScheduler` trait for later stages to adopt.
#[derive(Debug)]
pub struct ParallelDeltaScheduler<E> {
    inner: Mutex<BTreeMap<u64, Vec<E>>>,
}

impl<E> Default for ParallelDeltaScheduler<E> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }
}

impl<E> ParallelDeltaScheduler<E> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<E> DeltaScheduler<E> for ParallelDeltaScheduler<E> {
    fn push_event(&mut self, time: u64, event: E) {
        self.inner
            .lock()
            .unwrap()
            .entry(time)
            .or_default()
            .push(event);
    }

    fn deliver_at(&mut self, time: u64) -> Vec<E> {
        self.inner.lock().unwrap().remove(&time).unwrap_or_default()
    }

    fn advance_to(&mut self, time: u64) -> Vec<E> {
        let mut out = Vec::new();
        let mut guard = self.inner.lock().unwrap();
        let keys: Vec<u64> = guard
            .keys()
            .copied()
            .filter(|&t| t <= time)
            .collect();
        for t in keys {
            if let Some(mut evs) = guard.remove(&t) {
                out.append(&mut evs);
            }
        }
        out
    }

    fn pending(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .values()
            .map(Vec::len)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(time: u64, id: usize) -> DeltaEvent {
        DeltaEvent { time, id }
    }

    #[test]
    fn delivers_in_ascending_time_order_regardless_of_insertion_order() {
        let mut s = SequentialDeltaScheduler::<DeltaEvent>::new();
        s.push_event(50, ev(50, 3));
        s.push_event(10, ev(10, 1));
        s.push_event(30, ev(30, 2));
        let delivered = s.advance_to(100);
        assert_eq!(
            delivered,
            vec![ev(10, 1), ev(30, 2), ev(50, 3)],
            "events must be ordered by time, not insertion order"
        );
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn deliver_at_returns_only_that_time() {
        let mut s = SequentialDeltaScheduler::<DeltaEvent>::new();
        s.push_event(5, ev(5, 1));
        s.push_event(5, ev(5, 2));
        s.push_event(7, ev(7, 3));
        assert_eq!(s.deliver_at(5), vec![ev(5, 1), ev(5, 2)]);
        assert_eq!(s.deliver_at(7), vec![ev(7, 3)]);
        assert_eq!(s.deliver_at(5), Vec::<DeltaEvent>::new()); // already drained
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn advance_to_removes_delivered_events() {
        let mut s = SequentialDeltaScheduler::<DeltaEvent>::new();
        s.push_event(1, ev(1, 1));
        s.push_event(2, ev(2, 2));
        s.push_event(3, ev(3, 3));
        let delivered = s.advance_to(2);
        assert_eq!(delivered, vec![ev(1, 1), ev(2, 2)]);
        assert_eq!(s.pending(), 1);
        // event at time 3 still pending; advancing further delivers it.
        assert_eq!(s.advance_to(10), vec![ev(3, 3)]);
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn events_inserted_after_advance_are_not_retroactively_delivered() {
        let mut s = SequentialDeltaScheduler::<DeltaEvent>::new();
        s.push_event(10, ev(10, 1));
        s.advance_to(5);
        assert_eq!(s.pending(), 1);
        s.push_event(3, ev(3, 2)); // time 3 <= last advance 5, but inserted late
        assert_eq!(s.advance_to(10), vec![ev(3, 2), ev(10, 1)]);
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn pending_count_matches_undelivered_events() {
        let mut s = SequentialDeltaScheduler::<DeltaEvent>::new();
        assert_eq!(s.pending(), 0);
        s.push_event(1, ev(1, 1));
        s.push_event(2, ev(2, 2));
        assert_eq!(s.pending(), 2);
        s.deliver_at(1);
        assert_eq!(s.pending(), 1);
    }

    // ─── ParallelDeltaScheduler tests ───

    #[test]
    fn parallel_delivers_in_ascending_time_order() {
        let mut s = ParallelDeltaScheduler::<DeltaEvent>::new();
        s.push_event(50, ev(50, 3));
        s.push_event(10, ev(10, 1));
        s.push_event(30, ev(30, 2));
        let delivered = s.advance_to(100);
        assert_eq!(
            delivered,
            vec![ev(10, 1), ev(30, 2), ev(50, 3)],
            "parallel events must be ordered by time, not insertion order"
        );
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn parallel_deliver_at_returns_only_that_time() {
        let mut s = ParallelDeltaScheduler::<DeltaEvent>::new();
        s.push_event(5, ev(5, 1));
        s.push_event(5, ev(5, 2));
        s.push_event(7, ev(7, 3));
        assert_eq!(s.deliver_at(5), vec![ev(5, 1), ev(5, 2)]);
        assert_eq!(s.deliver_at(7), vec![ev(7, 3)]);
        assert_eq!(s.deliver_at(5), Vec::<DeltaEvent>::new());
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn parallel_concurrent_push_and_deliver_preserves_all_events() {
        use std::thread;

        let handles: Vec<thread::JoinHandle<ParallelDeltaScheduler<DeltaEvent>>> = (0..4)
            .map(|idx| {
                thread::spawn(move || {
                    let mut s = ParallelDeltaScheduler::<DeltaEvent>::new();
                    // Each thread pushes 200 events across distinct times.
                    for k in 0..200 {
                        let time: u64 = (idx as u64 * 1000) + (k as u64 % 50); // bucket: 0..49 per thread
                        s.push_event(time, ev(time, idx * 1000 + k));
                    }
                    s
                })
            })
            .collect();

        // Merge all per-thread schedulers into one by draining each.
        let mut merged = ParallelDeltaScheduler::<DeltaEvent>::new();
        for h in handles {
            let mut s = h.join().unwrap();
            let evs = s.advance_to(100_000);
            for e in evs {
                merged.push_event(e.time, e);
            }
        }

        let all = merged.advance_to(100_000);
        // 4 threads × 200 events = 800 events, none lost.
        assert_eq!(all.len(), 800, "no events may be lost under concurrency");
        // Times must be non-decreasing.
        for w in all.windows(2) {
            assert!(
                w[0].time <= w[1].time,
                "merged events must be time-ordered"
            );
        }
        assert_eq!(merged.pending(), 0);
    }

    #[test]
    fn parallel_concurrent_multi_thread_drain_is_consistent() {
        use std::thread;

        let handles: Vec<thread::JoinHandle<ParallelDeltaScheduler<DeltaEvent>>> = (0..8)
            .map(|idx| {
                thread::spawn(move || {
                    let mut s = ParallelDeltaScheduler::<DeltaEvent>::new();
                    for k in 0..100 {
                        s.push_event((k % 10) as u64, ev((k % 10) as u64, idx * 100 + k));
                    }
                    s
                })
            })
            .collect();

        // Each scheduler drains to exactly 100 events at times 0..9 (10 each).
        for h in handles {
            let mut s = h.join().unwrap();
            let total: usize = (0..10).map(|t| s.deliver_at(t).len()).sum();
            assert_eq!(total, 100, "each thread drained 100 events");
            assert_eq!(s.pending(), 0);
        }
    }

    // ─── ARSITEKTUR-03 stage 2: cross-scheduler equivalence ───
    // ParallelDeltaScheduler harus menghasilkan hasil identik dengan
    // SequentialDeltaScheduler untuk same sequence push/deliver.

    #[test]
    fn parallel_delta_matches_sequential_equivalence() {
        let times = [0, 5, 3, 10, 7, 1, 5, 100, 42];
        let mut seq = SequentialDeltaScheduler::<DeltaEvent>::new();
        let mut par = ParallelDeltaScheduler::<DeltaEvent>::new();
        for (i, &t) in times.iter().enumerate() {
            seq.push_event(t, ev(t, i));
            par.push_event(t, ev(t, i));
        }
        // Deliver all from both
        let max_t = times.iter().max().unwrap();
        let seq_all = seq.advance_to(*max_t + 10);
        let par_all = par.advance_to(*max_t + 10);
        assert_eq!(
            seq_all, par_all,
            "parallel and sequential delta schedulers must deliver identical event sequences"
        );
        assert_eq!(seq.pending(), 0);
        assert_eq!(par.pending(), 0);
    }

    #[test]
    fn parallel_delta_deliver_at_matches_sequential() {
        let mut seq = SequentialDeltaScheduler::<DeltaEvent>::new();
        let mut par = ParallelDeltaScheduler::<DeltaEvent>::new();
        seq.push_event(10, ev(10, 1));
        seq.push_event(10, ev(10, 2));
        seq.push_event(20, ev(20, 3));
        par.push_event(10, ev(10, 1));
        par.push_event(10, ev(10, 2));
        par.push_event(20, ev(20, 3));

        assert_eq!(seq.deliver_at(10), par.deliver_at(10));
        assert_eq!(seq.deliver_at(20), par.deliver_at(20));
        assert_eq!(seq.pending(), 0);
        assert_eq!(par.pending(), 0);
    }
}

// ─── RegionEvent-based scheduler type aliases (Stage 3 wiring) ───

/// Sequential delta scheduler for RegionEvent — drop-in replacement for
/// `Vec<Vec<RegionEvent>>` event queue in the engine.
pub type SequentialRegionScheduler = SequentialDeltaScheduler<RegionEvent>;

/// Parallel delta scheduler for RegionEvent — multi-threaded event queue
/// for the engine run loop (stage 3 of ARSITEKTUR-03).
pub type ParallelRegionScheduler = ParallelDeltaScheduler<RegionEvent>;

#[cfg(test)]
mod region_scheduler_tests {
    use super::*;
    use crate::simulator::types::{EventKind, EventRegion, RegionEvent};

    fn re(region: EventRegion, kind: EventKind) -> RegionEvent {
        RegionEvent { region, event: kind }
    }

    #[test]
    fn sequential_region_scheduler_basic() {
        let mut s = SequentialRegionScheduler::new();
        s.push_event(10, re(EventRegion::Active, EventKind::EvalProcess(1)));
        s.push_event(10, re(EventRegion::Active, EventKind::EvalProcess(2)));
        s.push_event(20, re(EventRegion::Nba, EventKind::EvalProcess(3)));

        let delivered = s.deliver_at(10);
        assert_eq!(delivered.len(), 2);
        assert_eq!(delivered[0].region, EventRegion::Active);
        assert_eq!(delivered[1].region, EventRegion::Active);

        let delivered2 = s.deliver_at(20);
        assert_eq!(delivered2.len(), 1);
        assert_eq!(delivered2[0].region, EventRegion::Nba);

        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn parallel_region_scheduler_basic() {
        let mut s = ParallelRegionScheduler::new();
        s.push_event(10, re(EventRegion::Active, EventKind::EvalProcess(1)));
        s.push_event(10, re(EventRegion::Active, EventKind::EvalProcess(2)));
        s.push_event(20, re(EventRegion::Nba, EventKind::EvalProcess(3)));

        let delivered = s.deliver_at(10);
        assert_eq!(delivered.len(), 2);

        let delivered2 = s.deliver_at(20);
        assert_eq!(delivered2.len(), 1);

        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn parallel_region_scheduler_concurrent() {
        use std::sync::Arc;
        use std::thread;

        // Test concurrent pushes to a SHARED scheduler (like real usage)
        let shared = Arc::new(std::sync::Mutex::new(ParallelRegionScheduler::new()));
        let handles: Vec<thread::JoinHandle<()>> = (0..4)
            .map(|idx| {
                let s = Arc::clone(&shared);
                thread::spawn(move || {
                    let mut s = s.lock().unwrap();
                    for k in 0..50 {
                        let time = (idx * 100 + k) as u64;
                        s.push_event(time, re(EventRegion::Active, EventKind::EvalProcess(k)));
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Drain all events
        let mut s = Arc::try_unwrap(shared).unwrap().into_inner().unwrap();
        let all = s.advance_to(100_000);
        assert_eq!(all.len(), 200); // 4 threads × 50 events
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn region_scheduler_equivalence() {
        let mut seq = SequentialRegionScheduler::new();
        let mut par = ParallelRegionScheduler::new();

        let events = [
            (10, EventRegion::Active, EventKind::EvalProcess(1)),
            (10, EventRegion::Nba, EventKind::EvalProcess(2)),
            (20, EventRegion::Active, EventKind::EvalProcess(3)),
            (20, EventRegion::Reactive, EventKind::EvalProcess(4)),
        ];

        for (t, r, k) in events {
            seq.push_event(t, re(r, k.clone()));
            par.push_event(t, re(r, k));
        }

        let seq_all = seq.advance_to(100);
        let par_all = par.advance_to(100);

        assert_eq!(seq_all.len(), par_all.len());
        for (s, p) in seq_all.iter().zip(par_all.iter()) {
            assert_eq!(s.region, p.region);
            // EventKind doesn't implement PartialEq, compare via Debug
            assert_eq!(format!("{:?}", s.event), format!("{:?}", p.event));
        }
    }
}
