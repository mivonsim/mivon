//! IC3/PDR foundation — stage 1 of FORMAL-05
//! (induction-based proof, Critical, open).
//!
//! Defines the cube/frame data structures and the two core IC3 operations:
//! subsumption checking and cube resolution. Later stages will add the
//! full IC3 fixpoint loop (push_frame, generalize, block, propagate) and
//! PDR. Not yet wired into the Z3 BMC pipeline — stage 1 is the tested
//! cube/frame foundation.

use std::collections::BTreeSet;

/// A literal: a variable with a polarity (true = positive, false = negative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lit {
    pub var: usize,
    pub sign: bool,
}

/// A cube: a consistent conjunction of literals (a partial state assignment).
/// Represented as a sorted set of literals; a cube containing both `x` and
/// `!x` is inconsistent and represents the empty (false) cube.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cube {
    pub lits: BTreeSet<Lit>,
}

/// A frame: a set of cubes (a disjunction / over-approximation of reachable
/// states at some induction depth).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub cubes: Vec<Cube>,
}

impl Cube {
    /// Empty cube (true): satisfied by every state.
    pub fn empty() -> Self {
        Self {
            lits: BTreeSet::new(),
        }
    }

    /// Add a literal; returns `None` if the cube becomes inconsistent
    /// (contains both `x` and `!x`).
    pub fn push_lit(mut self, lit: Lit) -> Option<Self> {
        let opp = Lit {
            var: lit.var,
            sign: !lit.sign,
        };
        if self.lits.contains(&opp) {
            return None; // inconsistent: x and !x => false
        }
        self.lits.insert(lit);
        Some(self)
    }

    /// Does `self` imply `other`? True iff every literal of `other` is
    /// present in `self` — i.e. `self` is more specific (stronger) than
    /// `other` (`self => other`). The empty cube (TRUE) is implied by every
    /// cube and implies only itself.
    pub fn implies(&self, other: &Cube) -> bool {
        other.lits.iter().all(|lit| self.lits.contains(lit))
    }
}

impl Frame {
    pub fn empty() -> Self {
        Self { cubes: Vec::new() }
    }

    pub fn push(&mut self, cube: Cube) {
        self.cubes.push(cube);
    }
}

/// Is `sub` subsumed by `super_frame`? True iff every cube in `sub` is
/// implied by some cube in `super_frame` (each `c` in `sub` has some `s`
/// in `super_frame` with `s => c`).
pub fn ic3_frame_subsumes(sub: &Frame, super_frame: &Frame) -> bool {
    sub.cubes.iter().all(|c| super_frame.cubes.iter().any(|s| s.implies(c)))
}

/// Resolve two cubes on `var`: combine their literals, dropping `var` and
/// `!var`, provided they are not both present with opposite polarity (which
/// would make the resolvent inconsistent). Returns `None` on inconsistency.
pub fn ic3_resolve(a: &Cube, b: &Cube, var: usize) -> Option<Cube> {
    let mut lits = BTreeSet::new();
    let mut combined_inconsistent = false;
    for lit in a.lits.iter().chain(b.lits.iter()) {
        if lit.var == var {
            continue; // resolve away the pivot variable
        }
        let opp = Lit {
            var: lit.var,
            sign: !lit.sign,
        };
        if lits.contains(&opp) {
            combined_inconsistent = true;
            break;
        }
        lits.insert(*lit);
    }
    if combined_inconsistent {
        return None;
    }
    Some(Cube { lits })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(var: usize, sign: bool) -> Lit {
        Lit { var, sign }
    }

    #[test]
    fn empty_cube_is_true_implied_by_everything() {
        let empty = Cube::empty();
        let c = Cube {
            lits: BTreeSet::from([lit(0, true)]),
        };
        // everything implies TRUE (empty cube)
        assert!(c.implies(&empty));
        assert!(empty.implies(&empty));
        // TRUE does not imply a non-empty cube
        assert!(!empty.implies(&c));
    }

    #[test]
    fn cube_implies_itself_and_subset() {
        let a = Cube {
            lits: BTreeSet::from([lit(0, true), lit(1, false)]),
        };
        let b = Cube {
            lits: BTreeSet::from([lit(0, true)]),
        };
        assert!(a.implies(&a));
        assert!(a.implies(&b)); // a (x & !y) implies b (x)
        assert!(!b.implies(&a)); // b (x) does not imply a (x & !y)
    }

    #[test]
    fn inconsistent_cube_add_returns_none() {
        let c = Cube::empty().push_lit(lit(0, true)).unwrap();
        assert!(c.push_lit(lit(0, false)).is_none());
    }

    #[test]
    fn frame_subsumes_itself_and_subset() {
        let mut f = Frame::empty();
        f.push(Cube::empty());
        let mut g = Frame::empty();
        g.push(Cube {
            lits: BTreeSet::from([lit(0, true)]),
        });
        // g (x) => f (TRUE)? every cube of f (empty) implied by some cube of g:
        // empty implied by x (vacuous) => true.
        assert!(ic3_frame_subsumes(&f, &g));
        // f (TRUE) => g (x)? every cube of g implied by f: TRUE => x is false.
        assert!(!ic3_frame_subsumes(&g, &f));
        assert!(ic3_frame_subsumes(&f, &f));
    }

    #[test]
    fn resolve_removes_pivot() {
        let a = Cube {
            lits: BTreeSet::from([lit(0, true), lit(1, true)]),
        };
        let b = Cube {
            lits: BTreeSet::from([lit(0, false), lit(2, true)]),
        };
        let r = ic3_resolve(&a, &b, 0).unwrap();
        assert_eq!(r.lits, BTreeSet::from([lit(1, true), lit(2, true)]));
    }

    #[test]
    fn resolve_on_pivot_gives_empty_cube_not_none() {
        // resolving x and !x on the pivot x yields the empty (consistent) cube.
        let a = Cube {
            lits: BTreeSet::from([lit(0, true)]),
        };
        let b = Cube {
            lits: BTreeSet::from([lit(0, false)]),
        };
        let r = ic3_resolve(&a, &b, 0).unwrap();
        assert!(r.lits.is_empty());
    }

    #[test]
    fn resolve_on_non_pivot_conflict_is_none() {
        // conflict on a non-pivot variable -> inconsistent resolvent.
        let a = Cube {
            lits: BTreeSet::from([lit(0, true), lit(1, true)]),
        };
        let b = Cube {
            lits: BTreeSet::from([lit(0, false), lit(1, false)]),
        };
        assert!(ic3_resolve(&a, &b, 2).is_none());
    }
}
