//! SSA (Static Single Assignment) IR for MIR.
//!
//! Arsitektur-01 (stage 1 of N): core SSA data model.
//!
//! This module is deliberately NOT yet connected to the optimization pipeline.
//! Later stages will lower MIR into this SSA form and run CSE/DCE/LICM on it.
//! The intent of stage 1 is to establish a correct, tested SSA foundation:
//! a dominator-tree computation that satisfies the classic property that the
//! entry block dominates every reachable block.


use std::collections::HashSet;

/// A value in SSA form. A value is defined exactly once: either a constant,
/// or the result of an instruction identified by (block index, instr index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Value {
    Const(u64),
    /// Result of `instr_idx`-th instruction inside `block_idx`.
    Instr(usize, usize),
}

/// An SSA instruction. Terminators end a basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    Add(Value, Value),
    Sub(Value, Value),
    Mul(Value, Value),
    /// Phi node: `(incoming value, predecessor block index)` pairs.
    Phi(Vec<(Value, usize)>),
    Jump(usize),
    Branch {
        cond: Value,
        then: usize,
        r#else: usize,
    },
    Return(Value),
    /// Placeholder for a removed dead definition. Keeps instruction indices
    /// stable so existing `Value::Instr(block, idx)` references stay valid.
    Nop,
}

/// A single basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub instrs: Vec<Instruction>,
}

/// A whole function in SSA form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsaFunction {
    pub blocks: Vec<BasicBlock>,
    /// Index of the entry block.
    pub entry: usize,
}

impl Default for SsaFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl SsaFunction {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            entry: 0,
        }
    }

    /// Push a block and return its index.
    pub fn push_block(&mut self, block: BasicBlock) -> usize {
        let idx = self.blocks.len();
        self.blocks.push(block);
        idx
    }

    /// Direct predecessors of block `b`, in discovery order.
    pub fn predecessors(&self, b: usize) -> Vec<usize> {
        let mut preds: Vec<usize> = Vec::new();
        for (i, block) in self.blocks.iter().enumerate() {
            for instr in &block.instrs {
                match instr {
                    Instruction::Jump(t) if *t == b => preds.push(i),
                    Instruction::Branch { then, r#else, .. }
                        if *then == b || *r#else == b =>
                    {
                        preds.push(i);
                    }
                    _ => {}
                }
            }
        }
        preds
    }

    /// Direct successors of block `b`, in order.
    pub fn successors(&self, b: usize) -> Vec<usize> {
        match &self.blocks[b].instrs.last() {
            Some(Instruction::Jump(t)) => vec![*t],
            Some(Instruction::Branch { then, r#else, .. }) => vec![*then, *r#else],
            _ => Vec::new(),
        }
    }

    /// Dominator tree via the classic iterative bitvector dataflow algorithm.
    ///
    /// Returns `dom[b]` = set of block indices that dominate `b`.
    ///
    /// Correctness properties tested:
    ///   * the entry block dominates itself and every reachable block;
    ///   * every reachable block dominates itself;
    ///   * unreachable blocks have an empty dominator set;
    ///   * the result is a fixpoint (re-running is idempotent).
    pub fn dominators(&self) -> Vec<Vec<bool>> {
        let n = self.blocks.len();
        if n == 0 {
            return Vec::new();
        }
        // entry dominates only itself; every other block initially dominates all.
        let mut dom: Vec<Vec<bool>> = (0..n)
            .map(|b| {
                if b == self.entry {
                    let mut row = vec![false; n];
                    row[b] = true;
                    row
                } else {
                    vec![true; n]
                }
            })
            .collect();

        let mut changed = true;
        while changed {
            changed = false;
            for b in 0..n {
                if b == self.entry {
                    continue;
                }
                let preds = self.predecessors(b);
                if preds.is_empty() {
                    continue;
                }
                let mut new_dom = vec![true; n];
                for p in &preds {
                    for d in 0..n {
                        new_dom[d] = new_dom[d] && dom[*p][d];
                    }
                }
                new_dom[b] = true;
                if new_dom != dom[b] {
                    dom[b] = new_dom;
                    changed = true;
                }
            }
        }

        // Blocks unreachable from the entry keep the all-true initial value;
        // clear them so they have an empty dominator set.
        let mut reachable = vec![false; n];
        reachable[self.entry] = true;
        let mut stack = vec![self.entry];
        while let Some(b) = stack.pop() {
            for s in self.successors(b) {
                if !reachable[s] {
                    reachable[s] = true;
                    stack.push(s);
                }
            }
        }
        for b in 0..n {
            if b != self.entry && !reachable[b] {
                dom[b] = vec![false; n];
            }
        }
        dom
    }

    /// Immediate dominator of each block (None for entry and unreachable blocks).
    ///
    /// `idom[b]` is the strict dominator of `b` that is itself dominated by
    /// every other strict dominator of `b` (the "lowest" dominator).
    pub fn idom(&self) -> Vec<Option<usize>> {
        let dom = self.dominators();
        let n = self.blocks.len();
        let mut result = vec![None; n];
        for b in 0..n {
            if b == self.entry {
                continue;
            }
            let strict: Vec<usize> = (0..n).filter(|&d| d != b && dom[b][d]).collect();
            // idom is the strict dominator dominated by all the others.
            let idom = strict
                .iter()
                .copied()
                .find(|&d| strict.iter().all(|&e| e == d || dom[d][e]));
            result[b] = idom;
        }
        result
    }

    /// Local dead-store elimination on SSA form.
    ///
    /// Replaces every non-terminator instruction whose defined result is never
    /// read anywhere in the function with `Instruction::Nop`. Indices are not
    /// shifted and the CFG is not modified, so existing `Value::Instr(block,
    /// idx)` references stay valid. Returns the number of stores eliminated.
    ///
    /// Safe for stage 2: no block removal (no index renumbering), no CFG
    /// change, and a value is only eliminated when provably unread.
    pub fn eliminate_dead_stores(&mut self) -> usize {
        let mut read: HashSet<Value> = HashSet::new();
        for block in &self.blocks {
            for instr in &block.instrs {
                collect_reads(instr, &mut read);
            }
        }
        let mut removed = 0;
        for (bi, block) in self.blocks.iter_mut().enumerate() {
            for (ii, instr) in block.instrs.iter_mut().enumerate() {
                if is_terminator(instr) {
                    continue;
                }
                // Only non-terminators (Add/Sub/Mul/Phi) define a result.
                if !defines_result(instr) {
                    continue;
                }
                let defined = Value::Instr(bi, ii);
                if !read.contains(&defined) {
                    *instr = Instruction::Nop;
                    removed += 1;
                }
            }
        }
        removed
    }
}

/// Values read by an instruction (its operands).
fn collect_reads(instr: &Instruction, out: &mut HashSet<Value>) {
    match instr {
        Instruction::Add(a, b)
        | Instruction::Sub(a, b)
        | Instruction::Mul(a, b) => {
            out.insert(*a);
            out.insert(*b);
        }
        Instruction::Phi(pairs) => {
            for (v, _) in pairs {
                out.insert(*v);
            }
        }
        Instruction::Branch { cond, .. } => {
            out.insert(*cond);
        }
        Instruction::Return(v) => {
            out.insert(*v);
        }
        Instruction::Jump(_) | Instruction::Nop => {}
    }
}

/// True if the instruction ends a basic block (defines no SSA value).
fn is_terminator(instr: &Instruction) -> bool {
    matches!(
        instr,
        Instruction::Jump(_) | Instruction::Branch { .. } | Instruction::Return(_)
    )
}

/// True if the instruction defines an SSA result (Add/Sub/Mul/Phi).
fn defines_result(instr: &Instruction) -> bool {
    matches!(
        instr,
        Instruction::Add(..) | Instruction::Sub(..) | Instruction::Mul(..) | Instruction::Phi(..)
    )
}

/// Set of values defined by a block (the result of each non-terminator
/// instruction, in order). Used by later stages to place phi nodes.
pub fn defined_values(block: &BasicBlock) -> Vec<Value> {
    block
        .instrs
        .iter()
        .enumerate()
        .filter_map(|(i, instr)| match instr {
            Instruction::Jump(_)
            | Instruction::Branch { .. }
            | Instruction::Return(_) => None,
            _ => Some(Value::Instr(0, i)), // block index filled in by caller
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(instrs: Vec<Instruction>) -> BasicBlock {
        BasicBlock { instrs }
    }

    /// Builds a diamond CFG:
    ///   0 (entry): t = a + b; if t goto 1 else 2
    ///   1:         x = t * 2; goto 3
    ///   2:         y = t + 1; goto 3
    ///   3:         z = phi(x from 1, y from 2); return z
    fn diamond() -> SsaFunction {
        let mut f = SsaFunction::new();
        let a = Value::Const(1);
        let b = Value::Const(2);
        let b0 = block(vec![
            Instruction::Add(a, b),   // 0: t
            Instruction::Branch {
                cond: Value::Instr(0, 0),
                then: 1,
                r#else: 2,
            },
        ]);
        let b1 = block(vec![
            Instruction::Mul(Value::Instr(0, 0), Value::Const(2)), // 1: x
            Instruction::Jump(3),
        ]);
        let b2 = block(vec![
            Instruction::Add(Value::Instr(0, 0), Value::Const(1)), // 2: y
            Instruction::Jump(3),
        ]);
        let b3 = block(vec![
            Instruction::Phi(vec![
                (Value::Instr(1, 0), 1),
                (Value::Instr(2, 0), 2),
            ]),
            Instruction::Return(Value::Instr(3, 0)),
        ]);
        f.push_block(b0); // 0
        f.push_block(b1); // 1
        f.push_block(b2); // 2
        f.push_block(b3); // 3
        f
    }

    #[test]
    fn diamond_dominators() {
        let f = diamond();
        let dom = f.dominators();
        // entry (0) dominates every block.
        for b in 0..4 {
            assert!(dom[b][0], "block {b} should be dominated by entry");
        }
        // every block dominates itself.
        for b in 0..4 {
            assert!(dom[b][b], "block {b} should dominate itself");
        }
        // block 1 is dominated only by {0, 1}.
        assert_eq!(dom[1], vec![true, true, false, false]);
        // block 2 is dominated only by {0, 2}.
        assert_eq!(dom[2], vec![true, false, true, false]);
        // block 3 is dominated by {0, 3} (both 1 and 2 converge, only entry + self).
        assert_eq!(dom[3], vec![true, false, false, true]);
    }

    #[test]
    fn diamond_idom() {
        let f = diamond();
        let idom = f.idom();
        assert_eq!(idom[0], None);
        assert_eq!(idom[1], Some(0));
        assert_eq!(idom[2], Some(0));
        assert_eq!(idom[3], Some(0));
    }

    #[test]
    fn dominators_are_idempotent() {
        let f = diamond();
        let d1 = f.dominators();
        let d2 = SsaFunction {
            blocks: f.blocks.clone(),
            entry: f.entry,
        }
        .dominators();
        assert_eq!(d1, d2);
    }

    #[test]
    fn empty_function_has_no_dominators() {
        let f = SsaFunction::new();
        assert!(f.dominators().is_empty());
    }

    #[test]
    fn single_block_dominates_itself() {
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Return(Value::Const(0))]));
        let dom = f.dominators();
        assert_eq!(dom, vec![vec![true]]);
        assert_eq!(f.idom(), vec![None]);
    }

    #[test]
    fn predecessors_are_correct() {
        let f = diamond();
        assert!(f.predecessors(0).is_empty());
        assert_eq!(f.predecessors(1), vec![0]);
        assert_eq!(f.predecessors(2), vec![0]);
        assert_eq!(f.predecessors(3), vec![1, 2]);
    }

    #[test]
    fn unreachable_block_has_no_idom() {
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Jump(1)])); // 0 entry, jumps to 1
        f.push_block(block(vec![Instruction::Return(Value::Const(0))])); // 1
        f.push_block(block(vec![Instruction::Return(Value::Const(1))])); // 2 unreachable
        let idom = f.idom();
        assert_eq!(idom[0], None);
        assert_eq!(idom[1], Some(0));
        assert_eq!(idom[2], None);
    }

    #[test]
    fn dead_store_eliminates_unused() {
        // block 0: dead = add(1,2); live = add(4,5); return live
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,0) dead
            Instruction::Add(Value::Const(4), Value::Const(5)), // Instr(0,1) live
            Instruction::Return(Value::Instr(0, 1)),
        ]));
        let removed = f.eliminate_dead_stores();
        assert_eq!(removed, 1);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Nop));
        assert!(matches!(f.blocks[0].instrs[1], Instruction::Add(..)));
        assert!(matches!(f.blocks[0].instrs[2], Instruction::Return(_)));
    }

    #[test]
    fn dead_store_chain_eliminates_only_bottom() {
        // a=add(1,2) is read by b=add(a,3); b is unused; c=add(7,8) live.
        // Only b is eliminated (a is still "read" by b in the original read set).
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,0) = a
            Instruction::Add(Value::Instr(0, 0), Value::Const(3)), // Instr(0,1) = b
            Instruction::Add(Value::Const(7), Value::Const(8)), // Instr(0,2) = c
            Instruction::Return(Value::Instr(0, 2)),
        ]));
        let removed = f.eliminate_dead_stores();
        assert_eq!(removed, 1);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Add(..))); // a kept
        assert!(matches!(f.blocks[0].instrs[1], Instruction::Nop)); // b eliminated
        assert!(matches!(f.blocks[0].instrs[2], Instruction::Add(..))); // c kept
    }

    #[test]
    fn dead_store_none_when_all_used() {
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,0)
            Instruction::Return(Value::Instr(0, 0)),
        ]));
        let removed = f.eliminate_dead_stores();
        assert_eq!(removed, 0);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Add(..)));
    }

    #[test]
    fn dead_store_phi_eliminated() {
        // block0 -> block1 (p=add) -> block2 (q=add) -> block3
        // block3: r=phi(p,q) [dead], s=add(5,6), return s
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Jump(1)])); // 0
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // p=Instr(1,0)
            Instruction::Jump(2),
        ])); // 1
        f.push_block(block(vec![
            Instruction::Add(Value::Const(3), Value::Const(4)), // q=Instr(2,0)
            Instruction::Jump(3),
        ])); // 2
        f.push_block(block(vec![
            Instruction::Phi(vec![
                (Value::Instr(1, 0), 1),
                (Value::Instr(2, 0), 2),
            ]), // r=Instr(3,0) dead
            Instruction::Add(Value::Const(5), Value::Const(6)), // s=Instr(3,1)
            Instruction::Return(Value::Instr(3, 1)),
        ])); // 3
        let removed = f.eliminate_dead_stores();
        assert_eq!(removed, 1);
        assert!(matches!(f.blocks[3].instrs[0], Instruction::Nop)); // r eliminated
        assert!(matches!(f.blocks[3].instrs[1], Instruction::Add(..))); // s kept
        assert!(matches!(f.blocks[1].instrs[0], Instruction::Add(..))); // p kept
        assert!(matches!(f.blocks[2].instrs[0], Instruction::Add(..))); // q kept
    }
}
