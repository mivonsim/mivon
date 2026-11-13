//! Delta-cycle scheduler foundation — stage 1 of SIM-02
//! (multi-threaded delta-cycle scheduling, Critical, open).
//!
//! Defines the `DeltaScheduler` trait and a single-threaded
//! `SequentialDeltaScheduler` implementation. Events are grouped by
//! simulation time (delta cycle) and delivered in ascending time order.
//! Later stages will add a lock-free / region-parallel multi-threaded
//! scheduler behind this trait.
//!
//! Not yet wired into the simulation engine run loop — stage 1 is the
//! trait + sequential baseline with a regression test for delta ordering.

use std::collections::BTreeMap;

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
}
