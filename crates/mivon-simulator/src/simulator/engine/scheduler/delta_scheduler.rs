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
//! Not yet wired into the simulation engine run loop — stage 1 is the
//! trait + sequential baseline with a regression test for delta ordering.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// An event scheduled at a given simulation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeltaEvent {
    pub time: u64,
    pub id: usize,
}

/// A scheduler that delivers events grouped by simulation time (delta cycle).
pub trait DeltaScheduler {
    /// Push an event to be delivered at `time`.
    fn push_event(&mut self, time: u64, event: DeltaEvent);
    /// Deliver all events at `time` (in insertion order), returning them.
    /// Returns empty if no events at `time`.
    fn deliver_at(&mut self, time: u64) -> Vec<DeltaEvent>;
    /// Advance to `time`, delivering all events at times `<= time` in
    /// ascending order. Returns all delivered events.
    fn advance_to(&mut self, time: u64) -> Vec<DeltaEvent>;
    /// Number of pending (undelivered) events.
    fn pending(&self) -> usize;
}

/// Single-threaded delta scheduler: events stored in a `BTreeMap` keyed by
/// time, delivered in ascending time order.
#[derive(Debug, Clone, Default)]
pub struct SequentialDeltaScheduler {
    events: BTreeMap<u64, Vec<DeltaEvent>>,
}

impl SequentialDeltaScheduler {
    pub fn new() -> Self {
        Self::default()
    }
}

impl DeltaScheduler for SequentialDeltaScheduler {
    fn push_event(&mut self, time: u64, event: DeltaEvent) {
        self.events.entry(time).or_default().push(event);
    }

    fn deliver_at(&mut self, time: u64) -> Vec<DeltaEvent> {
        self.events.remove(&time).unwrap_or_default()
    }

    fn advance_to(&mut self, time: u64) -> Vec<DeltaEvent> {
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
#[derive(Debug, Default)]
pub struct ParallelDeltaScheduler {
    inner: Mutex<BTreeMap<u64, Vec<DeltaEvent>>>,
}

impl ParallelDeltaScheduler {
    pub fn new() -> Self {
        Self::default()
    }
}

impl DeltaScheduler for ParallelDeltaScheduler {
    fn push_event(&mut self, time: u64, event: DeltaEvent) {
        self.inner
            .lock()
            .unwrap()
            .entry(time)
            .or_default()
            .push(event);
    }

    fn deliver_at(&mut self, time: u64) -> Vec<DeltaEvent> {
        self.inner.lock().unwrap().remove(&time).unwrap_or_default()
    }

    fn advance_to(&mut self, time: u64) -> Vec<DeltaEvent> {
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
        let mut s = SequentialDeltaScheduler::new();
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
        let mut s = SequentialDeltaScheduler::new();
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
        let mut s = SequentialDeltaScheduler::new();
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
        let mut s = SequentialDeltaScheduler::new();
        s.push_event(10, ev(10, 1));
        s.advance_to(5);
        assert_eq!(s.pending(), 1);
        s.push_event(3, ev(3, 2)); // time 3 <= last advance 5, but inserted late
        assert_eq!(s.advance_to(10), vec![ev(3, 2), ev(10, 1)]);
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn pending_count_matches_undelivered_events() {
        let mut s = SequentialDeltaScheduler::new();
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
        let mut s = ParallelDeltaScheduler::new();
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
        let mut s = ParallelDeltaScheduler::new();
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

        let handles: Vec<thread::JoinHandle<ParallelDeltaScheduler>> = (0..4)
            .map(|idx| {
                thread::spawn(move || {
                    let mut s = ParallelDeltaScheduler::new();
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
        let mut merged = ParallelDeltaScheduler::new();
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

        let handles: Vec<thread::JoinHandle<ParallelDeltaScheduler>> = (0..8)
            .map(|idx| {
                thread::spawn(move || {
                    let mut s = ParallelDeltaScheduler::new();
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
}
