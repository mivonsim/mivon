//! BDD (Binary Decision Diagram) engine — stage 1 of FORMAL-07.
//!
//! Foundation only: a hash-consed unique table producing canonical, reduced
//! BDD nodes (the `low == high` reduction rule is applied). Not yet wired
//! into the Z3 BMC / induction pipeline — later stages will implement apply
//! (AND/OR/NOT/ITE), variable reordering, and counterexample extraction.

use std::collections::HashMap;

/// A BDD variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BddVar(pub usize);

/// A reference to a BDD node. `BddRef(0)` = false terminal, `BddRef(1)` =
/// true terminal; `index >= 2` are internal nodes in the manager's arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BddRef(pub usize);

/// An internal BDD decision node: branch on `var`, follow `low` when var=0
/// and `high` when var=1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BddNode {
    pub var: BddVar,
    pub low: BddRef,
    pub high: BddRef,
}

/// BDD manager with a unique table (hash consing).
///
/// Every `(var, low, high)` triple is created at most once, so equivalent
/// subgraphs share nodes (canonical representation). The reduction rule
/// `mk(var, x, x) == x` eliminates redundant decision nodes.
pub struct BddManager {
    /// Arena: index 0 = false terminal, index 1 = true terminal, index >= 2
    /// = internal nodes.
    arena: Vec<BddNode>,
    /// `(var, low, high) -> node ref` unique table.
    unique: HashMap<(BddVar, BddRef, BddRef), BddRef>,
}

impl Default for BddManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BddManager {
    /// Create a new manager with the two constant terminals.
    pub fn new() -> Self {
        let mut arena = Vec::with_capacity(16);
        arena.push(BddNode {
            var: BddVar(usize::MAX),
            low: BddRef(0),
            high: BddRef(0),
        }); // false terminal
        arena.push(BddNode {
            var: BddVar(usize::MAX),
            low: BddRef(1),
            high: BddRef(1),
        }); // true terminal
        Self {
            arena,
            unique: HashMap::new(),
        }
    }

    /// Reference to the false terminal.
    pub fn false_ref(&self) -> BddRef {
        BddRef(0)
    }

    /// Reference to the true terminal.
    pub fn true_ref(&self) -> BddRef {
        BddRef(1)
    }

    /// Create (or reuse) the node `(var, low, high)`.
    ///
    /// Reduction: if `low == high`, the decision on `var` is redundant and
    /// `low` is returned instead. If an identical triple already exists, its
    /// reference is returned (hash consing). Otherwise a fresh node is
    /// allocated and registered in the unique table.
    pub fn mk(&mut self, var: BddVar, low: BddRef, high: BddRef) -> BddRef {
        if low == high {
            return low;
        }
        let key = (var, low, high);
        if let Some(&r) = self.unique.get(&key) {
            return r;
        }
        let r = BddRef(self.arena.len());
        self.arena.push(BddNode { var, low, high });
        self.unique.insert(key, r);
        r
    }

    /// Total number of nodes in the arena (terminals + internal nodes).
    pub fn node_count(&self) -> usize {
        self.arena.len()
    }

    /// Whether `r` is one of the two constant terminals.
    pub fn is_terminal(&self, r: BddRef) -> bool {
        r.0 < 2
    }

    /// Read the node at `r` (panics only if `r` is out of arena bounds;
    /// terminals 0/1 are valid arena entries).
    pub fn node(&self, r: BddRef) -> &BddNode {
        &self.arena[r.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_are_distinct_and_terminal() {
        let m = BddManager::new();
        let f = m.false_ref();
        let t = m.true_ref();
        assert_ne!(f, t);
        assert!(m.is_terminal(f));
        assert!(m.is_terminal(t));
        assert_eq!(m.node_count(), 2);
    }

    #[test]
    fn mk_dedups_identical_triples() {
        let mut m = BddManager::new();
        let v = BddVar(0);
        let f = m.false_ref();
        let t = m.true_ref();
        let a = m.mk(v, f, t);
        let b = m.mk(v, f, t);
        assert_eq!(a, b); // hash consing: same triple -> same ref
        assert_eq!(m.node_count(), 3); // only one internal node allocated
        assert!(!m.is_terminal(a));
    }

    #[test]
    fn mk_eliminates_redundant_decision() {
        let mut m = BddManager::new();
        let v = BddVar(0);
        let x = m.mk(BddVar(1), m.false_ref(), m.true_ref()); // some internal node
        // mk(v, x, x) must collapse to x (low == high -> var redundant)
        let y = m.mk(v, x, x);
        assert_eq!(y, x);
        assert_eq!(m.node_count(), 3); // no new node for the redundant decision
    }

    #[test]
    fn mk_distinct_vars_produce_distinct_nodes() {
        let mut m = BddManager::new();
        let f = m.false_ref();
        let t = m.true_ref();
        let n1 = m.mk(BddVar(0), f, t);
        let n2 = m.mk(BddVar(1), f, t);
        assert_ne!(n1, n2); // different var -> different node
        assert_eq!(m.node_count(), 4);
    }

    #[test]
    fn mk_reuses_across_different_call_sites() {
        let mut m = BddManager::new();
        let v = BddVar(5);
        let f = m.false_ref();
        let t = m.true_ref();
        let r1 = m.mk(v, f, t);
        // build something else in between
        let _ = m.mk(BddVar(6), f, t);
        let r2 = m.mk(v, f, t);
        assert_eq!(r1, r2); // unique table persists, triple reused
    }

    #[test]
    fn node_indexing_matches_arena() {
        let mut m = BddManager::new();
        let v = BddVar(0);
        let r = m.mk(v, m.false_ref(), m.true_ref());
        assert_eq!(r.0, 2); // first internal node at arena index 2
        assert_eq!(m.node(r).var, v);
        assert_eq!(m.node(r).low, m.false_ref());
        assert_eq!(m.node(r).high, m.true_ref());
    }
}
